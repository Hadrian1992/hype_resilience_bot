use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// Compute orderbook bid depth in USD.
/// bids: vector of (price_str, size_str) where price is USDC per token and size is token units.
pub fn compute_bid_depth_usd_from_strings(
    bids: &[(String, String)],
    current_price: &str,
    pct_threshold: f64,
) -> Decimal {
    let cur = Decimal::from_str(current_price).unwrap_or(Decimal::ZERO);
    let pct = Decimal::from_f64(pct_threshold).unwrap_or(Decimal::from_f64(0.02).unwrap());
    let limit = cur * (Decimal::ONE - pct);

    bids
        .iter()
        .filter_map(|(pxs, szs)| {
            let px = Decimal::from_str(pxs).ok()?;
            let sz = Decimal::from_str(szs).ok()?;
            if px >= limit {
                Some(px * sz)
            } else {
                None
            }
        })
        .fold(Decimal::ZERO, |acc, x| acc + x)
}

/// Simpler f64 variant for pre-parsed floats
pub fn compute_bid_depth_usd(bids: &[(f64, f64)], current_price: f64, pct_threshold: f64) -> f64 {
    let limit = current_price * (1.0 - pct_threshold);
    bids
        .iter()
        .filter(|&&(px, _sz)| px >= limit)
        .map(|&(px, sz)| px * sz)
        .sum()
}

/// Rolling 24h volume aggregator (per-symbol)
pub struct VolumeWindow {
    window: VecDeque<(u64, f64)>, // (timestamp_secs, volume_usd)
    total: f64,
    window_seconds: u64,
}

impl VolumeWindow {
    pub fn new(hours: u64) -> Self {
        Self {
            window: VecDeque::new(),
            total: 0.0,
            window_seconds: hours * 3600,
        }
    }

    pub fn push_trade(&mut self, timestamp_secs: u64, volume_usd: f64) {
        self.window.push_back((timestamp_secs, volume_usd));
        self.total += volume_usd;
        self.evict_old(timestamp_secs);
    }

    fn evict_old(&mut self, now_secs: u64) {
        while let Some(&(t, v)) = self.window.front() {
            if t + self.window_seconds < now_secs {
                self.window.pop_front();
                self.total -= v;
            } else {
                break;
            }
        }
    }

    pub fn total_24h(&self, now_secs: Option<u64>) -> f64 {
        if let Some(now) = now_secs {
            // return total after evicting on the fly (non-mutating)
            let cutoff = now - self.window_seconds;
            let mut sum = 0.0;
            for &(t, v) in &self.window {
                if t >= cutoff {
                    sum += v;
                }
            }
            sum
        } else {
            self.total
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[test]
    fn test_compute_bid_depth_usd_strings() {
        let bids = vec![
            ("10.0".to_string(), "5".to_string()), // $50
            ("9.9".to_string(), "10".to_string()), // $99
            ("9.0".to_string(), "1".to_string()),  // $9 (may be out of 2% window)
        ];
        let cur = "10.0";
        let depth = compute_bid_depth_usd_from_strings(&bids, cur, 0.02);
        // limit = 10 * 0.98 = 9.8 -> include 10.0 and 9.9
        let expected = Decimal::from_f64(50.0 + 99.0).unwrap();
        assert_eq!(depth, expected);
    }

    #[test]
    fn test_volume_window() {
        let mut vw = VolumeWindow::new(24);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        vw.push_trade(now - 100, 100.0);
        vw.push_trade(now - 10, 50.0);
        let total = vw.total_24h(Some(now));
        assert!((total - 150.0).abs() < 1e-6);
    }
}
