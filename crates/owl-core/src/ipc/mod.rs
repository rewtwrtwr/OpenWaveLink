//! Inter-Process Communication (IPC) Modules
//!
//! Windows Shared Memory Data Plane for VU meters and OBS Studio direct audio stream.

pub mod telemetry;
pub mod stream_pipe;

pub use telemetry::{TelemetryPublisher, TelemetryReader};
pub use stream_pipe::{StreamPipePublisher, StreamPipeReader};
