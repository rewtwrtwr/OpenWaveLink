//! Dynamic capture supervisor — the Control Plane side of the audio engine.
//!
//! # Why this module exists
//!
//! `ProcessAudioCapture`, `MicrophoneCapture` and `WasapiPlayback` all implement `Drop` by
//! calling `stop()`, which signals the stop event and **joins the WASAPI worker thread**. That
//! makes them *lifetime-owning* handles: if one is constructed in a local variable and allowed
//! to fall out of scope, the stream dies. The previous daemon created exactly one
//! `ProcessAudioCapture` at startup for one PID, so adding an application in the UI only
//! rewrote a text field — no capture stream was ever created for it.
//!
//! This module fixes that by giving the handles a permanent owner: a dedicated OS thread
//! ([`CaptureSupervisor`]) whose [`SupervisorState`] holds `HashMap<channel_id, CaptureHandle>`
//! and `Option<WasapiPlayback>`. Every handle is created on that thread and stays in those
//! fields until it is explicitly removed or the supervisor shuts down. Nothing else in the
//! process ever holds a capture or playback value, so the `Drop`-kills-the-stream trap cannot
//! fire by accident.
//!
//! # Threading contract
//!
//! * The async control plane (JSON-RPC server) sends [`SupervisorCommand`]s over a
//!   `std::sync::mpsc` channel — non-blocking, safe to call from a tokio task.
//! * The supervisor thread performs the blocking WASAPI work (`start()` can block for the
//!   process-loopback activation timeout) and answers over a `tokio::sync::oneshot`.
//! * Every engine mutation is a single `push` into the lock-free `rtrb::Producer<EngineEvent>`.
//!   The control plane never touches the matrix, and the real-time thread is never blocked.
//! * All `Drop`s of capture/playback handles happen on the supervisor thread, never on the
//!   real-time thread and never while holding the server's state lock.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use owl_core::engine::EngineEvent;
use owl_core::{
    InputChannelType, MatrixInputChannel, MicrophoneCapture, MixBusId, ProcessAudioCapture,
    WasapiPlayback,
};

/// Sample rate used when the capture endpoint cannot be queried.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// Which kind of source feeds a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    /// WASAPI process loopback for one PID (`owl.assignProcess` / `owl.addChannel`).
    Process { pid: u32 },
    /// Default hardware capture endpoint (`eCapture`), i.e. the microphone.
    Microphone,
}

/// A channel-creation request from the control plane.
#[derive(Debug, Clone)]
pub struct ChannelSpec {
    pub channel_id: u32,
    pub name: String,
    pub kind: CaptureKind,
}

/// Result of a successful capture start.
#[derive(Debug, Clone)]
pub struct ChannelReady {
    pub channel_id: u32,
    pub sample_rate: u32,
}

/// The render endpoint that is actually bound.
#[derive(Debug, Clone)]
pub struct OutputDevice {
    /// Endpoint id, or empty string when the system default endpoint is bound.
    pub id: String,
    /// Friendly name of the endpoint that is actually bound.
    pub name: String,
}

/// Owns one live capture stream. **Must not be dropped while the stream is wanted.**
enum CaptureHandle {
    Process(ProcessAudioCapture),
    Mic(MicrophoneCapture),
}

impl CaptureHandle {
    fn start(&mut self) -> Result<(), String> {
        match self {
            CaptureHandle::Process(c) => c.start().map_err(|e| e.to_string()),
            CaptureHandle::Mic(c) => c.start().map_err(|e| e.to_string()),
        }
    }

    fn take_consumer(&mut self) -> Option<rtrb::Consumer<f32>> {
        match self {
            CaptureHandle::Process(c) => c.take_consumer(),
            CaptureHandle::Mic(c) => c.take_consumer(),
        }
    }

    fn sample_rate(&self) -> Option<u32> {
        match self {
            CaptureHandle::Process(c) => c.format.map(|f| f.sample_rate),
            CaptureHandle::Mic(c) => c.format.map(|f| f.sample_rate),
        }
    }

    /// Explicitly stop the stream. `Drop` would do this anyway, but being explicit keeps the
    /// join point visible in the code.
    fn stop(&mut self) {
        match self {
            CaptureHandle::Process(c) => c.stop(),
            CaptureHandle::Mic(c) => c.stop(),
        }
    }
}

enum SupervisorCommand {
    Add(
        ChannelSpec,
        tokio::sync::oneshot::Sender<Result<ChannelReady, String>>,
    ),
    Remove(u32, tokio::sync::oneshot::Sender<Result<(), String>>),
    Assign {
        channel_id: u32,
        pid: u32,
        name: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    SetOutput {
        endpoint_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<OutputDevice, String>>,
    },
    Shutdown,
}

/// Async-friendly handle to the supervisor thread. Cheap to clone behind an `Arc`.
pub struct CaptureSupervisor {
    tx: Sender<SupervisorCommand>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl CaptureSupervisor {
    /// Start the supervisor thread. `engine_tx` is the daemon's single
    /// `rtrb::Producer<EngineEvent>`; the supervisor becomes its only writer.
    pub fn spawn(engine_tx: rtrb::Producer<EngineEvent>) -> std::io::Result<Arc<Self>> {
        let (tx, rx) = mpsc::channel();

        let handle = std::thread::Builder::new()
            .name("owl-capture-supervisor".to_string())
            .spawn(move || run_supervisor(rx, engine_tx))?;

        Ok(Arc::new(Self {
            tx,
            thread: Mutex::new(Some(handle)),
        }))
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(tokio::sync::oneshot::Sender<Result<T, String>>) -> SupervisorCommand,
    ) -> Result<T, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(build(tx))
            .map_err(|_| "capture supervisor is not running".to_string())?;
        match rx.await {
            Ok(result) => result,
            Err(_) => Err("capture supervisor dropped the request".to_string()),
        }
    }

    /// Create (or replace) the capture stream for `spec.channel_id` and attach it to the engine.
    pub async fn add_channel(&self, spec: ChannelSpec) -> Result<ChannelReady, String> {
        self.request(|reply| SupervisorCommand::Add(spec, reply))
            .await
    }

    /// Tear down the capture stream of `channel_id` and remove it from the matrix.
    pub async fn remove_channel(&self, channel_id: u32) -> Result<(), String> {
        self.request(|reply| SupervisorCommand::Remove(channel_id, reply))
            .await
    }

    /// Re-point an existing channel at a different PID (restarts its capture stream, keeping the
    /// matrix channel and therefore its fader / mute / VST3 state).
    pub async fn assign_process(
        &self,
        channel_id: u32,
        pid: u32,
        name: String,
    ) -> Result<(), String> {
        self.request(|reply| SupervisorCommand::Assign {
            channel_id,
            pid,
            name,
            reply,
        })
        .await
    }

    /// Bind playback to `endpoint_id` (`None` = system default) and hand the engine the new
    /// producer. Returns the endpoint that is actually bound.
    pub async fn set_output_device(
        &self,
        endpoint_id: Option<String>,
    ) -> Result<OutputDevice, String> {
        self.request(|reply| SupervisorCommand::SetOutput { endpoint_id, reply })
            .await
    }

    /// Stop every capture and the playback device, then join the supervisor thread.
    /// Idempotent.
    pub fn shutdown(&self) {
        let _ = self.tx.send(SupervisorCommand::Shutdown);
        if let Ok(mut guard) = self.thread.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
    }
}

/// Everything the supervisor thread owns. All capture/playback values live here.
struct SupervisorState {
    /// Live capture streams, keyed by matrix channel id. **This map is what keeps them alive.**
    captures: HashMap<u32, CaptureHandle>,
    /// Channel ids we have already pushed `EngineEvent::AddChannel` for.
    registered: HashSet<u32>,
    /// The one live playback device. Same lifetime rule as the captures.
    output: Option<WasapiPlayback>,
    engine_tx: rtrb::Producer<EngineEvent>,
}

fn run_supervisor(rx: Receiver<SupervisorCommand>, engine_tx: rtrb::Producer<EngineEvent>) {
    let mut state = SupervisorState {
        captures: HashMap::new(),
        registered: HashSet::new(),
        output: None,
        engine_tx,
    };

    while let Ok(cmd) = rx.recv() {
        match cmd {
            SupervisorCommand::Add(spec, reply) => {
                let _ = reply.send(add_capture(&mut state, spec));
            }
            SupervisorCommand::Remove(channel_id, reply) => {
                let _ = reply.send(remove_capture(&mut state, channel_id));
            }
            SupervisorCommand::Assign {
                channel_id,
                pid,
                name,
                reply,
            } => {
                let _ = reply.send(assign_process(&mut state, channel_id, pid, &name));
            }
            SupervisorCommand::SetOutput { endpoint_id, reply } => {
                let _ = reply.send(swap_output(&mut state, endpoint_id.as_deref()));
            }
            SupervisorCommand::Shutdown => break,
        }
    }

    // Graceful teardown, on the thread that created every handle.
    let ids: Vec<u32> = state.captures.keys().copied().collect();
    for channel_id in ids {
        stop_capture_stream(&mut state, channel_id);
        log::info!("Supervisor: capture for channel {channel_id} stopped");
    }
    if let Some(mut playback) = state.output.take() {
        playback.stop();
    }
    log::info!("Capture supervisor stopped");
}

/// Stop and drop the capture stream of `channel_id`, pushing `DetachCapture` to the engine.
///
/// Returns `true` if a live stream was actually torn down. The matrix channel is **not**
/// removed — that is [`remove_capture`]'s job, so that re-assigning a PID keeps the channel's
/// fader, mute and VST3 state.
fn stop_capture_stream(state: &mut SupervisorState, channel_id: u32) -> bool {
    // `remove` takes ownership out of the map: the handle is now a local, and it is dropped
    // (running `Drop::drop` -> `stop()` -> `join()`) at the end of this function, on the
    // supervisor thread. Nothing here runs on the real-time thread.
    let Some(mut capture) = state.captures.remove(&channel_id) else {
        return false;
    };

    let _ = state
        .engine_tx
        .push(EngineEvent::DetachCapture { channel_id });
    capture.stop();
    true
}

/// Start a capture stream, register its matrix channel and attach its ring to the engine.
fn add_capture(state: &mut SupervisorState, spec: ChannelSpec) -> Result<ChannelReady, String> {
    // Re-pointing an existing channel: tear the old stream down before starting the new one.
    stop_capture_stream(state, spec.channel_id);

    // If it's a process channel with PID 0 that is NOT System, it is an installed app or
    // virtual cable channel waiting for app launch. Register it in the matrix without
    // attempting process loopback on PID 0.
    if let CaptureKind::Process { pid } = spec.kind {
        if pid == 0 && !spec.name.eq_ignore_ascii_case("System") {
            if state.registered.insert(spec.channel_id) {
                let channel = build_matrix_channel(spec.channel_id, &spec.name, spec.kind, DEFAULT_SAMPLE_RATE);
                let _ = state.engine_tx.push(EngineEvent::AddChannel { channel: Box::new(channel) });
            }
            return Ok(ChannelReady {
                channel_id: spec.channel_id,
                sample_rate: DEFAULT_SAMPLE_RATE,
            });
        }
    }

    let mut handle = match spec.kind {
        CaptureKind::Process { pid } => CaptureHandle::Process(ProcessAudioCapture::new(pid)),
        CaptureKind::Microphone => CaptureHandle::Mic(
            MicrophoneCapture::new().map_err(|e| format!("microphone init failed: {e}"))?,
        ),
    };

    handle.start().map_err(|e| match spec.kind {
        CaptureKind::Process { pid } => {
            format!("failed to start process loopback capture for PID {pid}: {e}")
        }
        CaptureKind::Microphone => format!("failed to start microphone capture: {e}"),
    })?;

    let sample_rate = handle.sample_rate().unwrap_or(DEFAULT_SAMPLE_RATE);
    let consumer = handle
        .take_consumer()
        .ok_or_else(|| "capture start produced no ring consumer".to_string())?;

    // Register the matrix channel once. On a re-assign the channel already exists, so only the
    // capture ring is swapped and the channel's parameters survive.
    if state.registered.insert(spec.channel_id) {
        let channel = build_matrix_channel(spec.channel_id, &spec.name, spec.kind, sample_rate);
        if state
            .engine_tx
            .push(EngineEvent::AddChannel { channel: Box::new(channel) })
            .is_err()
        {
            state.registered.remove(&spec.channel_id);
            return Err("engine event queue is full (AddChannel)".to_string());
        }
    }

    let pid = match spec.kind {
        CaptureKind::Process { pid } => pid,
        CaptureKind::Microphone => 0,
    };

    if state
        .engine_tx
        .push(EngineEvent::AttachCapture {
            channel_id: spec.channel_id,
            pid,
            consumer,
        })
        .is_err()
    {
        return Err("engine event queue is full (AttachCapture)".to_string());
    }

    // Ownership transfer happens here: from now on the stream lives in the map, for as long as
    // the channel exists. Any earlier `return` above drops `handle` instead, which stops the
    // stream we just failed to attach — no leak, no orphan thread.
    let kind = spec.kind;
    state.captures.insert(spec.channel_id, handle);

    log::info!(
        "Supervisor: channel {} capture started ({:?}, {} Hz)",
        spec.channel_id,
        kind,
        sample_rate
    );

    Ok(ChannelReady {
        channel_id: spec.channel_id,
        sample_rate,
    })
}

/// Stop a channel's capture stream, then detach and remove it from the engine matrix.
fn remove_capture(state: &mut SupervisorState, channel_id: u32) -> Result<(), String> {
    let had_capture = stop_capture_stream(state, channel_id);

    let _ = state
        .engine_tx
        .push(EngineEvent::RemoveChannel { channel_id });
    state.registered.remove(&channel_id);

    if !had_capture {
        log::warn!("Supervisor: channel {channel_id} had no live capture to remove");
    }
    Ok(())
}

/// Point an existing channel at a different PID.
fn assign_process(
    state: &mut SupervisorState,
    channel_id: u32,
    pid: u32,
    name: &str,
) -> Result<(), String> {
    add_capture(
        state,
        ChannelSpec {
            channel_id,
            name: name.to_string(),
            kind: CaptureKind::Process { pid },
        },
    )
    .map(|_| ())
}

/// Bind a render endpoint and hand its producer to the engine.
fn swap_output(
    state: &mut SupervisorState,
    endpoint_id: Option<&str>,
) -> Result<OutputDevice, String> {
    // An explicitly requested endpoint that does not resolve is an error, not a silent
    // fallback to the default — otherwise the UI could show one device and play to another.
    let requested_name = match endpoint_id {
        Some(id) => Some(
            owl_core::devices::resolve_endpoint_name(id)
                .ok_or_else(|| format!("unknown render endpoint id: {id}"))?,
        ),
        None => None,
    };

    let mut playback = WasapiPlayback::new_with_device(endpoint_id)
        .map_err(|e| format!("failed to open render endpoint: {e}"))?;
    playback
        .start()
        .map_err(|e| format!("failed to start render endpoint: {e}"))?;
    let producer = playback
        .take_producer()
        .ok_or_else(|| "render endpoint start produced no ring producer".to_string())?;

    // Report the endpoint that is ACTUALLY bound, not the one that was requested. For the
    // default case `endpoint_id` is `None`, but the UI still needs the real id to highlight the
    // right row — `endpoint_id()` gives us the id the endpoint itself reports.
    let bound_name = playback
        .device_name()
        .map(|s| s.to_string())
        .or(requested_name)
        .or_else(default_output_name)
        .unwrap_or_else(|| "Unknown Output Device".to_string());
    let bound_id = playback
        .endpoint_id()
        .map(|s| s.to_string())
        .or_else(|| endpoint_id.map(|s| s.to_string()))
        .unwrap_or_default();

    // Hand the new producer to the engine BEFORE stopping the previous device, so the monitor
    // feed never has a silent gap. If the queue is full, `playback` is dropped here and the
    // previous device stays bound.
    if state
        .engine_tx
        .push(EngineEvent::SwapOutput { producer })
        .is_err()
    {
        return Err("engine event queue is full (SwapOutput)".to_string());
    }

    // The previous WasapiPlayback is stopped here, on the thread that created it. The engine has
    // already replaced its producer handle, so no stale producer remains in use.
    if let Some(mut previous) = state.output.replace(playback) {
        previous.stop();
    }

    log::info!(
        "Supervisor: playback bound to '{bound_name}' (id: {})",
        if bound_id.is_empty() { "system default" } else { &bound_id }
    );
    // The failure mode that made "no sound at all" invisible: the default render endpoint on
    // this machine is `Spotify (Elgato Virtual Audio)`, a cable nobody is listening to.
    if owl_core::devices::is_virtual_endpoint_name(&bound_name) {
        log::warn!(
            "Supervisor: playback endpoint '{bound_name}' looks like a VIRTUAL audio cable — \
             the monitor mix will not be audible on physical speakers/headphones. \
             Select a real output device (e.g. via owl.setOutputDevice)."
        );
    }

    Ok(OutputDevice {
        id: bound_id,
        name: bound_name,
    })
}

/// Friendly name of the current default render endpoint, if it can be enumerated.
fn default_output_name() -> Option<String> {
    let devices = owl_core::get_system_audio_devices().ok()?;
    devices
        .outputs
        .iter()
        .find(|d| d.is_default)
        .map(|d| d.name.clone())
}

/// Build the matrix channel with the routing this daemon has always used.
fn build_matrix_channel(
    channel_id: u32,
    name: &str,
    kind: CaptureKind,
    sample_rate: u32,
) -> MatrixInputChannel {
    let channel_type = match kind {
        CaptureKind::Process { pid } => InputChannelType::SoftwareProcess { pid },
        CaptureKind::Microphone => InputChannelType::Microphone,
    };

    let mut ch = MatrixInputChannel::new(channel_id, name, channel_type, sample_rate);

    match kind {
        CaptureKind::Process { .. } => {
            // Stream Mix at 30% (-10.5 dB), Monitor Mix at unity (0 dB).
            ch.send_mut(MixBusId::Stream).set_gain_slider(0.30);
            ch.send_mut(MixBusId::Stream).gain_smoother.reset(0.30);
            ch.send_mut(MixBusId::Monitor).set_gain_slider(0.50);
            ch.send_mut(MixBusId::Monitor).gain_smoother.reset(1.0);
        }
        CaptureKind::Microphone => {
            // Stream Mix at unity (0 dB), Monitor Mix muted to prevent headphone self-echo.
            ch.send_mut(MixBusId::Stream).set_gain_slider(0.50);
            ch.send_mut(MixBusId::Stream).gain_smoother.reset(1.0);
            ch.send_mut(MixBusId::Monitor).set_muted(true);
            ch.send_mut(MixBusId::Monitor).gain_smoother.reset(0.0);
        }
    }

    ch.send_mut(MixBusId::Vod).set_muted(true);
    ch.send_mut(MixBusId::Discord).set_muted(true);
    ch.send_mut(MixBusId::Aux).set_muted(true);
    ch.channel_fader.reset(1.0);

    ch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_endpoint_heuristic() {
        // Measured on this machine: the default eRender/eConsole endpoint is a virtual cable.
        assert!(owl_core::devices::is_virtual_endpoint_name(
            "Spotify (Elgato Virtual Audio)"
        ));
        assert!(owl_core::devices::is_virtual_endpoint_name(
            "CABLE Input (VB-Audio Virtual Cable)"
        ));
        assert!(owl_core::devices::is_virtual_endpoint_name(
            "System (Elgato Virtual Audio)"
        ));

        // The user's real headphones must NOT be flagged.
        assert!(!owl_core::devices::is_virtual_endpoint_name(
            "Speakers (Sound Blaster Play! 3)"
        ));
        assert!(!owl_core::devices::is_virtual_endpoint_name(
            "Microphone (fifine AM8 Pro)"
        ));
    }

    #[test]
    fn test_matrix_channel_routing_matches_legacy_daemon() {
        let proc_ch =
            build_matrix_channel(1, "chrome.exe", CaptureKind::Process { pid: 42 }, 48_000);
        assert_eq!(proc_ch.id, 1);
        assert_eq!(
            proc_ch.channel_type,
            InputChannelType::SoftwareProcess { pid: 42 }
        );
        assert!(!proc_ch.send(MixBusId::Stream).is_muted);
        assert!(!proc_ch.send(MixBusId::Monitor).is_muted);
        assert!(proc_ch.send(MixBusId::Vod).is_muted);
        assert!(proc_ch.send(MixBusId::Discord).is_muted);
        assert!(proc_ch.send(MixBusId::Aux).is_muted);

        let mic_ch = build_matrix_channel(2, "Hardware Mic", CaptureKind::Microphone, 48_000);
        assert_eq!(mic_ch.channel_type, InputChannelType::Microphone);
        assert!(mic_ch.clipguard.is_some(), "mic channel keeps its Clipguard");
        assert!(
            mic_ch.send(MixBusId::Monitor).is_muted,
            "mic must stay out of the headphone feed"
        );
        assert!(!mic_ch.send(MixBusId::Stream).is_muted);
    }
}
