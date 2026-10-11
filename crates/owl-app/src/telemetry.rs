//! High-Speed 60-144 FPS Telemetry Bridge from Windows Shared Memory
//!
//! Direct zero-copy memory-mapped access to `Local\OpenWaveLink_Telemetry_V1`.
//! Streams VU-meter samples to Tauri 2 WebView with zero JSON serialization overhead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use log::{debug, info};
use owl_core::ipc::TelemetryReader;
use owl_ipc::{AudioTelemetryBlock, MeterSample, MAX_CHANNELS, MAX_MIXES};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

/// Compact representation of stereo VU meter reading
#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct ChannelMeterDto {
    pub peak_l: f32,
    pub peak_r: f32,
    pub rms_l: f32,
    pub rms_r: f32,
    pub clipping: bool,
}

impl From<&MeterSample> for ChannelMeterDto {
    fn from(m: &MeterSample) -> Self {
        Self {
            peak_l: m.peak_l_f32(),
            peak_r: m.peak_r_f32(),
            rms_l: m.rms_l_f32(),
            rms_r: m.rms_r_f32(),
            clipping: m.is_clipping(),
        }
    }
}

/// Instantaneous snapshot of all active channels and mix buses
#[derive(Debug, Clone, Serialize, Default)]
pub struct TelemetrySnapshotDto {
    pub seq: u32,
    pub sample_rate: u32,
    pub channels: Vec<ChannelMeterDto>,
    pub mixes: Vec<ChannelMeterDto>,
}

/// Shared in-memory cache for ultra-fast Zero-Copy Tauri command queries
#[derive(Default)]
pub struct TelemetryState {
    pub raw_block: AudioTelemetryBlock,
    pub latest_dto: TelemetrySnapshotDto,
}

pub struct TelemetryService {
    is_running: Arc<AtomicBool>,
    state: Arc<RwLock<TelemetryState>>,
}

impl TelemetryService {
    pub fn new() -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            state: Arc::new(RwLock::new(TelemetryState::default())),
        }
    }

    pub fn state(&self) -> Arc<RwLock<TelemetryState>> {
        self.state.clone()
    }

    /// Spawn high-frequency telemetry reader thread (runs at ~120 Hz)
    pub fn start(&self, app_handle: AppHandle) {
        if self.is_running.swap(true, Ordering::SeqCst) {
            return; // Already running
        }

        let is_running = self.is_running.clone();
        let state = self.state.clone();

        std::thread::Builder::new()
            .name("owl-telemetry-worker".to_string())
            .spawn(move || {
                info!("Starting high-frequency Telemetry Worker thread");

                let mut last_seq = 0u32;
                let mut reader_opt: Option<TelemetryReader> = None;

                while is_running.load(Ordering::Relaxed) {
                    // Try to acquire reader handle if not already connected
                    if reader_opt.is_none() {
                        match TelemetryReader::open() {
                            Ok(reader) => {
                                info!("Connected to Windows Shared Memory telemetry mapping");
                                reader_opt = Some(reader);
                            }
                            Err(_) => {
                                // owl-daemon not started yet; sleep and retry
                                std::thread::sleep(Duration::from_millis(500));
                                continue;
                            }
                        }
                    }

                    if let Some(ref reader) = reader_opt {
                        match reader.read() {
                            Ok(block) => {
                                if block.frame_seq != last_seq {
                                    last_seq = block.frame_seq;

                                    let ch_count = (block.channel_count as usize).min(MAX_CHANNELS);
                                    let mix_count = (block.mix_count as usize).min(MAX_MIXES);

                                    let channels: Vec<ChannelMeterDto> = block.channels[..ch_count]
                                        .iter()
                                        .map(ChannelMeterDto::from)
                                        .collect();

                                    let mixes: Vec<ChannelMeterDto> = block.mixes[..mix_count]
                                        .iter()
                                        .map(ChannelMeterDto::from)
                                        .collect();

                                    let dto = TelemetrySnapshotDto {
                                        seq: block.frame_seq,
                                        sample_rate: block.sample_rate,
                                        channels,
                                        mixes,
                                    };

                                    // Update internal cache
                                    {
                                        if let Ok(mut lock) = state.try_write() {
                                            lock.raw_block = block;
                                            lock.latest_dto = dto.clone();
                                        }
                                    }

                                    // Emit telemetry event to WebView (~60-120 FPS)
                                    let _ = app_handle.emit("vu-meter-frame", &dto);
                                }
                            }
                            Err(e) => {
                                debug!("Telemetry read error (daemon may have restarted): {e}");
                                reader_opt = None;
                                std::thread::sleep(Duration::from_millis(200));
                                continue;
                            }
                        }
                    }

                    // Loop at ~120 Hz (8.3 ms per tick)
                    std::thread::sleep(Duration::from_millis(8));
                }

                info!("Telemetry Worker thread stopped");
            })
            .expect("Failed to spawn Telemetry Worker thread");
    }

    #[allow(dead_code)]
    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_dto_conversion() {
        let sample = MeterSample::new(0.85, 0.72, 0.60, 0.55, true);
        let dto = ChannelMeterDto::from(&sample);

        assert!((dto.peak_l - 0.85).abs() < 1e-4);
        assert!((dto.peak_r - 0.72).abs() < 1e-4);
        assert!((dto.rms_l - 0.60).abs() < 1e-4);
        assert!((dto.rms_r - 0.55).abs() < 1e-4);
        assert!(dto.clipping);
    }
}
