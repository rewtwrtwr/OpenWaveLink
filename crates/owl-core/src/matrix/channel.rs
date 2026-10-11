//! Input Channel and Matrix Send Nodes
//!
//! Each input channel features:
//! - Pre-Gain / Channel Fader with zipper noise reduction
//! - Channel Mute
//! - Optional OpenClipguard 2.0 (for microphone inputs)
//! - Individual send cross-points to all 5 mix buses (with smoothed gain, mute, pre/post-fader)
//! - Real-time Audio Meter

use std::sync::Arc;
use crate::dsp::{slider_to_gain, OnePoleSmoother, OpenClipguard, Vst3Rack};
use crate::matrix::bus::{MixBusId, NUM_MIX_BUSES};
use crate::meter::AudioMeterState;

/// Type of audio input source
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputChannelType {
    SoftwareProcess { pid: u32 },
    Microphone,
    Aux,
}

/// A cross-point node connecting an input channel to an output mix bus
#[derive(Debug, Clone)]
pub struct MatrixSendNode {
    pub bus_id: MixBusId,
    pub gain_smoother: OnePoleSmoother,
    pub is_muted: bool,
    pub is_pre_fader: bool,
}

impl MatrixSendNode {
    pub fn new(bus_id: MixBusId, sample_rate: f32, initial_gain: f32) -> Self {
        Self {
            bus_id,
            gain_smoother: OnePoleSmoother::with_initial_value(sample_rate, 0.015, initial_gain),
            is_muted: false,
            is_pre_fader: false,
        }
    }

    /// Set send gain from normalized slider position [0.0, 1.0]
    pub fn set_gain_slider(&mut self, slider: f32) {
        let gain = slider_to_gain(slider);
        self.gain_smoother.set_target(gain);
    }

    /// Set raw linear gain
    pub fn set_gain_linear(&mut self, gain: f32) {
        self.gain_smoother.set_target(gain);
    }

    /// Set send mute
    pub fn set_muted(&mut self, muted: bool) {
        self.is_muted = muted;
    }

    /// Set Pre/Post fader mode
    pub fn set_pre_fader(&mut self, pre_fader: bool) {
        self.is_pre_fader = pre_fader;
    }
}

/// An audio input channel in the Routing Matrix
pub struct MatrixInputChannel {
    pub id: u32,
    pub name: String,
    pub channel_type: InputChannelType,
    pub channel_fader: OnePoleSmoother,
    pub is_channel_muted: bool,
    pub clipguard: Option<OpenClipguard>,
    pub vst3_rack: Vst3Rack,
    pub sends: [MatrixSendNode; NUM_MIX_BUSES],
    pub meter: Arc<AudioMeterState>,
}

impl MatrixInputChannel {
    pub fn new(id: u32, name: impl Into<String>, channel_type: InputChannelType, sample_rate: u32) -> Self {
        let sr = if sample_rate == 0 { 48000 } else { sample_rate };
        let sr_f = sr as f32;
        let clipguard = match channel_type {
            InputChannelType::Microphone => Some(OpenClipguard::new(sample_rate)),
            _ => None,
        };

        let sends = [
            MatrixSendNode::new(MixBusId::Stream, sr_f, 1.0),
            MatrixSendNode::new(MixBusId::Monitor, sr_f, 1.0),
            MatrixSendNode::new(MixBusId::Vod, sr_f, 1.0),
            MatrixSendNode::new(MixBusId::Discord, sr_f, 1.0),
            MatrixSendNode::new(MixBusId::Aux, sr_f, 1.0),
        ];

        Self {
            id,
            name: name.into(),
            channel_type,
            channel_fader: OnePoleSmoother::with_initial_value(sr_f, 0.015, 1.0),
            is_channel_muted: false,
            clipguard,
            vst3_rack: Vst3Rack::new(sr),
            sends,
            meter: Arc::new(AudioMeterState::new()),
        }
    }

    /// Set channel master fader from normalized slider [0.0, 1.0]
    pub fn set_channel_fader_slider(&mut self, slider: f32) {
        let gain = slider_to_gain(slider);
        self.channel_fader.set_target(gain);
    }

    /// Set channel mute
    pub fn set_channel_muted(&mut self, muted: bool) {
        self.is_channel_muted = muted;
    }

    /// Access send node for a specific mix bus
    pub fn send_mut(&mut self, bus: MixBusId) -> &mut MatrixSendNode {
        &mut self.sends[bus.index()]
    }

    /// Access send node (read-only)
    pub fn send(&self, bus: MixBusId) -> &MatrixSendNode {
        &self.sends[bus.index()]
    }
}
