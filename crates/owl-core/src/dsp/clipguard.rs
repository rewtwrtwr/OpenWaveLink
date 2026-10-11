//! OpenClipguard 2.0 - Lookahead Peak Limiter & Soft Tanh Saturation
//!
//! A zero-allocation hard real-time DSP module protecting microphone inputs from
//! overload distortion and digital clipping during loud streamer transients.
//!
//! Specification:
//! - Lookahead delay line: 1.5 ms (72 stereo frames at 48 kHz)
//! - Threshold: -0.5 dBFS (~0.94406 linear)
//! - Attack time: 0.05 ms (~2.4 samples at 48 kHz)
//! - Release time: 60.0 ms (~2880 samples at 48 kHz)
//! - Soft analog-style hyperbolic tangent (tanh) saturation curve

/// OpenClipguard 2.0 DSP Processor
#[derive(Debug, Clone)]
pub struct OpenClipguard {
    sample_rate: u32,
    threshold: f32,
    alpha_att: f32,
    alpha_rel: f32,
    gain_env: f32,
    delay_buffer: Vec<(f32, f32)>,
    delay_len: usize,
    write_idx: usize,
    is_active: bool,
}

impl OpenClipguard {
    /// Create a new OpenClipguard instance for the specified sample rate.
    ///
    /// Allocates the 1.5 ms lookahead delay ring buffer once during construction.
    pub fn new(sample_rate: u32) -> Self {
        let sr = if sample_rate == 0 { 48000 } else { sample_rate };
        let sr_f = sr as f32;

        // 1.5 ms lookahead delay
        let delay_len = ((sr_f * 0.0015).round() as usize).max(1);

        // Threshold: -0.5 dBFS = 10^(-0.5 / 20) ~ 0.94406087
        let threshold = 10.0f32.powf(-0.5 / 20.0);

        // Attack time: 0.05 ms (0.00005 s)
        let tau_att = 0.00005f32;
        let alpha_att = 1.0 - (-1.0 / (sr_f * tau_att)).exp();

        // Release time: 60.0 ms (0.060 s)
        let tau_rel = 0.060f32;
        let alpha_rel = 1.0 - (-1.0 / (sr_f * tau_rel)).exp();

        Self {
            sample_rate: sr,
            threshold,
            alpha_att,
            alpha_rel,
            gain_env: 1.0,
            delay_buffer: vec![(0.0, 0.0); delay_len],
            delay_len,
            write_idx: 0,
            is_active: true,
        }
    }

    /// Enable or disable the clipguard processor.
    pub fn set_active(&mut self, active: bool) {
        self.is_active = active;
    }

    /// Is clipguard currently active?
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Threshold in linear amplitude (~0.944).
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Lookahead delay in sample frames (e.g. 72 frames at 48 kHz).
    pub fn delay_frames(&self) -> usize {
        self.delay_len
    }

    /// Reset internal state and flush delay line.
    pub fn reset(&mut self) {
        self.gain_env = 1.0;
        self.write_idx = 0;
        for item in self.delay_buffer.iter_mut() {
            *item = (0.0, 0.0);
        }
    }

    /// Current gain reduction envelope (1.0 = no reduction, < 1.0 = limiting).
    pub fn current_gain_reduction(&self) -> f32 {
        self.gain_env
    }

    /// Soft saturation using hyperbolic tangent (tanh) for extreme transients.
    ///
    /// Preserves C1 continuity at threshold and guarantees peak never exceeds 1.0 (0 dBFS).
    #[inline(always)]
    fn soft_saturate(&self, x: f32) -> f32 {
        let abs_x = x.abs();
        if abs_x > self.threshold {
            let margin = 1.0 - self.threshold; // ~0.05594
            let excess = abs_x - self.threshold;
            let compressed = margin * (excess / margin).tanh();
            let sign = if x >= 0.0 { 1.0 } else { -1.0 };
            sign * (self.threshold + compressed)
        } else {
            x
        }
    }

    /// Process a single stereo sample pair in hard real-time (Zero-Allocation).
    #[inline]
    pub fn process_sample_pair(&mut self, left_in: f32, right_in: f32) -> (f32, f32) {
        if !self.is_active {
            return (left_in, right_in);
        }

        // 1. Peak detection on incoming sample
        let peak = left_in.abs().max(right_in.abs());
        let target_gain = if peak > self.threshold {
            self.threshold / peak
        } else {
            1.0
        };

        // 2. Dynamic attack / release envelope smoothing
        if target_gain < self.gain_env {
            // Attack phase (gain is being reduced quickly)
            self.gain_env += self.alpha_att * (target_gain - self.gain_env);
        } else {
            // Release phase (gain is recovering smoothly to 1.0)
            self.gain_env += self.alpha_rel * (target_gain - self.gain_env);
        }

        // 3. Read delayed sample from the circular buffer
        let (delayed_l, delayed_r) = self.delay_buffer[self.write_idx];

        // 4. Write incoming sample to the circular buffer
        self.delay_buffer[self.write_idx] = (left_in, right_in);
        self.write_idx += 1;
        if self.write_idx >= self.delay_len {
            self.write_idx = 0;
        }

        // 5. Apply lookahead gain reduction
        let att_l = delayed_l * self.gain_env;
        let att_r = delayed_r * self.gain_env;

        // 6. Soft analog saturation for residual peaks
        (self.soft_saturate(att_l), self.soft_saturate(att_r))
    }

    /// Process interleaved stereo slice in-place: `[L0, R0, L1, R1, ...]`
    ///
    /// Hard real-time safe: Zero-Allocation.
    pub fn process_interleaved(&mut self, samples: &mut [f32]) {
        let frames = samples.len() / 2;
        for i in 0..frames {
            let l_idx = i * 2;
            let r_idx = l_idx + 1;
            let (out_l, out_r) = self.process_sample_pair(samples[l_idx], samples[r_idx]);
            samples[l_idx] = out_l;
            samples[r_idx] = out_r;
        }
    }

    /// Process separate left and right channel slices in-place.
    ///
    /// Hard real-time safe: Zero-Allocation.
    pub fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        for i in 0..frames {
            let (out_l, out_r) = self.process_sample_pair(left[i], right[i]);
            left[i] = out_l;
            right[i] = out_r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn test_clipguard_delay_initialization() {
        let clipguard = OpenClipguard::new(48000);
        // 48000 * 0.0015 = 72 frames
        assert_eq!(clipguard.delay_frames(), 72);
        assert!((clipguard.threshold() - 0.94406).abs() < 1e-4);
    }

    #[test]
    fn test_clipguard_overload_sine_6db() {
        let sample_rate = 48000;
        let mut clipguard = OpenClipguard::new(sample_rate);

        // Generate +6 dBFS 1 kHz sine wave: amplitude ~ 1.99526 (10^(6/20))
        let amplitude = 10.0f32.powf(6.0 / 20.0);
        let freq = 1000.0;
        let num_samples = 4800; // 100 ms of audio

        let mut samples_l = Vec::with_capacity(num_samples);
        let mut samples_r = Vec::with_capacity(num_samples);

        for n in 0..num_samples {
            let t = n as f32 / sample_rate as f32;
            let val = (2.0 * PI * freq * t).sin() * amplitude;
            samples_l.push(val);
            samples_r.push(val);
        }

        clipguard.process_stereo(&mut samples_l, &mut samples_r);

        // 1. Peak amplitude must strictly NEVER exceed 1.0 (0 dBFS)
        let mut max_peak = 0.0f32;
        for &s in samples_l.iter().skip(clipguard.delay_frames()) {
            if s.abs() > max_peak {
                max_peak = s.abs();
            }
        }

        assert!(
            max_peak <= 1.0001,
            "OpenClipguard output exceeded 1.0 (0 dBFS): max peak was {max_peak}"
        );

        // 2. The waveform must not be hard-clipped (check that values smoothly approach the ceiling without rectangular flat tops)
        // With hard clipping, dozens of consecutive samples are exactly equal to 1.0.
        // With tanh saturation, values curve smoothly.
        let mut consecutive_flat_count = 0;
        let mut max_flat_streak = 0;
        let mut prev = 0.0f32;

        for &s in samples_l.iter().skip(clipguard.delay_frames()) {
            if (s - prev).abs() < 1e-6 && s.abs() > 0.94 {
                consecutive_flat_count += 1;
                max_flat_streak = max_flat_streak.max(consecutive_flat_count);
            } else {
                consecutive_flat_count = 0;
            }
            prev = s;
        }

        assert!(
            max_flat_streak < 3,
            "Waveform should not have rectangular hard clipped tops! Max flat streak was {max_flat_streak}"
        );
    }

    #[test]
    fn test_clipguard_preserves_sub_threshold_signal() {
        let sample_rate = 48000;
        let mut clipguard = OpenClipguard::new(sample_rate);

        // Small signal well below threshold: 0.5 amplitude (~ -6 dBFS)
        let num_samples = 500;
        let mut samples_l: Vec<f32> = (0..num_samples)
            .map(|n| (2.0 * PI * 440.0 * (n as f32 / 48000.0)).sin() * 0.5)
            .collect();
        let mut samples_r = samples_l.clone();

        let original = samples_l.clone();
        clipguard.process_stereo(&mut samples_l, &mut samples_r);

        let delay = clipguard.delay_frames();
        for i in delay..num_samples {
            let orig = original[i - delay];
            let processed = samples_l[i];
            assert!(
                (orig - processed).abs() < 1e-4,
                "Sub-threshold signal must be perfectly preserved after delay! Orig: {orig}, Processed: {processed}"
            );
        }
    }
}
