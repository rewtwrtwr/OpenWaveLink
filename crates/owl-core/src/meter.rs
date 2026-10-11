//! Real-Time Lock-Free Audio Metering (RMS & Peak)

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use owl_ipc::MeterSample;

/// Snapshot of audio levels (linear and dB)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterReading {
    pub peak_l: f32,
    pub peak_r: f32,
    pub rms_l: f32,
    pub rms_r: f32,
    pub is_clipping: bool,
}

impl MeterReading {
    #[inline]
    pub fn linear_to_db(linear: f32) -> f32 {
        if linear <= 0.000001 {
            -96.0
        } else {
            (20.0 * linear.log10()).clamp(-96.0, 6.0)
        }
    }

    #[inline]
    pub fn peak_l_db(&self) -> f32 {
        Self::linear_to_db(self.peak_l)
    }

    #[inline]
    pub fn peak_r_db(&self) -> f32 {
        Self::linear_to_db(self.peak_r)
    }

    #[inline]
    pub fn rms_l_db(&self) -> f32 {
        Self::linear_to_db(self.rms_l)
    }

    #[inline]
    pub fn rms_r_db(&self) -> f32 {
        Self::linear_to_db(self.rms_r)
    }

    #[inline]
    pub fn to_ipc_sample(&self) -> MeterSample {
        MeterSample::new(self.peak_l, self.peak_r, self.rms_l, self.rms_r, self.is_clipping)
    }
}

/// Thread-safe Lock-Free container for real-time audio meters.
/// Written atomically from the audio thread, read from UI or IPC.
pub struct AudioMeterState {
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    rms_l: AtomicU32,
    rms_r: AtomicU32,
    clip_flag: AtomicBool,
    sequence: AtomicU64,
}

impl Default for AudioMeterState {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioMeterState {
    pub fn new() -> Self {
        Self {
            peak_l: AtomicU32::new(0.0f32.to_bits()),
            peak_r: AtomicU32::new(0.0f32.to_bits()),
            rms_l: AtomicU32::new(0.0f32.to_bits()),
            rms_r: AtomicU32::new(0.0f32.to_bits()),
            clip_flag: AtomicBool::new(false),
            sequence: AtomicU64::new(0),
        }
    }

    /// Atomically update meter levels (called from audio callback thread).
    /// Zero heap allocations, zero locks.
    #[inline(always)]
    pub fn update(&self, peak_l: f32, peak_r: f32, rms_l: f32, rms_r: f32, clip: bool) {
        self.peak_l.store(peak_l.to_bits(), Ordering::Release);
        self.peak_r.store(peak_r.to_bits(), Ordering::Release);
        self.rms_l.store(rms_l.to_bits(), Ordering::Release);
        self.rms_r.store(rms_r.to_bits(), Ordering::Release);
        self.clip_flag.store(clip, Ordering::Release);
        self.sequence.fetch_add(1, Ordering::Relaxed);
    }

    /// Read current meter levels.
    #[inline]
    pub fn read(&self) -> MeterReading {
        MeterReading {
            peak_l: f32::from_bits(self.peak_l.load(Ordering::Acquire)),
            peak_r: f32::from_bits(self.peak_r.load(Ordering::Acquire)),
            rms_l: f32::from_bits(self.rms_l.load(Ordering::Acquire)),
            rms_r: f32::from_bits(self.rms_r.load(Ordering::Acquire)),
            is_clipping: self.clip_flag.load(Ordering::Acquire),
        }
    }

    /// Reset meters to silence
    pub fn reset(&self) {
        self.update(0.0, 0.0, 0.0, 0.0, false);
    }
}
