//! OpenWaveLink Core Audio Engine (owl-core)
//!
//! Windows 11 Native WASAPI Process-Loopback Audio Capture, Audio Sessions, and Metering.

pub mod apps;
pub mod cable;
pub mod capture;
pub mod devices;
pub mod dsp;
pub mod engine;
pub mod error;
pub mod format;
pub mod ipc;
pub mod matrix;
pub mod meter;
pub mod playback;
pub mod policy;
pub mod session;

pub use apps::{get_app_icon_data_url, get_installed_applications, InstalledAppDto};
pub use cable::{CableEndpoints, CableError, CableManager, VirtualCable, CABLE_SAMPLE_RATE};
pub use capture::{MicrophoneCapture, ProcessAudioCapture};
pub use devices::{get_system_audio_devices, AudioDeviceDto, SystemAudioDevicesDto};
pub use dsp::{
    scan_vst3_plugins, slider_to_db, slider_to_gain, MasterBrickwallLimiter, OnePoleSmoother,
    OpenClipguard, Vst3PluginHost, Vst3PluginInfo, Vst3Rack, Vst3Slot,
};
pub use error::{CaptureError, SessionError};
pub use engine::{AttachError, AudioEngine, EngineEvent, BLOCK_FRAMES};
pub use format::{AudioFormatInfo, SampleType};
pub use ipc::{StreamPipePublisher, StreamPipeReader, TelemetryPublisher, TelemetryReader};
pub use matrix::{
    InputChannelType, MatrixCommand, MatrixInputChannel, MatrixSendNode, MixBus, MixBusId,
    RoutingMatrix, NUM_MIX_BUSES,
};
pub use meter::{AudioMeterState, MeterReading};
pub use playback::WasapiPlayback;
pub use policy::{set_app_audio_endpoint, set_default_playback_endpoint};
pub use session::{enumerate_audio_sessions, get_active_audio_sessions, AudioProcessDto, AudioProcessInfo};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meter_math() {
        let reading = MeterReading {
            peak_l: 1.0,
            peak_r: 0.5,
            rms_l: 0.707,
            rms_r: 0.353,
            is_clipping: false,
        };

        assert!((reading.peak_l_db() - 0.0).abs() < 0.1);
        assert!((reading.peak_r_db() - (-6.02)).abs() < 0.1);
        assert!((reading.rms_l_db() - (-3.01)).abs() < 0.1);
    }

    #[test]
    fn test_enumerate_sessions() {
        let sessions = enumerate_audio_sessions();
        assert!(sessions.is_ok(), "Session enumeration should succeed: {:?}", sessions.err());
        let list = sessions.unwrap();
        println!("Found {} audio sessions", list.len());
        for s in list {
            println!("- PID {}: {} (active: {})", s.pid, s.name, s.is_active);
        }
    }
}
