//! One-Pole Low-Pass Filter Smoother and Audio Fader Curve Conversions
//!
//! Provides zipper-noise-free gain interpolation for real-time mixer controls:
//! g[n] = g[n-1] + alpha * (g_target - g[n-1])
//! with tau ~ 15 ms.

/// One-Pole low-pass filter for smooth parameter interpolation.
#[derive(Debug, Clone)]
pub struct OnePoleSmoother {
    current: f32,
    target: f32,
    alpha: f32,
}

impl OnePoleSmoother {
    /// Create a new smoother with the given sample rate (e.g. 48000.0) and time constant tau (seconds, e.g. 0.015).
    pub fn new(sample_rate: f32, tau_seconds: f32) -> Self {
        let sample_rate = if sample_rate <= 0.0 { 48000.0 } else { sample_rate };
        let tau = if tau_seconds <= 0.0 { 0.015 } else { tau_seconds };
        // alpha = 1.0 - exp(-1.0 / (Fs * tau))
        let alpha = 1.0 - (-1.0 / (sample_rate * tau)).exp();
        Self {
            current: 1.0,
            target: 1.0,
            alpha,
        }
    }

    /// Create with an initial value.
    pub fn with_initial_value(sample_rate: f32, tau_seconds: f32, initial_value: f32) -> Self {
        let mut s = Self::new(sample_rate, tau_seconds);
        s.current = initial_value;
        s.target = initial_value;
        s
    }

    /// Set the target value to smoothly interpolate towards.
    #[inline(always)]
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Advance the smoother by one sample and return the interpolated value.
    #[inline(always)]
    pub fn next_sample(&mut self) -> f32 {
        self.current += self.alpha * (self.target - self.current);
        self.current
    }

    /// Current smoothed value.
    #[inline(always)]
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Target value.
    #[inline(always)]
    pub fn target(&self) -> f32 {
        self.target
    }

    /// Immediately set both current and target to a specific value without smoothing.
    pub fn reset(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    /// Returns true if the smoother has essentially settled to the target value.
    pub fn is_settled(&self) -> bool {
        (self.current - self.target).abs() < 1e-5
    }
}

/// Convert a normalized slider position in [0.0, 1.0] to a linear gain factor.
///
/// Mapping:
/// - 0.0 -> 0.0 (-inf dB, silence)
/// - 0.5 -> 1.0 (0 dB, Unity Gain)
/// - 1.0 -> ~3.98107 (+12 dB Boost)
#[inline]
pub fn slider_to_gain(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.0 {
        0.0
    } else if x <= 0.5 {
        // Lower half: quadratic attenuation curve from 0.0 to 1.0
        let norm = x / 0.5;
        norm * norm
    } else {
        // Upper half: exponential boost curve from 0 dB (1.0) to +12 dB (~3.981)
        let db = 24.0 * (x - 0.5); // 0.0 dB at 0.5, +12.0 dB at 1.0
        10.0f32.powf(db / 20.0)
    }
}

/// Convert a normalized slider position in [0.0, 1.0] to decibels (dBFS).
#[inline]
pub fn slider_to_db(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.0 {
        f32::NEG_INFINITY
    } else if x <= 0.5 {
        // dB = 40 * log10(x / 0.5)
        40.0 * (x / 0.5).log10()
    } else {
        24.0 * (x - 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slider_curve_cardinal_points() {
        // 0.0 -> 0.0 (-inf dB)
        assert_eq!(slider_to_gain(0.0), 0.0);
        assert!(slider_to_db(0.0).is_infinite() && slider_to_db(0.0) < 0.0);

        // 0.5 -> 1.0 (0 dB Unity Gain)
        let gain_mid = slider_to_gain(0.5);
        assert!((gain_mid - 1.0).abs() < 1e-6, "Expected 1.0 at 0.5, got {gain_mid}");
        assert!((slider_to_db(0.5) - 0.0).abs() < 1e-6);

        // 1.0 -> +12 dB (approx 3.98107)
        let gain_max = slider_to_gain(1.0);
        assert!((gain_max - 3.9810717).abs() < 1e-4, "Expected ~3.981 at 1.0, got {gain_max}");
        assert!((slider_to_db(1.0) - 12.0).abs() < 1e-5);
    }

    #[test]
    fn test_smoother_convergence() {
        let sample_rate = 48000.0;
        let tau = 0.015; // 15 ms
        let mut smoother = OnePoleSmoother::with_initial_value(sample_rate, tau, 0.0);
        smoother.set_target(1.0);

        // After tau seconds (720 samples), 1 - 1/e ~ 63.2%
        let samples_tau = (sample_rate * tau) as usize;
        for _ in 0..samples_tau {
            smoother.next_sample();
        }

        let val_at_tau = smoother.current();
        let expected_63 = 1.0 - (-1.0f32).exp(); // ~ 0.63212
        assert!(
            (val_at_tau - expected_63).abs() < 0.02,
            "Value at tau should be ~0.632, got {val_at_tau}"
        );

        // After 5*tau (~75 ms, 3600 samples), smoother should reach > 99.3%
        for _ in 0..(4 * samples_tau) {
            smoother.next_sample();
        }
        assert!(
            smoother.current() > 0.99,
            "After 5*tau value should be > 0.99, got {}",
            smoother.current()
        );
    }

    #[test]
    fn test_smoother_no_zipper_noise() {
        let mut smoother = OnePoleSmoother::with_initial_value(48000.0, 0.015, 0.0);
        smoother.set_target(1.0);

        let mut prev = 0.0;
        for _ in 0..1000 {
            let cur = smoother.next_sample();
            assert!(cur >= prev, "Smoother must be monotonic during upward step");
            // The jump between consecutive samples must be tiny (alpha ~ 0.00138)
            let step = cur - prev;
            assert!(step < 0.002, "Step between samples must be smooth, got {step}");
            prev = cur;
        }
    }
}
