//! Paper-trading signal engine: raises signals on risk events and later
//! grades them against realized price action ("did the signal play out?"").

use serde::Serialize;

#[derive(Debug, Clone)]
struct OpenSignal {
    id: u64,
    symbol: String,
    kind: String,
    ts: u64,
    price: f64,
    depth: f64,
    note: String,
}

/// A signal whose evaluation window has elapsed.
#[derive(Debug, Clone, Serialize)]
pub struct ClosedSignal {
    pub id: u64,
    pub symbol: String,
    pub kind: String,
    pub ts: u64,
    pub price_at_signal: f64,
    pub price_after: f64,
    pub change_pct: f64,
    pub horizon_secs: u64,
    pub validated: bool,
    pub note: String,
}

pub struct SignalEngine {
    horizon_secs: u64,
    validate_drop_pct: f64,
    cooldown_secs: u64,
    next_id: u64,
    open: Vec<OpenSignal>,
}

impl SignalEngine {
    /// `validate_drop_pct`: fractional drop vs signal price considered a "hit"
    /// (0.005 == -0.5%). Bearish validation: risk signals predict downside.
    pub fn new(horizon_secs: u64, validate_drop_pct: f64, cooldown_secs: u64) -> Self {
        Self {
            horizon_secs,
            validate_drop_pct,
            cooldown_secs,
            next_id: 1,
            open: Vec::new(),
        }
    }

    /// Cooldown prevents duplicate signals of the same kind for the same asset.
    pub fn can_raise(&self, symbol: &str, kind: &str, now: u64) -> bool {
        !self
            .open
            .iter()
            .any(|s| s.symbol == symbol && s.kind == kind && now.saturating_sub(s.ts) < self.cooldown_secs)
    }

    /// Registers a new open signal. Caller should gate with `can_raise`.
    pub fn raise(
        &mut self,
        symbol: &str,
        kind: &str,
        now: u64,
        price: f64,
        depth: f64,
        note: &str,
    ) -> (u64, u64, f64, f64, String) {
        let id = self.next_id;
        self.next_id += 1;
        self.open.push(OpenSignal {
            id,
            symbol: symbol.to_string(),
            kind: kind.to_string(),
            ts: now,
            price,
            depth,
            note: note.to_string(),
        });
        (id, now, price, depth, note.to_string())
    }

    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// Expires matured signals and grades them against the latest prices.
    /// `prices`: latest (symbol, price) pairs; missing price => invalidated.
    pub fn on_tick(
        &mut self,
        now: u64,
        prices: &[(String, f64)],
    ) -> Vec<ClosedSignal> {
        let horizon = self.horizon_secs;
        let drop_pct = self.validate_drop_pct;
        let mut matured_ids = Vec::new();
        for s in self.open.iter() {
            if now.saturating_sub(s.ts) >= horizon {
                matured_ids.push(s.id);
            }
        }
        let mut out = Vec::new();
        for id in matured_ids {
            let pos = match self.open.iter().position(|s| s.id == id) {
                Some(p) => p,
                None => continue,
            };
            let s = self.open.remove(pos);
            let price_after = prices
                .iter()
                .find(|(sym, _)| *sym == s.symbol)
                .map(|(_, p)| *p)
                .unwrap_or(0.0);
            let change_pct = if s.price > 1e-9 && price_after > 0.0 {
                (price_after - s.price) / s.price
            } else {
                0.0
            };
            let validated = change_pct <= -drop_pct;
            out.push(ClosedSignal {
                id: s.id,
                symbol: s.symbol,
                kind: s.kind,
                ts: s.ts,
                price_at_signal: s.price,
                price_after,
                change_pct,
                horizon_secs: horizon,
                validated,
                note: s.note,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_validated_on_drop() {
        let mut eng = SignalEngine::new(60, 0.005, 0);
        assert!(eng.can_raise("HYPE", "CRITICAL", 1_000));
        eng.raise("HYPE", "CRITICAL", 1_000, 100.0, 5_000.0, "test");
        assert_eq!(eng.open_count(), 1);

        // before horizon: nothing closes
        assert!(eng.on_tick(1_030, &[("HYPE".into(), 80.0)]).is_empty());

        let closed = eng.on_tick(1_061, &[("HYPE".into(), 98.0)]);
        assert_eq!(closed.len(), 1);
        assert!(closed[0].validated);
        assert!((closed[0].change_pct + 0.02).abs() < 1e-9);
        assert_eq!(eng.open_count(), 0);
    }

    #[test]
    fn test_signal_invalidated_on_rise() {
        let mut eng = SignalEngine::new(60, 0.005, 0);
        eng.raise("HYPE", "WARN", 0, 100.0, 9_000.0, "t");
        let closed = eng.on_tick(61, &[("HYPE".into(), 105.0)]);
        assert_eq!(closed.len(), 1);
        assert!(!closed[0].validated);
    }

    #[test]
    fn test_cooldown_blocks_duplicates() {
        let mut eng = SignalEngine::new(60, 0.01, 300);
        eng.raise("HYPE", "WARN", 0, 100.0, 9_000.0, "t");
        assert!(!eng.can_raise("HYPE", "WARN", 299));
        assert!(eng.can_raise("HYPE", "WARN", 301));
    }
}
