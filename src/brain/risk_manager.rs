use crate::brain::mathematics::{VolumeWindow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RiskState {
    Ok,
    Warn,
    Critical,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Action {
    AlertTelegram(String),
    ThrottleBuyback { recommended_usd: f64 },
}

#[derive(Debug, Clone)]
pub struct RiskConfig {
    pub depth_min_usd: f64,           // below this -> critical
    pub unlock_ratio_warn: f64,       // upcoming_unlock_usd > unlock_ratio_warn * volume_24h -> warn
    pub buyback_budget_ratio: f64,    // fraction of 24h volume as buyback budget
}

pub struct CentralRiskManager {
    cfg: RiskConfig,
    pub window: VolumeWindow,
}

impl CentralRiskManager {
    pub fn new(cfg: RiskConfig) -> Self {
        let window = VolumeWindow::new(24);
        Self { cfg, window }
    }

    /// Evaluate current state given metrics.
    /// Returns (RiskState, optional Action)
    pub fn evaluate(&self, volume_24h: f64, depth_usd: f64, upcoming_unlock_usd: f64) -> (RiskState, Option<Action>) {
        // If depth is very small -> critical
        if depth_usd <= self.cfg.depth_min_usd {
            return (
                RiskState::Critical,
                Some(Action::AlertTelegram(format!("CRITICAL: depth_usd={:.2} < min={:.2}", depth_usd, self.cfg.depth_min_usd))),
            );
        }

        // If upcoming unlock is large fraction of 24h volume -> warn
        if volume_24h > 0.0 && upcoming_unlock_usd > (self.cfg.unlock_ratio_warn * volume_24h) {
            let recommended = (volume_24h * self.cfg.buyback_budget_ratio).min(upcoming_unlock_usd);
            return (
                RiskState::Warn,
                Some(Action::ThrottleBuyback { recommended_usd: recommended }),
            );
        }

        (RiskState::Ok, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_manager_critical() {
        let cfg = RiskConfig { depth_min_usd: 1000.0, unlock_ratio_warn: 0.1, buyback_budget_ratio: 0.05 };
        let rm = CentralRiskManager::new(cfg);
        let (state, action) = rm.evaluate(1_000_000.0, 500.0, 10_000.0);
        assert_eq!(state, RiskState::Critical);
        assert!(action.is_some());
    }

    #[test]
    fn test_risk_manager_warn() {
        let cfg = RiskConfig { depth_min_usd: 100.0, unlock_ratio_warn: 0.1, buyback_budget_ratio: 0.05 };
        let rm = CentralRiskManager::new(cfg);
        // upcoming unlock is 20% of volume_24h -> warn
        let (state, action) = rm.evaluate(1000.0, 1000.0, 200.0);
        assert_eq!(state, RiskState::Warn);
        assert!(action.is_some());
        match action.unwrap() {
            Action::ThrottleBuyback { recommended_usd } => assert!(recommended_usd > 0.0),
            _ => panic!("unexpected action"),
        }
    }

    #[test]
    fn test_risk_manager_ok() {
        let cfg = RiskConfig { depth_min_usd: 100.0, unlock_ratio_warn: 0.2, buyback_budget_ratio: 0.05 };
        let rm = CentralRiskManager::new(cfg);
        let (state, action) = rm.evaluate(1000.0, 1000.0, 50.0);
        assert_eq!(state, RiskState::Ok);
        assert!(action.is_none());
    }
}
