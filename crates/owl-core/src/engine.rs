//! OpenWaveLink Real-Time Audio Engine
//!
//! Owns the routing matrix, a **dynamic pool of capture rings** (one per input channel),
//! the per-block output buffers and the playback producer.
//!
//! # Architecture
//!
//! The engine is the only thing the real-time thread touches. Every mutation of the engine
//! (adding/removing a channel, attaching/detaching a capture ring, swapping the output device)
//! travels through the lock-free [`EngineEvent`] queue and is applied inside
//! [`AudioEngine::process_block`]. The control plane therefore never touches the matrix.
//!
//! # Real-time discipline
//!
//! [`AudioEngine::process_block`] is **allocation-free**:
//! - fixed `[f32; BLOCK_FRAMES]` scratch buffer per slot (no `Vec` growth on the audio thread),
//! - the slot table has a fixed capacity of [`owl_ipc::MAX_CHANNELS`] (32),
//! - the matrix channel table is pre-reserved to the same capacity in `RoutingMatrix::new`,
//! - no locks, no I/O, no blocking.

use crate::matrix::{MatrixCommand, MatrixInputChannel, RoutingMatrix, NUM_MIX_BUSES};
use owl_ipc::{MeterSample, MAX_CHANNELS};

/// Frames processed per engine block (~5.3 ms @ 48 kHz).
pub const BLOCK_FRAMES: usize = 256;

/// Number of mix buses mirrored into the engine's own output buffers.
const BUSES: usize = NUM_MIX_BUSES;

/// An input channel with no capture ring attached.
const NO_CAPTURE: u32 = u32::MAX;

/// Commands sent from the Control Plane into the Real-Time Audio Core.
///
/// The engine owns the matrix, so channel lifecycle (add/remove) has to travel through this
/// same lock-free queue — `AudioEngine::add_channel` is only callable before the engine is
/// moved into the real-time thread, while `owl.addChannel` / `owl.removeChannel` arrive at
/// runtime.
pub enum EngineEvent {
    /// Register a new channel in the matrix at runtime.
    AddChannel { channel: Box<MatrixInputChannel> },
    /// Remove a channel (and its capture ring) from the matrix at runtime.
    RemoveChannel { channel_id: u32 },
    /// Attach a live capture ring (produced by `ProcessAudioCapture`) to a channel.
    AttachCapture {
        channel_id: u32,
        pid: u32,
        consumer: rtrb::Consumer<f32>,
    },
    /// Detach and drop the capture ring of a channel.
    DetachCapture { channel_id: u32 },
    /// Swap the playback producer (output device change).
    SwapOutput { producer: rtrb::Producer<f32> },
}

/// One input slot: a matrix channel plus its (optional) capture ring and scratch buffers.
///
/// The scratch buffers are fixed-size arrays so that processing a block never allocates.
struct CaptureSlot {
    channel_id: u32,
    /// PID of the captured process, or 0 when no capture is attached.
    pid: u32,
    consumer: Option<rtrb::Consumer<f32>>,
    scratch_l: [f32; BLOCK_FRAMES],
    scratch_r: [f32; BLOCK_FRAMES],
    /// Frames this slot contributed during the most recent block.
    frames: usize,
    /// A left sample read without its right partner at the end of the previous block.
    ///
    /// The ring carries an interleaved stream with no frame framing, so an odd number of
    /// samples can be available at a block boundary. Holding the orphan here keeps the
    /// L/R phase locked across blocks instead of dropping or misaligning it.
    pending_left: Option<f32>,
}

impl CaptureSlot {
    fn new(channel_id: u32) -> Self {
        Self {
            channel_id,
            pid: 0,
            consumer: None,
            scratch_l: [0.0; BLOCK_FRAMES],
            scratch_r: [0.0; BLOCK_FRAMES],
            frames: 0,
            pending_left: None,
        }
    }

    /// True when this slot has a live capture ring attached.
    #[inline]
    fn is_attached(&self) -> bool {
        self.consumer.is_some()
    }
}

/// Why a capture ring could not be attached to a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachError {
    /// No channel with that id exists in the matrix.
    NoSuchChannel,
    /// All [`owl_ipc::MAX_CHANNELS`] engine slots are occupied.
    NoFreeSlot,
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachError::NoSuchChannel => write!(f, "no such channel"),
            AttachError::NoFreeSlot => {
                write!(f, "all {} engine slots are occupied", MAX_CHANNELS)
            }
        }
    }
}

impl std::error::Error for AttachError {}

/// Real-time audio engine: matrix + dynamic capture pool + output buffers + playback feed.
pub struct AudioEngine {
    matrix: RoutingMatrix,
    /// Fixed-capacity slot table, indexed by matrix channel id.
    slots: [CaptureSlot; MAX_CHANNELS],
    /// Interleaved per-bus output buffers for the current block.
    bus_buffers: [[f32; BLOCK_FRAMES * 2]; BUSES],
    /// Interleaved Monitor Mix snapshot for the current block.
    monitor_mix: [f32; BLOCK_FRAMES * 2],
    /// Interleaved Stream Mix snapshot for the current block.
    stream_mix: [f32; BLOCK_FRAMES * 2],
    /// Interleaved processed MicFX snapshot for the current block.
    mic_fx: [f32; BLOCK_FRAMES * 2],
    /// Frames produced by the most recent `process_block`.
    frames: usize,
    /// Playback feed (Monitor Mix). Pushed to, never blocked on.
    playback_producer: Option<rtrb::Producer<f32>>,
    /// Control-plane -> real-time engine event queue.
    event_consumer: Option<rtrb::Consumer<EngineEvent>>,
}

impl AudioEngine {
    /// Create a new engine for the given sample rate (e.g. 48000 Hz).
    pub fn new(sample_rate: u32) -> Self {
        let mut slots: [CaptureSlot; MAX_CHANNELS] = std::array::from_fn(|_| CaptureSlot::new(0));
        // Ids are assigned by the caller; NO_CAPTURE marks "no capture attached yet".
        for slot in slots.iter_mut() {
            slot.channel_id = NO_CAPTURE;
        }

        Self {
            matrix: RoutingMatrix::new(sample_rate),
            slots,
            bus_buffers: [[0.0; BLOCK_FRAMES * 2]; BUSES],
            monitor_mix: [0.0; BLOCK_FRAMES * 2],
            stream_mix: [0.0; BLOCK_FRAMES * 2],
            mic_fx: [0.0; BLOCK_FRAMES * 2],
            frames: 0,
            playback_producer: None,
            event_consumer: None,
        }
    }

    /// Create the lock-free control queue; the returned `Producer` goes to the Control Plane.
    pub fn attach_engine_queue(&mut self, capacity: usize) -> rtrb::Producer<EngineEvent> {
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
        self.event_consumer = Some(consumer);
        producer
    }

    /// Create and attach the matrix's parameter queue (channel/send/mix gain & mute).
    ///
    /// Delegates to [`RoutingMatrix::attach_command_queue`]. The returned `Producer` is what the
    /// daemon's `ServerState.cmd_producer` feeds.
    pub fn attach_command_queue(&mut self, capacity: usize) -> rtrb::Producer<MatrixCommand> {
        self.matrix.attach_command_queue(capacity)
    }

    /// Register an input channel (the matrix owns it from here on). Returns the channel id.
    ///
    /// This is the pre-real-time path. At runtime use [`EngineEvent::AddChannel`] instead.
    ///
    /// The channel is always registered in the matrix; only the *slot* is capacity-limited.
    /// Use [`AudioEngine::try_add_channel`] when the caller needs to observe slot exhaustion.
    pub fn add_channel(&mut self, channel: MatrixInputChannel) -> u32 {
        let id = channel.id;
        self.matrix.add_channel(channel);
        if self.ensure_slot(id).is_none() {
            log::warn!(
                "engine: channel {id} registered in the matrix but no free capture slot \
                 ({MAX_CHANNELS} in use) — its audio will stay silent"
            );
        }
        id
    }

    /// Register a channel and report slot exhaustion instead of silently degrading.
    ///
    /// Returns [`AttachError::NoFreeSlot`] when all [`owl_ipc::MAX_CHANNELS`] slots are taken;
    /// the channel is *not* left half-registered in that case.
    pub fn try_add_channel(&mut self, channel: MatrixInputChannel) -> Result<u32, AttachError> {
        let id = channel.id;
        if self.slot_index(id).is_none() && self.free_slot().is_none() {
            return Err(AttachError::NoFreeSlot);
        }
        self.matrix.add_channel(channel);
        self.ensure_slot(id);
        Ok(id)
    }

    /// Attach a live capture ring to an existing channel. Returns `false` if no such channel.
    ///
    /// Use [`AudioEngine::try_attach_capture`] to distinguish "no such channel" from
    /// "no free slot".
    pub fn attach_capture(
        &mut self,
        channel_id: u32,
        pid: u32,
        consumer: rtrb::Consumer<f32>,
    ) -> bool {
        match self.try_attach_capture(channel_id, pid, consumer) {
            Ok(()) => true,
            Err(err) => {
                log::warn!("engine: cannot attach capture for channel {channel_id} (pid {pid}): {err}");
                false
            }
        }
    }

    /// Fallible [`AudioEngine::attach_capture`].
    ///
    /// This is the observable path: it reports *why* an attach failed instead of silently
    /// leaving the channel with a ring nobody reads.
    pub fn try_attach_capture(
        &mut self,
        channel_id: u32,
        pid: u32,
        consumer: rtrb::Consumer<f32>,
    ) -> Result<(), AttachError> {
        if self.matrix.channel(channel_id).is_none() {
            return Err(AttachError::NoSuchChannel);
        }
        let idx = match self.slot_index(channel_id) {
            Some(idx) => idx,
            None => self.ensure_slot(channel_id).ok_or(AttachError::NoFreeSlot)?,
        };
        let slot = &mut self.slots[idx];
        slot.consumer = Some(consumer);
        slot.pid = pid;
        // Reset the interleave carry: an orphan LEFT sample from a previously attached stream
        // must never leak into the new one (e.g. Chrome restarting under a new pid), which
        // would shift L/R phase by one sample.
        slot.pending_left = None;
        Ok(())
    }

    /// Detach and drop the capture ring of a channel. Returns `false` if none was attached.
    pub fn detach_capture(&mut self, channel_id: u32) -> bool {
        let Some(idx) = self.slot_index(channel_id) else {
            return false;
        };
        let slot = &mut self.slots[idx];
        let had_capture = slot.consumer.is_some();
        slot.consumer = None;
        slot.pid = 0;
        slot.frames = 0;
        slot.pending_left = None;
        had_capture
    }

    /// Remove a channel from the matrix entirely (detaches its capture first).
    pub fn remove_channel(&mut self, channel_id: u32) -> bool {
        self.detach_capture(channel_id);
        if let Some(idx) = self.slot_index(channel_id) {
            self.slots[idx].channel_id = NO_CAPTURE;
        }
        self.matrix.remove_channel(channel_id)
    }

    /// Drain engine events, pull every attached capture ring, run the matrix, store the results,
    /// and push the Monitor mix into the playback producer.
    ///
    /// Returns the number of frames processed this block (0 when nothing was available).
    ///
    /// Real-Time Safety: Zero-Allocation, no locks, no I/O.
    pub fn process_block(&mut self) -> usize {
        // 1. Drain control-plane events FIRST — they mutate the slot table.
        self.drain_events();

        // 2. Drain the matrix parameter queue (channel/send/mix gain & mute).
        self.matrix.process_commands();

        // 3. Zero the bus buffers for this block.
        for bus_buf in self.bus_buffers.iter_mut() {
            bus_buf.fill(0.0);
        }

        // 4. Pull each attached capture ring into its slot's scratch buffers.
        //    A slot that yields 0 frames contributes silence but MUST still keep its offset,
        //    so we simply skip it here and let `max_frames` stay driven by the other slots.
        let mut target_frames = BLOCK_FRAMES;
        if let Some(producer) = &self.playback_producer {
            let space = producer.slots() / 2;
            if space < target_frames {
                target_frames = space;
            }
        }
        if target_frames == 0 {
            self.frames = 0;
            return 0;
        }

        let mut max_frames = 0usize;
        for slot in self.slots.iter_mut() {
            slot.frames = 0;
            let Some(consumer) = slot.consumer.as_mut() else {
                continue;
            };

            // De-interleave straight into the scratch pair: L -> scratch_l, R -> scratch_r.
            // `pending_left` carries an orphan left sample from the previous block so the
            // stereo phase stays locked across block boundaries.
            let mut frames = 0usize;
            if let Some(left) = slot.pending_left.take() {
                match consumer.pop() {
                    Ok(right) => {
                        slot.scratch_l[0] = left;
                        slot.scratch_r[0] = right;
                        frames = 1;
                    }
                    Err(_) => {
                        // Still no partner — keep holding it for the next block.
                        slot.pending_left = Some(left);
                        continue;
                    }
                }
            }

            while frames < target_frames {
                let Ok(left) = consumer.pop() else {
                    break;
                };
                match consumer.pop() {
                    Ok(right) => {
                        slot.scratch_l[frames] = left;
                        slot.scratch_r[frames] = right;
                        frames += 1;
                    }
                    Err(_) => {
                        // Odd tail: hold the orphan left sample for the next block.
                        slot.pending_left = Some(left);
                        break;
                    }
                }
            }

            slot.frames = frames;
            if frames > max_frames {
                max_frames = frames;
            }
        }

        if max_frames == 0 {
            self.frames = 0;
            return 0;
        }

        // 5. Run the matrix over the widest slot. `process_channel_into` sums into the buses;
        //    `finalize_buses` applies the master section.
        {
            // Clear mic_fx for the current block
            self.mic_fx[..max_frames * 2].fill(0.0);

            // Destructure so the borrow checker sees five disjoint borrows instead of five
            // overlapping ones into the same array.
            let [b0, b1, b2, b3, b4] = &mut self.bus_buffers;
            let mut outputs: [&mut [f32]; BUSES] = [b0, b1, b2, b3, b4];

            for slot in self.slots.iter_mut() {
                if slot.frames == 0 {
                    continue;
                }
                let frames = slot.frames;
                let (scratch_l, scratch_r) = (&mut slot.scratch_l, &mut slot.scratch_r);

                let is_mic = self.matrix.channel(slot.channel_id)
                    .is_some_and(|ch| matches!(ch.channel_type, crate::matrix::channel::InputChannelType::Microphone));

                self.matrix.process_channel_into(
                    slot.channel_id,
                    &mut scratch_l[..frames],
                    &mut scratch_r[..frames],
                    &mut outputs,
                );

                if is_mic {
                    // Publish the processed mic audio directly to its buffer
                    for i in 0..frames {
                        self.mic_fx[i * 2] = scratch_l[i];
                        self.mic_fx[i * 2 + 1] = scratch_r[i];
                    }
                }
            }

            self.matrix.finalize_buses(max_frames, &mut outputs);
        }

        // 6. Snapshot Stream + Monitor mixes for the rest of the system.
        let used = max_frames * 2;
        self.stream_mix[..used].copy_from_slice(&self.bus_buffers[crate::MixBusId::Stream.index()][..used]);
        self.monitor_mix[..used].copy_from_slice(&self.bus_buffers[crate::MixBusId::Monitor.index()][..used]);

        // 7. Feed playback: drop on full, never block.
        if let Some(producer) = self.playback_producer.as_mut() {
            for &sample in &self.monitor_mix[..used] {
                if producer.push(sample).is_err() {
                    break; // ring full — drop the rest of this block
                }
            }
        }

        self.frames = max_frames;
        max_frames
    }

    /// Frames produced by the most recent `process_block`.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Interleaved Stream Mix for the current block, `frames()*2` samples.
    pub fn stream_mix(&self) -> &[f32] {
        &self.stream_mix[..self.frames * 2]
    }

    /// Interleaved MicFX for the current block, `frames()*2` samples.
    pub fn mic_fx(&self) -> &[f32] {
        &self.mic_fx[..self.frames * 2]
    }

    /// Interleaved Monitor Mix for the current block, `frames()*2` samples.
    pub fn monitor_mix(&self) -> &[f32] {
        &self.monitor_mix[..self.frames * 2]
    }

    /// Read-only access to the routing matrix.
    pub fn matrix(&self) -> &RoutingMatrix {
        &self.matrix
    }

    /// Mutable access to the routing matrix (control plane / setup only — never the audio thread
    /// while `process_block` is running).
    pub fn matrix_mut(&mut self) -> &mut RoutingMatrix {
        &mut self.matrix
    }

    /// True when at least one capture ring is attached.
    pub fn has_active_captures(&self) -> bool {
        self.slots.iter().any(|s| s.is_attached())
    }

    /// Channel ids that currently have a live capture ring.
    pub fn active_capture_ids(&self) -> Vec<u32> {
        self.slots
            .iter()
            .filter(|s| s.is_attached() && s.channel_id != NO_CAPTURE)
            .map(|s| s.channel_id)
            .collect()
    }

    /// PID attached to a channel's capture ring, if any.
    pub fn capture_pid(&self, channel_id: u32) -> Option<u32> {
        let idx = self.slot_index(channel_id)?;
        let slot = &self.slots[idx];
        if slot.is_attached() { Some(slot.pid) } else { None }
    }

    /// Publish VU meters into pre-allocated slices (zero-allocation).
    pub fn write_telemetry_samples(
        &self,
        out_channels: &mut [MeterSample],
        out_mixes: &mut [MeterSample],
    ) {
        self.matrix.write_telemetry_samples(out_channels, out_mixes);
    }

    // ---- internals -------------------------------------------------------------------

    /// Drain and apply all pending control-plane events. Called at the top of `process_block`.
    ///
    /// Real-Time Safety: Zero-Allocation — `EngineEvent::AddChannel` pushes into a pre-reserved
    /// matrix channel table and a fixed-capacity slot table.
    fn drain_events(&mut self) {
        let Some(mut consumer) = self.event_consumer.take() else {
            return;
        };
        while let Ok(event) = consumer.pop() {
            match event {
                EngineEvent::AddChannel { channel } => {
                    let id = channel.id;
                    self.matrix.add_channel(*channel);
                    if self.ensure_slot(id).is_none() {
                        // Surfaced, never silent: the channel exists but cannot carry audio.
                        log::warn!(
                            "engine: AddChannel {id} rejected — no free capture slot \
                             ({MAX_CHANNELS} in use); channel is silent"
                        );
                    }
                }
                EngineEvent::RemoveChannel { channel_id } => {
                    self.remove_channel(channel_id);
                }
                EngineEvent::AttachCapture {
                    channel_id,
                    pid,
                    consumer,
                } => {
                    // `try_attach_capture` is used so the event path and the direct path share
                    // one implementation, and so a failure is reported rather than swallowed.
                    if let Err(err) = self.try_attach_capture(channel_id, pid, consumer) {
                        log::warn!(
                            "engine: AttachCapture failed for channel {channel_id} (pid {pid}): {err}"
                        );
                    }
                }
                EngineEvent::DetachCapture { channel_id } => {
                    self.detach_capture(channel_id);
                }
                EngineEvent::SwapOutput { producer } => {
                    // Replacing the Option drops the previous producer, so a stale handle can
                    // never keep receiving the monitor mix.
                    self.playback_producer = Some(producer);
                }
            }
        }
        self.event_consumer = Some(consumer);
    }

    /// Find the slot table index for a channel id.
    fn slot_index(&self, channel_id: u32) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.channel_id == channel_id && s.channel_id != NO_CAPTURE)
    }

    /// Index of the first unoccupied slot, if any.
    fn free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.channel_id == NO_CAPTURE)
    }

    /// Find the slot for a channel, allocating a free entry when needed.
    ///
    /// Returns `None` only when all `MAX_CHANNELS` slots are in use. Never allocates.
    fn ensure_slot(&mut self, channel_id: u32) -> Option<usize> {
        if let Some(idx) = self.slot_index(channel_id) {
            return Some(idx);
        }
        let free = self.free_slot()?;
        self.slots[free].channel_id = channel_id;
        self.slots[free].frames = 0;
        Some(free)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::{InputChannelType, MixBusId};
    use crate::dsp::slider_to_gain;

    /// Build a channel routed to the Monitor bus only, at unity gain.
    fn monitor_channel(id: u32, sample_rate: u32) -> MatrixInputChannel {
        let mut ch = MatrixInputChannel::new(
            id,
            format!("ch{id}"),
            InputChannelType::SoftwareProcess { pid: 1000 + id },
            sample_rate,
        );
        for bus in MixBusId::ALL {
            ch.send_mut(bus).set_muted(true);
            ch.send_mut(bus).gain_smoother.reset(0.0);
        }
        ch.send_mut(MixBusId::Monitor).set_muted(false);
        ch.send_mut(MixBusId::Monitor).gain_smoother.reset(1.0);
        ch.channel_fader.reset(1.0);
        ch
    }

    /// Fill a ring with `frames` stereo frames of a DC value and attach it to `channel_id`.
    fn attach_dc(
        engine: &mut AudioEngine,
        channel_id: u32,
        dc: f32,
        frames: usize,
    ) -> rtrb::Producer<f32> {
        let (mut producer, consumer) = rtrb::RingBuffer::new(frames * 2 + 64);
        for _ in 0..frames {
            producer.push(dc).expect("push L");
            producer.push(dc).expect("push R");
        }
        assert!(
            engine.attach_capture(channel_id, 1000 + channel_id, consumer),
            "attach_capture should succeed for an existing channel"
        );
        producer
    }

    #[test]
    fn test_engine_attaches_capture_and_carries_dc_to_monitor() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        engine.add_channel(monitor_channel(1, sr));
        attach_dc(&mut engine, 1, 0.5, BLOCK_FRAMES);

        assert!(engine.has_active_captures());
        assert_eq!(engine.active_capture_ids(), vec![1]);

        let frames = engine.process_block();
        assert_eq!(frames, BLOCK_FRAMES, "a full block should be processed");

        // Channel fader + Monitor send are both at unity, so the Monitor mix carries the DC.
        let monitor = engine.monitor_mix();
        assert_eq!(monitor.len(), frames * 2);
        let expected = 0.5;
        for (i, &s) in monitor.iter().enumerate() {
            assert!(
                (s - expected).abs() < 1e-3,
                "monitor sample {i} should be ~{expected}, got {s}"
            );
        }

        // The Stream bus was muted on this channel, so it must stay silent.
        assert!(
            engine.stream_mix().iter().all(|&s| s.abs() < 1e-6),
            "stream mix must stay silent for a monitor-only channel"
        );
    }

    #[test]
    fn test_engine_applies_channel_and_send_gain() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        let mut ch = monitor_channel(1, sr);
        // Slider 0.5 == unity (0 dB); halve it to 0.25 to get a quarter of the DC.
        ch.send_mut(MixBusId::Monitor).set_gain_linear(0.25);
        ch.send_mut(MixBusId::Monitor).gain_smoother.reset(0.25);
        engine.add_channel(ch);
        attach_dc(&mut engine, 1, 1.0, BLOCK_FRAMES);

        let frames = engine.process_block();
        assert_eq!(frames, BLOCK_FRAMES);
        let monitor = engine.monitor_mix();
        let expected = 0.25;
        for (i, &s) in monitor.iter().enumerate() {
            assert!(
                (s - expected).abs() < 1e-3,
                "monitor sample {i} should be ~{expected}, got {s}"
            );
        }
    }

    #[test]
    fn test_engine_two_channels_sum_and_removal_is_isolated() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        engine.add_channel(monitor_channel(1, sr));
        engine.add_channel(monitor_channel(2, sr));

        attach_dc(&mut engine, 1, 0.25, BLOCK_FRAMES);
        attach_dc(&mut engine, 2, 0.5, BLOCK_FRAMES);

        let mut ids = engine.active_capture_ids();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2], "both channels must be attached independently");

        let frames = engine.process_block();
        assert_eq!(frames, BLOCK_FRAMES);
        let expected = 0.25 + 0.5;
        for (i, &s) in engine.monitor_mix().iter().enumerate() {
            assert!(
                (s - expected).abs() < 1e-3,
                "monitor sample {i} should sum to ~{expected}, got {s}"
            );
        }

        // Remove channel 1: channel 2 must keep working untouched.
        assert!(engine.remove_channel(1), "channel 1 should be removed");
        assert!(!engine.has_active_captures() || engine.active_capture_ids() == vec![2]);
        assert_eq!(engine.active_capture_ids(), vec![2]);
        assert!(engine.matrix().channel(1).is_none(), "channel 1 left the matrix");
        assert!(engine.matrix().channel(2).is_some(), "channel 2 stays in the matrix");

        attach_dc(&mut engine, 2, 0.5, BLOCK_FRAMES);
        let frames = engine.process_block();
        assert_eq!(frames, BLOCK_FRAMES);
        for (i, &s) in engine.monitor_mix().iter().enumerate() {
            assert!(
                (s - 0.5).abs() < 1e-3,
                "after removal monitor sample {i} should be ~0.5, got {s}"
            );
        }
    }

    #[test]
    fn test_engine_detach_stops_contribution_without_disturbing_other_slot() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        engine.add_channel(monitor_channel(1, sr));
        engine.add_channel(monitor_channel(2, sr));

        attach_dc(&mut engine, 1, 0.5, BLOCK_FRAMES);
        attach_dc(&mut engine, 2, 0.25, BLOCK_FRAMES);

        assert_eq!(engine.process_block(), BLOCK_FRAMES);
        for &s in engine.monitor_mix() {
            assert!((s - 0.75).abs() < 1e-3, "expected both channels summing, got {s}");
        }

        // Detach channel 1 -> only channel 2 remains audible.
        assert!(engine.detach_capture(1), "detach should report an attached capture");
        assert!(!engine.detach_capture(1), "second detach must report nothing attached");
        assert_eq!(engine.active_capture_ids(), vec![2]);

        attach_dc(&mut engine, 2, 0.25, BLOCK_FRAMES);
        assert_eq!(engine.process_block(), BLOCK_FRAMES);
        for (i, &s) in engine.monitor_mix().iter().enumerate() {
            assert!(
                (s - 0.25).abs() < 1e-3,
                "after detach monitor sample {i} should be ~0.25, got {s}"
            );
        }
    }

    #[test]
    fn test_engine_slot_with_no_frames_contributes_silence_and_keeps_offsets() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        engine.add_channel(monitor_channel(1, sr));
        engine.add_channel(monitor_channel(2, sr));

        // Channel 1 has an EMPTY ring; channel 2 has data. Processing must still work and the
        // empty slot must not shift channel 2's frame offsets.
        let (_empty_producer, empty_consumer) = rtrb::RingBuffer::<f32>::new(512);
        assert!(engine.attach_capture(1, 1001, empty_consumer));
        attach_dc(&mut engine, 2, 0.5, BLOCK_FRAMES);

        let frames = engine.process_block();
        assert_eq!(frames, BLOCK_FRAMES, "the non-empty slot drives the block size");
        for (i, &s) in engine.monitor_mix().iter().enumerate() {
            assert!(
                (s - 0.5).abs() < 1e-3,
                "monitor sample {i} should be exactly the non-empty slot, got {s}"
            );
        }
    }

    #[test]
    fn test_engine_process_block_with_no_captures_returns_zero() {
        let mut engine = AudioEngine::new(48000);
        engine.add_channel(monitor_channel(1, 48000));
        assert_eq!(engine.process_block(), 0);
        assert_eq!(engine.frames(), 0);
        assert!(engine.stream_mix().is_empty());
        assert!(engine.monitor_mix().is_empty());
    }

    #[test]
    fn test_engine_events_drive_runtime_channel_lifecycle() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        let mut events = engine.attach_engine_queue(64);

        // Runtime add: the control plane sends the fully-configured channel by value.
        events
            .push(EngineEvent::AddChannel {
                channel: Box::new(monitor_channel(7, sr)),
            })
            .expect("push AddChannel");

        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(BLOCK_FRAMES * 2 + 64);
        for _ in 0..BLOCK_FRAMES {
            producer.push(0.5).unwrap();
            producer.push(0.5).unwrap();
        }
        events
            .push(EngineEvent::AttachCapture {
                channel_id: 7,
                pid: 4242,
                consumer,
            })
            .expect("push AttachCapture");

        assert_eq!(engine.process_block(), BLOCK_FRAMES);
        assert!(engine.matrix().channel(7).is_some(), "AddChannel reached the matrix");
        assert_eq!(engine.capture_pid(7), Some(4242));
        for &s in engine.monitor_mix() {
            assert!((s - 0.5).abs() < 1e-3, "runtime-added channel should be audible, got {s}");
        }

        // Runtime remove: the channel leaves the matrix and stops contributing.
        events
            .push(EngineEvent::RemoveChannel { channel_id: 7 })
            .expect("push RemoveChannel");
        assert_eq!(engine.process_block(), 0);
        assert!(engine.matrix().channel(7).is_none(), "RemoveChannel reached the matrix");
        assert!(!engine.has_active_captures());
    }

    #[test]
    fn test_engine_swap_output_replaces_producer() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        let mut events = engine.attach_engine_queue(64);
        engine.add_channel(monitor_channel(1, sr));

        let (old_producer, old_consumer) = rtrb::RingBuffer::<f32>::new(4096);
        events.push(EngineEvent::SwapOutput { producer: old_producer }).unwrap();
        attach_dc(&mut engine, 1, 0.5, BLOCK_FRAMES);
        assert_eq!(engine.process_block(), BLOCK_FRAMES);
        assert_eq!(old_consumer.slots(), BLOCK_FRAMES * 2, "first producer got the mix");

        // Swap to a new output device.
        let (new_producer, mut new_consumer) = rtrb::RingBuffer::<f32>::new(4096);
        events.push(EngineEvent::SwapOutput { producer: new_producer }).unwrap();

        attach_dc(&mut engine, 1, 0.5, BLOCK_FRAMES);
        assert_eq!(engine.process_block(), BLOCK_FRAMES);

        let drained_old = old_consumer.slots();
        assert_eq!(
            drained_old,
            BLOCK_FRAMES * 2,
            "the stale producer must receive nothing after the swap"
        );
        assert_eq!(
            new_consumer.slots(),
            BLOCK_FRAMES * 2,
            "the new producer must receive the monitor mix"
        );
        let mut first = 0.0f32;
        for _ in 0..BLOCK_FRAMES * 2 {
            first = new_consumer.pop().unwrap();
        }
        assert!((first - 0.5).abs() < 1e-3);
    }

    #[test]
    fn test_engine_playback_push_drops_on_full_without_blocking() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        let mut events = engine.attach_engine_queue(64);
        engine.add_channel(monitor_channel(1, sr));

        // Tiny ring: most of the monitor mix must be dropped, and processing must still finish.
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(16);
        events.push(EngineEvent::SwapOutput { producer }).unwrap();

        attach_dc(&mut engine, 1, 0.5, BLOCK_FRAMES);
        assert_eq!(engine.process_block(), 8, "processing must be bounded by playback capacity");

        assert_eq!(consumer.slots(), 16, "the ring should be filled to capacity and no further");
    }

    #[test]
    fn test_engine_attach_capture_rejects_unknown_channel() {
        let mut engine = AudioEngine::new(48000);
        let (_producer, consumer) = rtrb::RingBuffer::<f32>::new(64);
        assert!(
            !engine.attach_capture(99, 1, consumer),
            "attaching to a non-existent channel must return false"
        );
        assert!(!engine.has_active_captures());
    }

    #[test]
    fn test_engine_command_queue_still_reaches_the_matrix() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        engine.add_channel(monitor_channel(1, sr));
        let mut cmds = engine.attach_command_queue(16);
        cmds.push(MatrixCommand::SetChannelGain {
            channel_idx: 1,
            slider: 0.85,
        })
        .unwrap();

        attach_dc(&mut engine, 1, 0.0, 0); // attach an (empty) capture so the block runs
        engine.process_block();

        let ch = engine.matrix().channel(1).expect("channel 1");
        assert!(
            (ch.channel_fader.target() - slider_to_gain(0.85)).abs() < 1e-4,
            "the matrix command queue attached through the engine must be drained"
        );
    }

    #[test]
    fn test_engine_supports_max_channels() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        for id in 1..=MAX_CHANNELS as u32 {
            engine.add_channel(monitor_channel(id, sr));
        }
        assert_eq!(engine.matrix().channel_count(), MAX_CHANNELS);

        // Attach a capture to every slot; all of them must be tracked.
        let mut producers = Vec::new();
        for id in 1..=MAX_CHANNELS as u32 {
            let (mut p, c) = rtrb::RingBuffer::<f32>::new(64);
            for _ in 0..8 {
                p.push(0.1).unwrap();
                p.push(0.1).unwrap();
            }
            assert!(engine.attach_capture(id, id, c), "slot {id} should attach");
            producers.push(p);
        }
        assert_eq!(engine.active_capture_ids().len(), MAX_CHANNELS);

        let frames = engine.process_block();
        assert_eq!(frames, 8);
        // 32 channels x 0.1 DC, all summed into the Monitor bus, then brickwall-limited to <= 1.0.
        for &s in engine.monitor_mix() {
            assert!(s <= 1.0 && s >= -1.0, "limiter must keep the sum inside [-1, 1], got {s}");
        }
        let _ = producers;
    }

    #[test]
    fn test_engine_odd_sample_tail_keeps_lr_phase_across_blocks() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        engine.add_channel(monitor_channel(1, sr));

        // A stereo stream with DISTINCT L and R values, so any phase slip is detectable.
        // Send an ODD number of samples (3): L=1.0, R=0.0, L=0.5 -> the R partner of the
        // trailing 0.5 is missing, so it must be carried into the next block.
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(4096);
        producer.push(1.0).unwrap(); // L
        producer.push(0.0).unwrap(); // R
        producer.push(0.5).unwrap(); // L (orphan — no R yet)
        assert!(engine.attach_capture(1, 1001, consumer));

        // Block 1: only one complete frame exists, so exactly 1 frame is produced.
        assert_eq!(engine.process_block(), 1, "one complete frame in the odd tail");
        assert!(
            (engine.monitor_mix()[0] - 1.0).abs() < 1e-3,
            "frame 0 L must be 1.0, got {}",
            engine.monitor_mix()[0]
        );
        assert!(
            (engine.monitor_mix()[1] - 0.0).abs() < 1e-3,
            "frame 0 R must be 0.0, got {}",
            engine.monitor_mix()[1]
        );

        // Now the partner for the carried 0.5 arrives.
        producer.push(0.25).unwrap(); // R for the carried L
        assert_eq!(engine.process_block(), 1, "the carried L must pair with the new R");
        assert!(
            (engine.monitor_mix()[0] - 0.5).abs() < 1e-3,
            "carried L must stay on the LEFT channel (0.5), got {} — a phase slip would put \
             the old right sample here",
            engine.monitor_mix()[0]
        );
        assert!(
            (engine.monitor_mix()[1] - 0.25).abs() < 1e-3,
            "new sample must land on the RIGHT channel (0.25), got {}",
            engine.monitor_mix()[1]
        );
    }

    #[test]
    fn test_engine_reattach_clears_orphan_left_sample() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        engine.add_channel(monitor_channel(1, sr));

        // Leave an orphan LEFT sample (0.5) pending from the first stream.
        let (mut old_producer, old_consumer) = rtrb::RingBuffer::<f32>::new(64);
        old_producer.push(0.5).unwrap(); // orphan L, never paired
        assert!(engine.attach_capture(1, 1001, old_consumer));
        assert_eq!(engine.process_block(), 0, "an orphan L alone yields no frame");

        // Re-attach the SAME channel to a NEW stream (e.g. Chrome restarted under a new pid).
        let (mut new_producer, new_consumer) = rtrb::RingBuffer::<f32>::new(64);
        new_producer.push(0.9).unwrap(); // L
        new_producer.push(0.8).unwrap(); // R
        assert!(engine.attach_capture(1, 2002, new_consumer));
        assert_eq!(engine.capture_pid(1), Some(2002));

        assert_eq!(engine.process_block(), 1);
        assert!(
            (engine.monitor_mix()[0] - 0.9).abs() < 1e-3,
            "the new stream's L (0.9) must be first — a leaked orphan (0.5) means the reset \
             did not happen, got {}",
            engine.monitor_mix()[0]
        );
        assert!(
            (engine.monitor_mix()[1] - 0.8).abs() < 1e-3,
            "the new stream's R (0.8) must follow, got {}",
            engine.monitor_mix()[1]
        );
    }

    #[test]
    fn test_engine_try_attach_reports_no_free_slot() {
        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        // Fill every slot with an attached capture.
        for id in 1..=MAX_CHANNELS as u32 {
            engine.add_channel(monitor_channel(id, sr));
            let (_p, c) = rtrb::RingBuffer::<f32>::new(16);
            assert!(engine.try_attach_capture(id, id, c).is_ok(), "slot {id} should attach");
        }
        assert_eq!(engine.active_capture_ids().len(), MAX_CHANNELS);

        // A 33rd channel cannot get a slot: the failure must be reported, not swallowed.
        assert_eq!(
            engine.try_add_channel(monitor_channel(MAX_CHANNELS as u32 + 1, sr)),
            Err(AttachError::NoFreeSlot),
            "adding beyond MAX_CHANNELS must report NoFreeSlot"
        );
        assert!(
            engine.matrix().channel(MAX_CHANNELS as u32 + 1).is_none(),
            "a rejected channel must not be half-registered in the matrix"
        );
    }

    #[test]
    fn test_engine_try_attach_reports_no_such_channel() {
        let mut engine = AudioEngine::new(48000);
        let (_p, c) = rtrb::RingBuffer::<f32>::new(16);
        assert_eq!(
            engine.try_attach_capture(99, 1, c),
            Err(AttachError::NoSuchChannel),
            "attaching to a missing channel must report NoSuchChannel, not NoFreeSlot"
        );
    }

    #[test]
    fn test_engine_telemetry_mirrors_matrix() {        let sr = 48000;
        let mut engine = AudioEngine::new(sr);
        engine.add_channel(monitor_channel(1, sr));
        attach_dc(&mut engine, 1, 0.5, BLOCK_FRAMES);
        assert_eq!(engine.process_block(), BLOCK_FRAMES);

        let mut ch_meters = [MeterSample::default(); MAX_CHANNELS];
        let mut mix_meters = [MeterSample::default(); owl_ipc::MAX_MIXES];
        engine.write_telemetry_samples(&mut ch_meters, &mut mix_meters);

        assert!(
            ch_meters[0].peak_l_f32() > 0.4,
            "channel 1 meter should show the DC level, got {}",
            ch_meters[0].peak_l_f32()
        );
        assert!(
            mix_meters[MixBusId::Monitor.index()].peak_l_f32() > 0.4,
            "monitor bus meter should show the DC level"
        );
    }
}





