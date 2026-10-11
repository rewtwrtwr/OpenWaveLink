//! 32-bit Float Routing Matrix Engine
//!
//! N Input Channels x M Output Mix Buses cross-point summing matrix.
//!
//! Complies with Hard Real-Time Audio discipline:
//! - 32-bit IEEE 754 Float processing
//! - Zero-allocation during block processing
//! - Lock-free metering and parameter updates
//! - Lookahead Clipguard on mic inputs
//! - True-Peak Brickwall Limiting on all output buses

pub mod bus;
pub mod channel;

pub use bus::{MixBus, MixBusId, NUM_MIX_BUSES};
pub use channel::{InputChannelType, MatrixInputChannel, MatrixSendNode};

/// Commands sent from the Control Plane to the Real-Time Audio Core
#[derive(Debug, Clone, PartialEq)]
pub enum MatrixCommand {
    SetChannelGain { channel_idx: usize, slider: f32 },
    SetChannelMute { channel_idx: usize, muted: bool },
    SetSendGain { channel_idx: usize, mix_idx: usize, slider: f32 },
    SetSendMute { channel_idx: usize, mix_idx: usize, muted: bool },
    SetMixGain { mix_idx: usize, slider: f32 },
    SetMixMute { mix_idx: usize, muted: bool },
    AssignProcess { channel_idx: usize, pid: u32 },
    SetVst3Bypass { channel_idx: usize, slot_idx: usize, bypassed: bool },
    SetVst3DryWet { channel_idx: usize, slot_idx: usize, dry_wet: f32 },
    ClearVst3Slot { channel_idx: usize, slot_idx: usize },
    LoadVst3Plugin { channel_idx: usize, slot_idx: usize, plugin_id: String },
}

/// 32-bit Float Routing Matrix Engine
pub struct RoutingMatrix {
    sample_rate: u32,
    channels: Vec<MatrixInputChannel>,
    buses: [MixBus; NUM_MIX_BUSES],
    command_consumer: Option<rtrb::Consumer<MatrixCommand>>,
}

impl RoutingMatrix {
    /// Create a new Routing Matrix for the given sample rate (e.g. 48000 Hz)
    pub fn new(sample_rate: u32) -> Self {
        let sr = if sample_rate == 0 { 48000 } else { sample_rate };
        let buses = [
            MixBus::new(MixBusId::Stream, sr),
            MixBus::new(MixBusId::Monitor, sr),
            MixBus::new(MixBusId::Vod, sr),
            MixBus::new(MixBusId::Discord, sr),
            MixBus::new(MixBusId::Aux, sr),
        ];

        Self {
            sample_rate: sr,
            // Pre-reserve the full channel capacity so that `add_channel` — which is also
            // reachable from the real-time thread via `EngineEvent::AddChannel` — can never
            // reallocate the channel table. `Vec::push` only allocates when `len == capacity`.
            channels: Vec::with_capacity(owl_ipc::MAX_CHANNELS),
            buses,
            command_consumer: None,
        }
    }

    /// Attach a lock-free command queue to this matrix. Returns the Producer handle for the Control Plane.
    pub fn attach_command_queue(&mut self, capacity: usize) -> rtrb::Producer<MatrixCommand> {
        let (prod, cons) = rtrb::RingBuffer::new(capacity);
        self.command_consumer = Some(cons);
        prod
    }

    /// Set an existing rtrb::Consumer for MatrixCommands.
    pub fn set_command_consumer(&mut self, consumer: rtrb::Consumer<MatrixCommand>) {
        self.command_consumer = Some(consumer);
    }

    /// Helper to look up a channel: **ID-first, index-fallback**.
    ///
    /// Channels are identified by their stable `id` (assigned by the daemon). Once channels can
    /// be removed at runtime, a slice index is no longer a stable identifier, so the id must win.
    /// The positional fallback is kept for legacy callers that pass a 0-based index.
    fn find_channel_mut(&mut self, key: usize) -> Option<&mut MatrixInputChannel> {
        if let Some(pos) = self.channels.iter().position(|c| c.id == key as u32) {
            self.channels.get_mut(pos)
        } else {
            self.channels.get_mut(key) // legacy index fallback
        }
    }

    /// Apply a single command to the matrix parameters
    pub fn apply_command(&mut self, cmd: MatrixCommand) {
        match cmd {
            MatrixCommand::SetChannelGain { channel_idx, slider } => {
                if let Some(ch) = self.find_channel_mut(channel_idx) {
                    ch.set_channel_fader_slider(slider);
                }
            }
            MatrixCommand::SetChannelMute { channel_idx, muted } => {
                if let Some(ch) = self.find_channel_mut(channel_idx) {
                    ch.set_channel_muted(muted);
                }
            }
            MatrixCommand::SetSendGain { channel_idx, mix_idx, slider } => {
                if let Some(ch) = self.find_channel_mut(channel_idx)
                    && mix_idx < NUM_MIX_BUSES {
                    ch.sends[mix_idx].set_gain_slider(slider);
                }
            }
            MatrixCommand::SetSendMute { channel_idx, mix_idx, muted } => {
                if let Some(ch) = self.find_channel_mut(channel_idx)
                    && mix_idx < NUM_MIX_BUSES {
                    ch.sends[mix_idx].set_muted(muted);
                }
            }
            MatrixCommand::SetMixGain { mix_idx, slider } => {
                if mix_idx < NUM_MIX_BUSES {
                    self.buses[mix_idx].set_volume_slider(slider);
                }
            }
            MatrixCommand::SetMixMute { mix_idx, muted } => {
                if mix_idx < NUM_MIX_BUSES {
                    self.buses[mix_idx].set_muted(muted);
                }
            }
            MatrixCommand::AssignProcess { channel_idx, pid } => {
                if let Some(ch) = self.find_channel_mut(channel_idx) {
                    ch.channel_type = InputChannelType::SoftwareProcess { pid };
                }
            }
            MatrixCommand::SetVst3Bypass { channel_idx, slot_idx, bypassed } => {
                if let Some(ch) = self.find_channel_mut(channel_idx)
                    && slot_idx < ch.vst3_rack.slots.len() {
                    ch.vst3_rack.slots[slot_idx].set_bypassed(bypassed);
                }
            }
            MatrixCommand::SetVst3DryWet { channel_idx, slot_idx, dry_wet } => {
                if let Some(ch) = self.find_channel_mut(channel_idx)
                    && slot_idx < ch.vst3_rack.slots.len() {
                    ch.vst3_rack.slots[slot_idx].set_dry_wet(dry_wet);
                }
            }
            MatrixCommand::ClearVst3Slot { channel_idx, slot_idx } => {
                if let Some(ch) = self.find_channel_mut(channel_idx)
                    && slot_idx < ch.vst3_rack.slots.len() {
                    ch.vst3_rack.slots[slot_idx].clear();
                }
            }
            MatrixCommand::LoadVst3Plugin { channel_idx, slot_idx, plugin_id } => {
                let sr = self.sample_rate as f64;
                if let Some(ch) = self.find_channel_mut(channel_idx)
                    && slot_idx < ch.vst3_rack.slots.len() {
                    let available = crate::dsp::vst3::scan_vst3_plugins();
                    if let Some(info) = available.into_iter().find(|p| p.id == plugin_id || p.name == plugin_id) {
                        ch.vst3_rack.slots[slot_idx].load_plugin(info, sr, 1024);
                    }
                }
            }
        }
    }

    /// Drain and apply all pending commands from the lock-free queue.
    ///
    /// Real-Time Safety: Zero-allocation, non-blocking.
    pub fn process_commands(&mut self) {
        if let Some(mut consumer) = self.command_consumer.take() {
            while let Ok(cmd) = consumer.pop() {
                self.apply_command(cmd);
            }
            self.command_consumer = Some(consumer);
        }
    }

    /// Collect current meter readings into pre-allocated slices for Shared Memory Telemetry.
    ///
    /// Real-Time Safety: Zero-Allocation.
    pub fn write_telemetry_samples(
        &self,
        out_channels: &mut [owl_ipc::MeterSample],
        out_mixes: &mut [owl_ipc::MeterSample],
    ) {
        let ch_count = self.channels.len().min(out_channels.len());
        for (i, slot) in out_channels.iter_mut().enumerate().take(ch_count) {
            let r = self.channels[i].meter.read();
            *slot = owl_ipc::MeterSample::new(r.peak_l, r.peak_r, r.rms_l, r.rms_r, r.is_clipping);
        }

        let mix_count = NUM_MIX_BUSES.min(out_mixes.len());
        for (i, slot) in out_mixes.iter_mut().enumerate().take(mix_count) {
            let r = self.buses[i].meter.read();
            *slot = owl_ipc::MeterSample::new(r.peak_l, r.peak_r, r.rms_l, r.rms_r, r.is_clipping);
        }
    }

    /// Add an input channel to the matrix. Returns channel ID.
    ///
    /// Real-Time Safety: allocation-free as long as the channel count stays within
    /// `owl_ipc::MAX_CHANNELS` (the capacity is reserved up front in `new`).
    pub fn add_channel(&mut self, channel: MatrixInputChannel) -> u32 {
        let id = channel.id;
        self.channels.push(channel);
        id
    }

    /// Remove a channel from the matrix by its stable ID. Returns `true` if it was removed.
    ///
    /// Real-Time Safety: allocation-free (`Vec::remove` shifts in place, never allocates).
    pub fn remove_channel(&mut self, channel_id: u32) -> bool {
        if let Some(pos) = self.channels.iter().position(|c| c.id == channel_id) {
            self.channels.remove(pos);
            true
        } else {
            false
        }
    }

    /// IDs of all currently registered channels, in matrix order.
    pub fn channel_ids(&self) -> Vec<u32> {
        self.channels.iter().map(|c| c.id).collect()
    }

    /// Number of registered input channels.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get channel reference by ID.
    pub fn channel(&self, id: u32) -> Option<&MatrixInputChannel> {
        self.channels.iter().find(|c| c.id == id)
    }

    /// Get mutable channel reference by ID.
    pub fn channel_mut(&mut self, id: u32) -> Option<&mut MatrixInputChannel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    /// Access mix bus by ID.
    pub fn bus(&self, id: MixBusId) -> &MixBus {
        &self.buses[id.index()]
    }

    /// Access mutable mix bus by ID.
    pub fn bus_mut(&mut self, id: MixBusId) -> &mut MixBus {
        &mut self.buses[id.index()]
    }

    /// Process a block of stereo audio samples across the matrix.
    ///
    /// # Parameters:
    /// - `inputs`: list of `(channel_id, left_input, right_input)` slices.
    /// - `outputs_interleaved`: array of 5 mutable interleaved stereo slices (`[L0, R0, L1, R1, ...]`),
    ///   one for each mix bus (`MixBusId::Stream`, `Monitor`, etc.).
    ///
    /// Real-Time Safety: Zero-Allocation.
    pub fn process_interleaved(
        &mut self,
        inputs: &mut [(u32, &mut [f32], &mut [f32])],
        outputs_interleaved: &mut [&mut [f32]; NUM_MIX_BUSES],
    ) {
        // 0. Drain and execute pending commands from Control Plane
        self.process_commands();

        let frame_count = outputs_interleaved[0].len() / 2;

        // 1. Clear all output mix buffers to silence
        for bus_buf in outputs_interleaved.iter_mut() {
            bus_buf.fill(0.0);
        }

        // 2. Process each input channel and sum into target mix buses
        for (ch_id, in_l, in_r) in inputs.iter_mut() {
            self.process_channel_into(*ch_id, in_l, in_r, outputs_interleaved);
        }

        // 3. Process master section for each mix bus: Master Fader -> Master Limiter -> Bus Meter
        self.finalize_buses(frame_count, outputs_interleaved);
    }

    /// Process ONE input channel into the mix buses.
    ///
    /// Order of operations: Clipguard (if present) -> VST3 rack -> per-sample channel fader
    /// + send gain -> sum into buses -> channel meter update.
    ///
    /// `frames` actually used = `min(in_l.len(), in_r.len(), outputs_interleaved[0].len() / 2)`.
    /// The output buffers are NOT cleared — the caller owns the clear/sum/finalize order.
    ///
    /// Real-Time Safety: Zero-Allocation.
    pub fn process_channel_into(
        &mut self,
        channel_id: u32,
        in_l: &mut [f32],
        in_r: &mut [f32],
        outputs_interleaved: &mut [&mut [f32]; NUM_MIX_BUSES],
    ) {
        let frame_count = outputs_interleaved[0].len() / 2;
        let frames = frame_count.min(in_l.len()).min(in_r.len());
        if frames == 0 {
            return;
        }

        let Some(ch) = self.channels.iter_mut().find(|c| c.id == channel_id) else {
            return;
        };

        // Apply OpenClipguard if present (e.g. on microphone channel)
        if let Some(ref mut clipguard) = ch.clipguard {
            clipguard.process_stereo(&mut in_l[..frames], &mut in_r[..frames]);
        }

        // Apply VST3 Insert Rack with PDC and Crash Guard
        ch.vst3_rack.process(&mut in_l[..frames], &mut in_r[..frames]);

        // Calculate channel metering stats
        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        let mut sum_sq_l = 0.0f64;
        let mut sum_sq_r = 0.0f64;

        for n in 0..frames {
            let sample_l = in_l[n];
            let sample_r = in_r[n];

            let abs_l = sample_l.abs();
            let abs_r = sample_r.abs();
            if abs_l > peak_l {
                peak_l = abs_l;
            }
            if abs_r > peak_r {
                peak_r = abs_r;
            }
            sum_sq_l += (sample_l * sample_l) as f64;
            sum_sq_r += (sample_r * sample_r) as f64;

            // Channel level fader
            let ch_gain = if ch.is_channel_muted {
                0.0
            } else {
                ch.channel_fader.next_sample()
            };

            // Send to all 5 buses
            for (bus_idx, bus_out) in outputs_interleaved.iter_mut().enumerate() {
                let send = &mut ch.sends[bus_idx];
                
                // Fix Bug 3: Read gain smoother every frame regardless of mute state
                let send_node_gain = send.gain_smoother.next_sample();
                
                let eff_gain = if send.is_muted {
                    0.0
                } else if send.is_pre_fader {
                    send_node_gain
                } else {
                    send_node_gain * ch_gain
                };

                let out_idx = n * 2;
                bus_out[out_idx] += sample_l * eff_gain;
                bus_out[out_idx + 1] += sample_r * eff_gain;
            }
        }

        let rms_l = (sum_sq_l / (frames as f64)).sqrt() as f32;
        let rms_r = (sum_sq_r / (frames as f64)).sqrt() as f32;
        let clip = peak_l >= 0.999 || peak_r >= 0.999;
        ch.meter.update(peak_l, peak_r, rms_l, rms_r, clip);
    }

    /// Master section for all buses: master fader (0 when muted) -> brickwall limiter -> bus meter.
    ///
    /// `frame_count` is clamped per bus to that bus's own buffer length, so a short buffer can
    /// never cause an out-of-bounds write.
    ///
    /// Real-Time Safety: Zero-Allocation.
    pub fn finalize_buses(
        &mut self,
        frame_count: usize,
        outputs_interleaved: &mut [&mut [f32]; NUM_MIX_BUSES],
    ) {
        for (bus_idx, bus_buf) in outputs_interleaved.iter_mut().enumerate() {
            let bus = &mut self.buses[bus_idx];
            let frames = frame_count.min(bus_buf.len() / 2);
            if frames == 0 {
                continue;
            }

            let mut peak_l = 0.0f32;
            let mut peak_r = 0.0f32;
            let mut sum_sq_l = 0.0f64;
            let mut sum_sq_r = 0.0f64;

            for n in 0..frames {
                let l_idx = n * 2;
                let r_idx = l_idx + 1;

                let master_gain = if bus.is_muted {
                    0.0
                } else {
                    bus.master_fader.next_sample()
                };

                let mut l = bus_buf[l_idx] * master_gain;
                let mut r = bus_buf[r_idx] * master_gain;

                // Master Brickwall Limiter
                bus.limiter.process_frame(&mut l, &mut r);

                let abs_l = l.abs();
                let abs_r = r.abs();
                if abs_l > peak_l {
                    peak_l = abs_l;
                }
                if abs_r > peak_r {
                    peak_r = abs_r;
                }
                sum_sq_l += (l * l) as f64;
                sum_sq_r += (r * r) as f64;

                bus_buf[l_idx] = l;
                bus_buf[r_idx] = r;
            }

            let rms_l = (sum_sq_l / (frames as f64)).sqrt() as f32;
            let rms_r = (sum_sq_r / (frames as f64)).sqrt() as f32;
            let clip = peak_l >= 0.999 || peak_r >= 0.999;
            bus.meter.update(peak_l, peak_r, rms_l, rms_r, clip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matrix_isolation() {
        let sample_rate = 48000;
        let mut matrix = RoutingMatrix::new(sample_rate);

        // Add Channel 1: Music
        let mut ch1 = MatrixInputChannel::new(
            1,
            "Music Player",
            InputChannelType::SoftwareProcess { pid: 1234 },
            sample_rate,
        );
        // Route Music ONLY to Stream Mix (Bus 0), MUTE or 0.0 on Monitor Mix (Bus 1)
        ch1.send_mut(MixBusId::Stream).set_gain_linear(1.0);
        ch1.send_mut(MixBusId::Stream).gain_smoother.reset(1.0);
        ch1.send_mut(MixBusId::Monitor).set_muted(true);
        ch1.send_mut(MixBusId::Monitor).gain_smoother.reset(0.0);
        ch1.send_mut(MixBusId::Vod).set_muted(true);
        ch1.send_mut(MixBusId::Discord).set_muted(true);
        ch1.send_mut(MixBusId::Aux).set_muted(true);
        ch1.channel_fader.reset(1.0);
        matrix.add_channel(ch1);

        // Add Channel 2: Microphone
        let mut ch2 = MatrixInputChannel::new(2, "Mic", InputChannelType::Microphone, sample_rate);
        // Route Mic ONLY to Monitor Mix (Bus 1), MUTE on Stream Mix (Bus 0)
        ch2.send_mut(MixBusId::Stream).set_muted(true);
        ch2.send_mut(MixBusId::Stream).gain_smoother.reset(0.0);
        ch2.send_mut(MixBusId::Monitor).set_gain_linear(1.0);
        ch2.send_mut(MixBusId::Monitor).gain_smoother.reset(1.0);
        ch2.send_mut(MixBusId::Vod).set_muted(true);
        ch2.send_mut(MixBusId::Discord).set_muted(true);
        ch2.send_mut(MixBusId::Aux).set_muted(true);
        ch2.channel_fader.reset(1.0);
        matrix.add_channel(ch2);

        // Prepare test audio blocks (256 frames = 512 interleaved samples)
        let frames = 256;
        let mut music_l = vec![0.5f32; frames];
        let mut music_r = vec![0.5f32; frames];

        let mut mic_l = vec![0.25f32; frames];
        let mut mic_r = vec![0.25f32; frames];

        let mut out_stream = vec![0.0f32; frames * 2];
        let mut out_monitor = vec![0.0f32; frames * 2];
        let mut out_vod = vec![0.0f32; frames * 2];
        let mut out_discord = vec![0.0f32; frames * 2];
        let mut out_aux = vec![0.0f32; frames * 2];

        {
            let mut inputs = [
                (1, music_l.as_mut_slice(), music_r.as_mut_slice()),
                (2, mic_l.as_mut_slice(), mic_r.as_mut_slice()),
            ];

            let mut outputs = [
                out_monitor.as_mut_slice(),
                out_stream.as_mut_slice(),
                out_vod.as_mut_slice(),
                out_discord.as_mut_slice(),
                out_aux.as_mut_slice(),
            ];

            matrix.process_interleaved(&mut inputs, &mut outputs);
        }

        // Verification 1: Stream mix must contain Music (0.5), and NO Mic (0.0)
        let stream_sample = out_stream[0];
        assert!(
            (stream_sample - 0.5).abs() < 0.05,
            "Stream mix should have music level ~0.5, got {stream_sample}"
        );

        // Verification 2: Monitor mix must contain Mic (0.25, delayed through clipguard), and NO Music (0.0)
        // Since Mic has clipguard lookahead buffer (72 samples), first 72 samples of mic are 0.0, then 0.25
        let monitor_sample_start = out_monitor[0];
        assert_eq!(
            monitor_sample_start, 0.0,
            "Monitor mix initial samples should be 0.0 (delayed mic), music must NOT bleed in"
        );

        let delay_frame = 80;
        let monitor_sample_delayed = out_monitor[delay_frame * 2];
        assert!(
            (monitor_sample_delayed - 0.25).abs() < 0.05,
            "Monitor mix should contain delayed mic ~0.25, got {monitor_sample_delayed}"
        );

        // Verification 3: VOD, Discord, and Aux must be absolute silence (0.0)
        assert!(out_vod.iter().all(|&s| s == 0.0), "VOD bus must be silent");
        assert!(out_discord.iter().all(|&s| s == 0.0), "Discord bus must be silent");
        assert!(out_aux.iter().all(|&s| s == 0.0), "Aux bus must be silent");
    }

    #[test]
    fn test_matrix_commands_queue() {
        let sample_rate = 48000;
        let mut matrix = RoutingMatrix::new(sample_rate);

        let ch1 = MatrixInputChannel::new(
            1,
            "App Channel",
            InputChannelType::SoftwareProcess { pid: 100 },
            sample_rate,
        );
        matrix.add_channel(ch1);

        let mut cmd_producer = matrix.attach_command_queue(32);

        // Send a batch of commands
        cmd_producer
            .push(MatrixCommand::SetChannelMute {
                channel_idx: 0,
                muted: true,
            })
            .expect("push command");
        cmd_producer
            .push(MatrixCommand::SetChannelGain {
                channel_idx: 0,
                slider: 0.85,
            })
            .expect("push command");
        cmd_producer
            .push(MatrixCommand::SetSendGain {
                channel_idx: 0,
                mix_idx: 0,
                slider: 0.70,
            })
            .expect("push command");
        cmd_producer
            .push(MatrixCommand::SetMixGain {
                mix_idx: 0,
                slider: 0.90,
            })
            .expect("push command");
        cmd_producer
            .push(MatrixCommand::SetMixMute {
                mix_idx: 1,
                muted: true,
            })
            .expect("push command");
        cmd_producer
            .push(MatrixCommand::AssignProcess {
                channel_idx: 0,
                pid: 9999,
            })
            .expect("push command");

        // Before process_interleaved, commands are pending
        assert_eq!(matrix.channel(1).unwrap().is_channel_muted, false);

        // Run process cycle
        let frames = 64;
        let mut in_l = vec![0.0f32; frames];
        let mut in_r = vec![0.0f32; frames];
        let mut out0 = vec![0.0f32; frames * 2];
        let mut out1 = vec![0.0f32; frames * 2];
        let mut out2 = vec![0.0f32; frames * 2];
        let mut out3 = vec![0.0f32; frames * 2];
        let mut out4 = vec![0.0f32; frames * 2];

        {
            let mut inputs = [(1, in_l.as_mut_slice(), in_r.as_mut_slice())];
            let mut outputs = [
                out0.as_mut_slice(),
                out1.as_mut_slice(),
                out2.as_mut_slice(),
                out3.as_mut_slice(),
                out4.as_mut_slice(),
            ];
            matrix.process_interleaved(&mut inputs, &mut outputs);
        }

        // Verify applied commands
        let ch = matrix.channel(1).unwrap();
        assert_eq!(ch.is_channel_muted, true, "Channel should be muted");
        assert_eq!(
            ch.channel_type,
            InputChannelType::SoftwareProcess { pid: 9999 },
            "PID should be 9999"
        );
        assert!(
            (ch.channel_fader.target() - crate::dsp::slider_to_gain(0.85)).abs() < 1e-4,
            "Channel fader target should match slider 0.85"
        );
        assert!(
            (ch.sends[0].gain_smoother.target() - crate::dsp::slider_to_gain(0.70)).abs() < 1e-4,
            "Send gain target should match slider 0.70"
        );

        let bus0 = matrix.bus(MixBusId::Monitor);
        assert!(
            (bus0.master_fader.target() - crate::dsp::slider_to_gain(0.90)).abs() < 1e-4,
            "Mix 0 fader target should match slider 0.90"
        );

        let bus1 = matrix.bus(MixBusId::Stream);
        assert_eq!(bus1.is_muted, true, "Mix 1 should be muted");
    }
}



