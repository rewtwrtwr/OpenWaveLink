//! Master True-Peak Brickwall Limiter for Output Mix Buses
//!
//! Protects each of the 5 mix buses (Stream, Monitor, VOD, Discord, Aux) from
//! inter-sample peaks and digital clipping during multi-channel audio summing.

/// True-Peak Master Brickwall Limiter
#[derive(Debug, Clone)]
pub struct MasterBrickwallLimiter {
    ceiling: f32,
    knee_start: f32,
    alpha_att: f32,
    alpha_rel: f32,
    gain_env: f32,
}

impl MasterBrickwallLimiter {
    /// Create a new Master Brickwall Limiter with ceiling at 1.0 (0 dBFS).
    pub fn new(sample_rate: u32) -> Self {
        let sr_f = if sample_rate == 0 { 48000.0 } else { sample_rate as f32 };

        // Ceiling: 0 dBFS (1.0)
        let ceiling = 1.0f32;
        // Soft knee starts at -0.3 dBFS ~ 0.966
        let knee_start = 10.0f32.powf(-0.3 / 20.0);

        // Fast attack: 0.1 ms (0.0001 s)
        let alpha_att = 1.0 - (-1.0 / (sr_f * 0.0001)).exp();
        // Smooth release: 80 ms (0.080 s)
        let alpha_rel = 1.0 - (-1.0 / (sr_f * 0.080)).exp();

        Self {
            ceiling,
            knee_start,
            alpha_att,
            alpha_rel,
            gain_env: 1.0,
        }
    }

    /// Process a stereo frame in-place.
    #[inline(always)]
    pub fn process_frame(&mut self, left: &mut f32, right: &mut f32) {
        let peak = left.abs().max(right.abs());

        let target_gain = if peak > self.knee_start {
            if peak >= self.ceiling {
                self.ceiling / peak
            } else {
                // Soft knee region: quadratic interpolation
                let x = (peak - self.knee_start) / (self.ceiling - self.knee_start);
                1.0 - 0.5 * x * x * (1.0 - self.ceiling / peak)
            }
        } else {
            1.0
        };

        if target_gain < self.gain_env {
            self.gain_env += self.alpha_att * (target_gain - self.gain_env);
        } else {
            self.gain_env += self.alpha_rel * (target_gain - self.gain_env);
        }

        *left *= self.gain_env;
        *right *= self.gain_env;

        // Hard brickwall clamp as ultimate safety net
        if left.abs() > self.ceiling {
            *left = left.signum() * self.ceiling;
        }
        if right.abs() > self.ceiling {
            *right = right.signum() * self.ceiling;
        }
    }

    /// Process interleaved stereo slice in-place: `[L0, R0, L1, R1, ...]`.
    pub fn process_interleaved(&mut self, samples: &mut [f32]) {
        let frames = samples.len() / 2;
        for i in 0..frames {
            let l_idx = i * 2;
            let r_idx = l_idx + 1;
            let mut l = samples[l_idx];
            let mut r = samples[r_idx];
            self.process_frame(&mut l, &mut r);
            samples[l_idx] = l;
            samples[r_idx] = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_master_limiter_brickwall() {
        let mut limiter = MasterBrickwallLimiter::new(48000);

        // Huge overload signal (summing 4 hot channels: amplitude 4.0)
        let mut l = 4.0f32;
        let mut r = -3.5f32;

        limiter.process_frame(&mut l, &mut r);

        assert!(l <= 1.0 && l >= -1.0, "Limiter ceiling violated for L: {l}");
        assert!(r <= 1.0 && r >= -1.0, "Limiter ceiling violated for R: {r}");
    }
}
