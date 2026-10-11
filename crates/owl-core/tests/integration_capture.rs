//! Integration tests for OpenWaveLink Core

use owl_core::{enumerate_audio_sessions, AudioMeterState, ProcessAudioCapture};
use owl_ipc::MeterSample;

#[test]
fn test_ipc_structures_alignment() {
    assert_eq!(std::mem::size_of::<MeterSample>(), 20);
    assert_eq!(std::mem::align_of::<MeterSample>(), 4);

    let sample = MeterSample::new(0.8, 0.6, 0.5, 0.4, false);
    assert!((sample.peak_l_f32() - 0.8).abs() < 1e-6);
    assert!((sample.peak_r_f32() - 0.6).abs() < 1e-6);
    assert!((sample.rms_l_f32() - 0.5).abs() < 1e-6);
    assert!((sample.rms_r_f32() - 0.4).abs() < 1e-6);
    assert!(!sample.is_clipping());
}

#[test]
fn test_meter_state_realtime() {
    let meter = AudioMeterState::new();
    meter.update(0.9, 0.85, 0.6, 0.55, true);

    let reading = meter.read();
    assert_eq!(reading.is_clipping, true);
    assert!((reading.peak_l - 0.9).abs() < 1e-6);
    assert!((reading.peak_r - 0.85).abs() < 1e-6);
    assert!((reading.rms_l - 0.6).abs() < 1e-6);
    assert!((reading.rms_r - 0.55).abs() < 1e-6);

    meter.reset();
    let reset_reading = meter.read();
    assert_eq!(reset_reading.peak_l, 0.0);
    assert_eq!(reset_reading.is_clipping, false);
}

#[test]
fn test_loopback_capture_lifecycle() {
    let current_pid = std::process::id();
    let mut capture = ProcessAudioCapture::new(current_pid);

    let start_res = capture.start();
    assert!(start_res.is_ok(), "Failed to start capture: {:?}", start_res.err());
    assert!(capture.is_running());
    assert!(capture.format.is_some());

    let fmt = capture.format.unwrap();
    assert!(fmt.channels >= 1);
    assert!(fmt.sample_rate >= 44100);

    // Read meter during execution
    let meter = capture.meter();
    let reading = meter.read();
    assert!(reading.peak_l >= 0.0);

    // Clean shutdown
    capture.stop();
    assert!(!capture.is_running());
}

#[test]
fn test_session_enumeration() {
    let sessions = enumerate_audio_sessions().expect("Session enumeration failed");
    println!("Enumerated {} sessions", sessions.len());
    for s in sessions {
        assert!(s.name.len() > 0);
    }
}
