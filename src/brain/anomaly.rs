//! Lightweight anomaly detection: EWMA mean/variance z-scores on depth.

/// Standardizes observations against an EWMA mean/stddev.
/// Returns Some(z) only after `warmup` samples have been absorbed.
pub struct EwmaZ {
    alpha: f64,
    warmup: usize,
    n: usize,
    mean: f64,
    var: f64,
    initialized: bool,
}

impl EwmaZ {
    pub fn new(alpha: f64, warmup: usize) -> Self {
        Self {
            alpha,
            warmup,
            n: 0,
            mean: 0.0,
            var: 0.0,
            initialized: false,
        }
    }

    pub fn update(&mut self, x: f64) -> Option<f64> {
        if !x.is_finite() {
            return None;
        }
        if !self.initialized {
            self.initialized = true;
            self.mean = x;
            self.var = 0.0;
            self.n = 1;
            return None;
        }
        let diff = x - self.mean;
        // floor stddev so flat series still flag large jumps
        let sd = self
            .var
            .sqrt()
            .max(self.mean.abs() * 1e-4)
            .max(1e-9);
        let z = diff / sd;
        self.mean += self.alpha * diff;
        self.var += self.alpha * (diff * diff - self.var);
        self.n += 1;
        if self.n >= self.warmup {
            Some(z)
        } else {
            None
        }
    }

    /// Current EWMA mean (baseline of the latest observation).
    pub fn mean(&self) -> f64 {
        self.mean
    }
}

/// Flags depth readings whose |z| exceeds the threshold.
pub struct DepthAnomalyDetector {
    ewma: EwmaZ,
    threshold: f64,
}

impl DepthAnomalyDetector {
    /// Minimum relative deviation from baseline required ON TOP of the z-score,
    /// so perfectly flat series don't flag microscopic moves as anomalies.
    const MIN_REL_DEV: f64 = 0.02;

    pub fn new(threshold: f64) -> Self {
        Self {
            ewma: EwmaZ::new(0.05, 60),
            threshold,
        }
    }

    /// Returns (z-score when warmed up, is_anomalous).
    pub fn update(&mut self, depth_usd: f64) -> (Option<f64>, bool) {
        match self.ewma.update(depth_usd) {
            Some(z) => {
                let baseline = self.ewma.mean();
                let rel_dev = if baseline.abs() > 1e-9 {
                    (depth_usd - baseline).abs() / baseline.abs()
                } else {
                    f64::INFINITY
                };
                let anomalous = z.abs() > self.threshold && rel_dev > Self::MIN_REL_DEV;
                (Some(z), anomalous)
            }
            None => (None, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flat_series_ok_then_spike_flagged() {
        let mut det = DepthAnomalyDetector::new(3.0);
        let mut flagged_during_warmup = false;
        for _ in 0..300 {
            let (_, a) = det.update(100.0);
            flagged_during_warmup |= a;
        }
        // steady series must stay calm
        let (_, a2) = det.update(100.5);
        assert!(!a2);
        let (_, a3) = det.update(40.0); // sudden -60% dump
        assert!(a3, "expected spike to be flagged as anomaly");
    }

    #[test]
    fn test_warmup_suppresses_scores() {
        let mut e = EwmaZ::new(0.05, 10);
        assert!(e.update(5.0).is_none());
        assert!(e.update(5.0).is_none());
    }
}
