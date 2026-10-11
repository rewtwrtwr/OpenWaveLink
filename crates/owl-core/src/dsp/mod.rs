//! Digital Signal Processing (DSP) Modules for OpenWaveLink
//!
//! Includes:
//! - One-Pole parameter smoothers (zipper noise reduction)
//! - OpenClipguard 2.0 (1.5 ms lookahead limiter with hyperbolic tangent saturation)
//! - Master True-Peak Brickwall Limiter

pub mod clipguard;
pub mod limiter;
pub mod smoother;
pub mod vst3;

pub use clipguard::OpenClipguard;
pub use limiter::MasterBrickwallLimiter;
pub use smoother::{slider_to_db, slider_to_gain, OnePoleSmoother};
pub use vst3::{scan_vst3_plugins, Vst3PluginHost, Vst3PluginInfo, Vst3Rack, Vst3Slot};
