//! 带宽预测模型
//!
//! 基于 Holt-Winters 指数平滑的短期带宽预测。

/// Holt-Winters 带宽预测器
pub struct HoltWintersPredictor {
    /// 水平分量
    level: f64,
    /// 趋势分量
    trend: f64,
    /// 水平平滑因子
    alpha: f64,
    /// 趋势平滑因子
    beta: f64,
    /// 是否已初始化
    initialized: bool,
}

impl HoltWintersPredictor {
    pub fn new(alpha: f64, beta: f64) -> Self {
        Self {
            level: 0.0,
            trend: 0.0,
            alpha: alpha.clamp(0.0, 1.0),
            beta: beta.clamp(0.0, 1.0),
            initialized: false,
        }
    }

    /// 记录新的带宽观测值(字节/秒)
    pub fn observe(&mut self, value: f64) {
        if !self.initialized {
            self.level = value;
            self.trend = 0.0;
            self.initialized = true;
            return;
        }
        let new_level = self.alpha * value + (1.0 - self.alpha) * (self.level + self.trend);
        self.trend = self.beta * (new_level - self.level) + (1.0 - self.beta) * self.trend;
        self.level = new_level;
    }

    /// 预测未来第 steps 步的带宽(不会返回负值)
    pub fn predict(&self, steps: u64) -> f64 {
        if !self.initialized {
            return 0.0;
        }
        (self.level + self.trend * steps as f64).max(0.0)
    }

    /// 当前水平估计
    pub fn current_level(&self) -> f64 {
        self.level
    }
}

impl Default for HoltWintersPredictor {
    fn default() -> Self {
        Self::new(0.3, 0.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_predictor_initialization() {
        let pred = HoltWintersPredictor::default();
        assert_eq!(pred.predict(1), 0.0);
    }

    #[test]
    fn test_predictor_single_observation() {
        let mut pred = HoltWintersPredictor::default();
        pred.observe(100.0);
        assert_eq!(pred.current_level(), 100.0);
        assert_eq!(pred.predict(1), 100.0); // trend = 0
    }

    #[test]
    fn test_predictor_trend() {
        let mut pred = HoltWintersPredictor::new(0.5, 0.5);
        pred.observe(100.0);
        pred.observe(200.0);
        // 水平和趋势应该上升
        assert!(pred.predict(1) > 100.0);
    }

    #[test]
    fn test_predictor_stable() {
        let mut pred = HoltWintersPredictor::default();
        for _ in 0..10 {
            pred.observe(1000.0);
        }
        let p = pred.predict(1);
        assert!((p - 1000.0).abs() < 10.0);
    }

    #[test]
    fn test_predictor_no_observations() {
        let pred = HoltWintersPredictor::default();
        assert_eq!(pred.predict(0), 0.0);
    }
}
