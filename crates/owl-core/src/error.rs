//! Error types for OpenWaveLink Core Audio Engine

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("COM error: {0}")]
    WindowsCom(#[from] windows::core::Error),

    #[error("Failed to activate audio interface: timeout waiting for completion handler")]
    ActivationTimeout,

    #[error("Failed to activate audio interface: {0}")]
    ActivationFailed(String),

    #[error("Audio format not supported: {0}")]
    UnsupportedFormat(String),

    #[error("Capture worker thread failed or panicked")]
    WorkerThreadError,

    #[error("Win32 system error (code: {0})")]
    Win32(u32),

    #[error("Target process PID {0} does not exist or has already exited")]
    ProcessNotFound(u32),
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("COM error: {0}")]
    WindowsCom(#[from] windows::core::Error),

    #[error("Failed to initialize multimedia device enumerator")]
    EnumeratorInitFailed,

    #[error("Default audio endpoint not found")]
    DeviceNotFound,

    #[error("Failed to query session manager")]
    SessionManagerFailed,
}
