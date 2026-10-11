//! Output Mix Buses and Master Section
//!
//! Up to 5 independent stereo mix buses:
//! 0: Stream Mix (broadcast feed)
//! 1: Monitor Mix (personal headphones)
//! 2: VOD Track (recording stream without copyrighted music)
//! 3: Discord Feed (mix-minus for voice chat)
//! 4: Aux Mix (auxiliary routing)

use std::sync::Arc;
use crate::dsp::{slider_to_gain, MasterBrickwallLimiter, OnePoleSmoother};
use crate::meter::AudioMeterState;

pub const NUM_MIX_BUSES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum MixBusId {
    Monitor = 0,
    Stream = 1,
    Vod = 2,
    Discord = 3,
    Aux = 4,
}

impl MixBusId {
    pub const ALL: [MixBusId; NUM_MIX_BUSES] = [
        MixBusId::Monitor,
        MixBusId::Stream,
        MixBusId::Vod,
        MixBusId::Discord,
        MixBusId::Aux,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            MixBusId::Monitor => "Monitor Mix",
            MixBusId::Stream => "Stream Mix",
            MixBusId::Vod => "VOD Track",
            MixBusId::Discord => "Discord Feed",
            MixBusId::Aux => "Aux Mix",
        }
    }
}

/// Output Mix Bus with Master Fader, Brickwall Limiter, and Audio Meter
pub struct MixBus {
    pub id: MixBusId,
    pub name: &'static str,
    pub master_fader: OnePoleSmoother,
    pub is_muted: bool,
    pub limiter: MasterBrickwallLimiter,
    pub meter: Arc<AudioMeterState>,
}

impl MixBus {
    pub fn new(id: MixBusId, sample_rate: u32) -> Self {
        let sr_f = if sample_rate == 0 { 48000.0 } else { sample_rate as f32 };
        Self {
            id,
            name: id.name(),
            master_fader: OnePoleSmoother::with_initial_value(sr_f, 0.015, 1.0),
            is_muted: false,
            limiter: MasterBrickwallLimiter::new(sample_rate),
            meter: Arc::new(AudioMeterState::new()),
        }
    }

    /// Set master volume fader from normalized slider [0.0, 1.0]
    pub fn set_volume_slider(&mut self, slider: f32) {
        let gain = slider_to_gain(slider);
        self.master_fader.set_target(gain);
    }

    /// Set mute state
    pub fn set_muted(&mut self, muted: bool) {
        self.is_muted = muted;
    }
}

