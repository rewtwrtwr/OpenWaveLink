//! OpenWaveLink Control Plane - JSON-RPC 2.0 Localhost WebSocket Server
//!
//! Provides two-way WebSocket communication on `127.0.0.1:1824` for Tauri UI,
//! In-Game HUD overlay, and Windows 11 System Tray Flyout.
//!
//! Channel lifecycle (`owl.addChannel` / `owl.removeChannel`) and output-device switching
//! (`owl.setOutputDevice`) are delegated to the [`CaptureSupervisor`], which owns the live
//! WASAPI streams. Those calls await the supervisor with the state lock **released**, so a
//! blocking `IAudioClient::Initialize` can never stall unrelated RPCs.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use owl_core::MatrixCommand;
use owl_core::CableManager;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use crate::capture_pool::{CaptureKind, CaptureSupervisor, ChannelSpec};

pub const DEFAULT_WS_PORT: u16 = 1824;

/// VST3 slot state for a channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vst3SlotState {
    pub slot_id: usize,
    pub plugin_id: Option<String>,
    pub plugin_name: Option<String>,
    pub category: Option<String>,
    pub is_enabled: bool,
    pub is_bypassed: bool,
    pub dry_wet: f32,
}

/// Channel state maintained by Control Plane
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub id: u32,
    pub name: String,
    pub channel_type: String,
    pub pid: u32,
    pub gain: f32,
    pub is_muted: bool,
    pub sends: Vec<SendState>,
    pub vst3_slots: Vec<Vst3SlotState>,
}

/// Send state for an input channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendState {
    pub mix_id: usize,
    pub name: String,
    pub gain: f32,
    pub is_muted: bool,
}

/// Mix bus state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MixState {
    pub id: usize,
    pub name: String,
    pub gain: f32,
    pub is_muted: bool,
}

/// Daemon and Matrix state shared across WebSocket sessions
pub struct ServerState {
    pub channels: Vec<ChannelState>,
    pub mixes: Vec<MixState>,
    pub cmd_producer: Arc<Mutex<rtrb::Producer<MatrixCommand>>>,
    pub broadcast_tx: broadcast::Sender<String>,
    /// Owns the live capture/playback streams. `None` in unit tests and when the audio core
    /// could not be started; channel bookkeeping still works, only the streams are absent.
    pub supervisor: Option<Arc<CaptureSupervisor>>,
    /// Endpoint id actually bound for the Monitor mix (empty = system default).
    pub output_device_id: String,
    /// Friendly name of the endpoint actually bound.
    pub output_device_name: String,
    /// Next channel id to hand out. Never reused after removal, so a stale UI reference can
    /// never address a different channel.
    pub next_channel_id: u32,
    /// Owns the USB/IP virtual cable server. `None` when the cable helpers
    /// (`virtualcables.exe` / `usbip.exe`) are not installed, in which case the
    /// cable RPCs report a precise reason instead of failing silently.
    pub cables: Option<Arc<CableManager>>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: Option<String>,
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: serde_json::Value,
}

impl ServerState {
    pub fn new(cmd_producer: rtrb::Producer<MatrixCommand>) -> (Self, broadcast::Sender<String>) {
        let (broadcast_tx, _) = broadcast::channel(128);

        let mix_names = ["Stream Mix", "Monitor Mix", "VOD Track", "Discord Feed", "Aux Mix"];
        let mixes = mix_names
            .iter()
            .enumerate()
            .map(|(i, &name)| MixState {
                id: i,
                name: name.to_string(),
                gain: 1.0,
                is_muted: false,
            })
            .collect();

        let state = Self {
            channels: Vec::new(),
            mixes,
            cmd_producer: Arc::new(Mutex::new(cmd_producer)),
            broadcast_tx: broadcast_tx.clone(),
            supervisor: None,
            output_device_id: String::new(),
            output_device_name: String::new(),
            next_channel_id: 1,
            cables: None,
        };

        (state, broadcast_tx)
    }

    /// Attach the capture supervisor that owns the live WASAPI streams.
    pub fn set_supervisor(&mut self, supervisor: Arc<CaptureSupervisor>) {
        self.supervisor = Some(supervisor);
    }

    /// Attach the virtual cable manager. Called once at startup when the
    /// USB/IP helpers are present; absent helpers leave this `None` and the
    /// cable RPCs explain exactly what is missing.
    pub fn set_cable_manager(&mut self, manager: Arc<CableManager>) {
        self.cables = Some(manager);
    }

    /// Reserve the next channel id (never reused).
    pub fn allocate_channel_id(&mut self) -> u32 {
        let id = self.next_channel_id;
        self.next_channel_id = self.next_channel_id.saturating_add(1);
        id
    }

    /// Add an initial channel to state
    pub fn add_channel(
        &mut self,
        id: u32,
        name: impl Into<String>,
        channel_type: &str,
        pid: u32,
        gain: f32,
        is_muted: bool,
    ) {
        self.next_channel_id = self.next_channel_id.max(id.saturating_add(1));
        self.channels.push(build_channel_state(
            id,
            name.into(),
            channel_type,
            pid,
            gain,
            is_muted,
        ));
    }

    /// Find a channel by ID first, then fall back to slice index.
    ///
    /// ID-first is required once channels can be created and deleted: with the old
    /// index-first lookup, `channel_id == 2` would address whatever channel happens to sit at
    /// index 2, which is a different channel as soon as one is removed.
    pub fn find_channel_mut(&mut self, ch_idx: usize) -> Option<&mut ChannelState> {
        if let Some(pos) = self.channels.iter().position(|c| c.id as usize == ch_idx) {
            self.channels.get_mut(pos)
        } else {
            self.channels.get_mut(ch_idx)
        }
    }

    /// Broadcast a JSON-RPC notification to all connected WebSocket clients
    pub fn broadcast_notification(&self, method: &str, params: serde_json::Value) {
        let notification = JsonRpcNotification {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        };

        if let Ok(json_str) = serde_json::to_string(&notification) {
            let _ = self.broadcast_tx.send(json_str);
        }
    }

    /// Broadcast the full channel list after a create/delete.
    fn broadcast_channels_list(&self) {
        self.broadcast_notification(
            "channelsListChanged",
            serde_json::json!({ "channels": self.channels }),
        );
    }
}

/// Build a `ChannelState` with the default send and VST3 layout.
fn build_channel_state(
    id: u32,
    name: String,
    channel_type: &str,
    pid: u32,
    gain: f32,
    is_muted: bool,
) -> ChannelState {
    let mix_names = ["Stream Mix", "Monitor Mix", "VOD Track", "Discord Feed", "Aux Mix"];
    let sends = mix_names
        .iter()
        .enumerate()
        .map(|(i, &mname)| SendState {
            mix_id: i,
            name: mname.to_string(),
            gain: 1.0,
            is_muted: false,
        })
        .collect();

    let vst3_slots = (0..4)
        .map(|i| Vst3SlotState {
            slot_id: i,
            plugin_id: None,
            plugin_name: None,
            category: None,
            is_enabled: false,
            is_bypassed: false,
            dry_wet: 1.0,
        })
        .collect();

    ChannelState {
        id,
        name,
        channel_type: channel_type.to_string(),
        pid,
        gain,
        is_muted,
        sends,
        vst3_slots,
    }
}

/// WebSocket Server instance
pub struct WsServer {
    pub port: u16,
    pub addr: SocketAddr,
    pub info_path: PathBuf,
    listener: TcpListener,
    state: Arc<tokio::sync::RwLock<ServerState>>,
    broadcast_tx: broadcast::Sender<String>,
}

impl WsServer {
    /// Bind on 127.0.0.1:1824 or find a free port if busy
    pub async fn bind(
        preferred_port: u16,
        state: ServerState,
        broadcast_tx: broadcast::Sender<String>,
    ) -> Result<Self> {
        let mut port = preferred_port;
        let listener = match TcpListener::bind(format!("127.0.0.1:{port}")).await {
            Ok(l) => l,
            Err(_) => {
                // If preferred port is busy, find a free ephemeral port
                warn!("Port {port} busy, attempting to bind ephemeral port on 127.0.0.1:0");
                TcpListener::bind("127.0.0.1:0")
                    .await
                    .context("Failed to bind any port on 127.0.0.1")?
            }
        };

        let addr = listener.local_addr()?;
        port = addr.port();

        let info_path = write_ws_info(port)?;
        info!("WebSocket Server listening on ws://{} (info written to {})", addr, info_path.display());

        Ok(Self {
            port,
            addr,
            info_path,
            listener,
            state: Arc::new(tokio::sync::RwLock::new(state)),
            broadcast_tx,
        })
    }

    /// Get shared state handle
    pub fn state(&self) -> Arc<tokio::sync::RwLock<ServerState>> {
        Arc::clone(&self.state)
    }

    /// Run the server accept loop
    pub async fn run(self) -> Result<()> {
        let state = self.state;
        let broadcast_tx = self.broadcast_tx;

        loop {
            match self.listener.accept().await {
                Ok((stream, client_addr)) => {
                    let state_clone = Arc::clone(&state);
                    let broadcast_clone = broadcast_tx.clone();

                    tokio::spawn(async move {
                        if let Err(e) = handle_client(stream, state_clone, broadcast_clone).await {
                            warn!("Client {} disconnected with error: {:?}", client_addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Error accepting WebSocket connection: {:?}", e);
                    break;
                }
            }
        }

        Ok(())
    }
}

async fn handle_client(
    stream: TcpStream,
    state: Arc<tokio::sync::RwLock<ServerState>>,
    broadcast_tx: broadcast::Sender<String>,
) -> Result<()> {
    let ws_stream = accept_async(stream).await?;
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let mut broadcast_rx = broadcast_tx.subscribe();

    loop {
        tokio::select! {
            // Receive server broadcast notification and forward to client
            broadcast_msg = broadcast_rx.recv() => {
                match broadcast_msg {
                    Ok(text) => {
                        if ws_sender.send(Message::text(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }

            // Receive client message
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let text_str = text.as_str();
                        if let Some(resp) = handle_rpc(text_str, &state).await
                            && ws_sender.send(Message::text(resp)).await.is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if ws_sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

async fn handle_rpc(
    text: &str,
    state_lock: &Arc<tokio::sync::RwLock<ServerState>>,
) -> Option<String> {
    let req: JsonRpcRequest = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id: None,
                result: None,
                error: Some(JsonRpcError {
                    code: -32700,
                    message: format!("Parse error: {e}"),
                }),
            };
            return serde_json::to_string(&resp).ok();
        }
    };

    let id = req.id.clone();
    let method = req.method.as_str();
    let params = req.params.unwrap_or(serde_json::Value::Null);

    // Methods that must await the capture supervisor are handled before taking the state lock,
    // so a blocking WASAPI call never holds it.
    match method {
        "owl.addChannel" | "addChannel" => {
            return rpc_add_channel(id, &params, state_lock).await;
        }
        "owl.removeChannel" | "removeChannel" => {
            return rpc_remove_channel(id, &params, state_lock).await;
        }
        "owl.setOutputDevice" | "setOutputDevice" => {
            return rpc_set_output_device(id, &params, state_lock).await;
        }
        "owl.createCable" | "createCable" | "owl.addCable" | "addCable" => {
            return rpc_create_cable(id, &params, state_lock).await;
        }
        "owl.listCables" | "listCables" | "owl.getCables" | "getCables" => {
            return rpc_list_cables(id, state_lock).await;
        }
        "owl.bindAppAudio" | "bindAppAudio" => {
            let pid = parse_int_field(&params, &["pid"]).unwrap_or(0) as u32;
            let endpoint_id = parse_str_field(&params, &["endpoint_id", "endpointId"]).unwrap_or_default();
            let res = owl_core::set_app_audio_endpoint(pid, &endpoint_id);
            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "success": res.is_ok(),
                    "message": res.err(),
                })),
                error: None,
            };
            return serde_json::to_string(&resp).ok();
        }
        _ => {}
    }

    let mut state = state_lock.write().await;

    match method {
        "owl.getChannels" | "getChannels" => {
            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::to_value(&state.channels).unwrap_or_default()),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.getMixes" | "getMixes" => {
            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::to_value(&state.mixes).unwrap_or_default()),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.getEngineState" | "getEngineState" => {
            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "channels": state.channels,
                    "mixes": state.mixes,
                    "output_device_id": state.output_device_id,
                    "output_device_name": state.output_device_name,
                })),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.setChannel" | "setChannel" | "setChannelGain" | "setChannelMute" => {
            let ch_idx = parse_int_field(&params, &["channel_idx", "channelId", "channel_id", "id"])
                .unwrap_or(0);
            let gain_opt = parse_float_field(&params, &["slider", "gain"]);
            let mute_opt = parse_bool_field(&params, &["is_muted", "isMuted", "muted"]);
            let name_opt = parse_str_field(&params, &["name"]);

            if let Some(gain) = gain_opt {
                if let Ok(mut prod) = state.cmd_producer.lock() {
                    let _ = prod.push(MatrixCommand::SetChannelGain {
                        channel_idx: ch_idx,
                        slider: gain,
                    });
                }
                if let Some(ch) = state.find_channel_mut(ch_idx) {
                    ch.gain = gain;
                }
            }

            if let Some(muted) = mute_opt {
                if let Ok(mut prod) = state.cmd_producer.lock() {
                    let _ = prod.push(MatrixCommand::SetChannelMute {
                        channel_idx: ch_idx,
                        muted,
                    });
                }
                if let Some(ch) = state.find_channel_mut(ch_idx) {
                    ch.is_muted = muted;
                }
            }

            if let Some(ref name) = name_opt
                && let Some(ch) = state.find_channel_mut(ch_idx)
            {
                ch.name = name.clone();
            }

            // Broadcast notification
            state.broadcast_notification(
                "channelChanged",
                serde_json::json!({
                    "channel_idx": ch_idx,
                    "gain": gain_opt,
                    "is_muted": mute_opt,
                    "name": name_opt,
                }),
            );

            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({ "success": true, "channel_idx": ch_idx })),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.setSend" | "setSend" | "setSendGain" | "setSendMute" => {
            let ch_idx = parse_int_field(&params, &["channel_idx", "channelId", "channel_id", "id"])
                .unwrap_or(0);
            let mix_idx = parse_int_field(&params, &["mix_idx", "mixId", "mix_id"])
                .unwrap_or(0);
            let gain_opt = parse_float_field(&params, &["slider", "gain"]);
            let mute_opt = parse_bool_field(&params, &["is_muted", "isMuted", "muted"]);

            if let Some(gain) = gain_opt {
                if let Ok(mut prod) = state.cmd_producer.lock() {
                    let _ = prod.push(MatrixCommand::SetSendGain {
                        channel_idx: ch_idx,
                        mix_idx,
                        slider: gain,
                    });
                }
                if let Some(ch) = state.find_channel_mut(ch_idx)
                    && let Some(send) = ch.sends.get_mut(mix_idx)
                {
                    send.gain = gain;
                }
            }

            if let Some(muted) = mute_opt {
                if let Ok(mut prod) = state.cmd_producer.lock() {
                    let _ = prod.push(MatrixCommand::SetSendMute {
                        channel_idx: ch_idx,
                        mix_idx,
                        muted,
                    });
                }
                if let Some(ch) = state.find_channel_mut(ch_idx)
                    && let Some(send) = ch.sends.get_mut(mix_idx)
                {
                    send.is_muted = muted;
                }
            }

            state.broadcast_notification(
                "channelChanged",
                serde_json::json!({
                    "channel_idx": ch_idx,
                    "mix_idx": mix_idx,
                    "gain": gain_opt,
                    "is_muted": mute_opt,
                }),
            );

            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({ "success": true, "channel_idx": ch_idx, "mix_idx": mix_idx })),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.setMix" | "setMix" | "setMixGain" | "setMixMute" => {
            let mix_idx = parse_int_field(&params, &["mix_idx", "mixId", "mix_id", "id"])
                .unwrap_or(0);
            let gain_opt = parse_float_field(&params, &["slider", "gain"]);
            let mute_opt = parse_bool_field(&params, &["is_muted", "isMuted", "muted"]);

            if let Some(gain) = gain_opt {
                if let Ok(mut prod) = state.cmd_producer.lock() {
                    let _ = prod.push(MatrixCommand::SetMixGain {
                        mix_idx,
                        slider: gain,
                    });
                }
                if let Some(mix) = state.mixes.get_mut(mix_idx) {
                    mix.gain = gain;
                }
            }

            if let Some(muted) = mute_opt {
                if let Ok(mut prod) = state.cmd_producer.lock() {
                    let _ = prod.push(MatrixCommand::SetMixMute {
                        mix_idx,
                        muted,
                    });
                }
                if let Some(mix) = state.mixes.get_mut(mix_idx) {
                    mix.is_muted = muted;
                }
            }

            state.broadcast_notification(
                "mixChanged",
                serde_json::json!({
                    "mix_idx": mix_idx,
                    "gain": gain_opt,
                    "is_muted": mute_opt,
                }),
            );

            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({ "success": true, "mix_idx": mix_idx })),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.assignProcess" | "assignProcess" | "assignProcessToChannel" => {
            let ch_idx = parse_int_field(&params, &["channel_idx", "channelId", "channel_id", "id"])
                .unwrap_or(0);
            let pid = parse_int_field(&params, &["pid"]).map(|p| p as u32).unwrap_or(0);
            let proc_name = parse_str_field(&params, &["process_name", "processName", "name"]);

            // The matrix command goes first and alone: it is the documented parameter path, and
            // the existing daemon tests assert on this exact queue entry.
            if let Ok(mut prod) = state.cmd_producer.lock() {
                let _ = prod.push(MatrixCommand::AssignProcess {
                    channel_idx: ch_idx,
                    pid,
                });
            }

            let mut resolved_channel_id = None;
            let mut resolved_name = proc_name.clone();
            if let Some(ch) = state.find_channel_mut(ch_idx) {
                ch.pid = pid;
                ch.channel_type = "SoftwareProcess".to_string();
                if let Some(ref name) = proc_name {
                    ch.name = name.clone();
                }
                resolved_channel_id = Some(ch.id);
                resolved_name = Some(ch.name.clone());
            }

            // A PID change is a real capture restart: the old loopback stream is torn down and a
            // new one attached for the same channel, so the channel keeps its fader/mute/VST3
            // state. This is the fix for "adding an app in the UI produced no audio".
            let supervisor = state.supervisor.clone();
            let channel_id = resolved_channel_id.unwrap_or(ch_idx as u32);
            let capture_name = resolved_name.unwrap_or_else(|| format!("PID {pid}"));
            let broadcast_after = state.broadcast_tx.clone();
            drop(state);

            let mut capture_restarted = false;
            let mut capture_error = None;
            if let Some(supervisor) = supervisor {
                match supervisor.assign_process(channel_id, pid, capture_name).await {
                    Ok(()) => capture_restarted = true,
                    Err(err) => {
                        warn!("assignProcess: capture restart failed for channel {channel_id}: {err}");
                        capture_error = Some(err);
                    }
                }
            }

            let notification = JsonRpcNotification {
                jsonrpc: "2.0",
                method: "channelChanged".to_string(),
                params: serde_json::json!({
                    "channel_idx": ch_idx,
                    "channel_id": channel_id,
                    "pid": pid,
                    "name": proc_name,
                    "capture_restarted": capture_restarted,
                    "capture_error": capture_error,
                }),
            };
            if let Ok(json_str) = serde_json::to_string(&notification) {
                let _ = broadcast_after.send(json_str);
            }

            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "success": capture_error.is_none(),
                    "channel_idx": ch_idx,
                    "channel_id": channel_id,
                    "pid": pid,
                    "capture_restarted": capture_restarted,
                    "capture_error": capture_error,
                })),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.getVst3Plugins" | "getVst3Plugins" => {
            let plugins = owl_core::scan_vst3_plugins();
            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "plugins": plugins
                })),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.setVst3Slot" | "setVst3Slot" => {
            let ch_idx = parse_int_field(&params, &["channel_idx", "channelId", "id"]).unwrap_or(0);
            let slot_idx = parse_int_field(&params, &["slot_idx", "slotId", "slot"]).unwrap_or(0);
            let plugin_id = parse_str_field(&params, &["plugin_id", "pluginId"]);
            let is_bypassed = parse_bool_field(&params, &["is_bypassed", "isBypassed", "bypassed"]);
            let dry_wet = parse_float_field(&params, &["dry_wet", "dryWet", "mix"]);
            let clear = parse_bool_field(&params, &["clear"]).unwrap_or(false);

            if let Ok(mut prod) = state.cmd_producer.lock() {
                if clear {
                    let _ = prod.push(MatrixCommand::ClearVst3Slot {
                        channel_idx: ch_idx,
                        slot_idx,
                    });
                } else {
                    if let Some(ref pid) = plugin_id {
                        let _ = prod.push(MatrixCommand::LoadVst3Plugin {
                            channel_idx: ch_idx,
                            slot_idx,
                            plugin_id: pid.clone(),
                        });
                    }
                    if let Some(bypassed) = is_bypassed {
                        let _ = prod.push(MatrixCommand::SetVst3Bypass {
                            channel_idx: ch_idx,
                            slot_idx,
                            bypassed,
                        });
                    }
                    if let Some(dw) = dry_wet {
                        let _ = prod.push(MatrixCommand::SetVst3DryWet {
                            channel_idx: ch_idx,
                            slot_idx,
                            dry_wet: dw,
                        });
                    }
                }
            }

            if let Some(ch) = state.find_channel_mut(ch_idx) {
                if slot_idx < ch.vst3_slots.len() {
                    let slot = &mut ch.vst3_slots[slot_idx];
                    if clear {
                        slot.plugin_id = None;
                        slot.plugin_name = None;
                        slot.category = None;
                        slot.is_enabled = false;
                        slot.is_bypassed = false;
                    } else {
                        if let Some(ref pid) = plugin_id {
                            let available = owl_core::scan_vst3_plugins();
                            if let Some(info) = available.iter().find(|p| &p.id == pid || &p.name == pid) {
                                slot.plugin_id = Some(info.id.clone());
                                slot.plugin_name = Some(info.name.clone());
                                slot.category = Some(info.category.clone());
                                slot.is_enabled = true;
                            }
                        }
                        if let Some(bypassed) = is_bypassed {
                            slot.is_bypassed = bypassed;
                        }
                        if let Some(dw) = dry_wet {
                            slot.dry_wet = dw;
                        }
                    }
                }
            }

            state.broadcast_notification(
                "vst3SlotChanged",
                serde_json::json!({
                    "channel_idx": ch_idx,
                    "slot_idx": slot_idx,
                }),
            );

            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "success": true,
                    "channel_idx": ch_idx,
                    "slot_idx": slot_idx
                })),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        "owl.openVst3Editor" | "openVst3Editor" => {
            let ch_idx = parse_int_field(&params, &["channel_idx", "channelId", "id"]).unwrap_or(0);
            let slot_idx = parse_int_field(&params, &["slot_idx", "slotId", "slot"]).unwrap_or(0);

            // Trigger Win32 container window creation
            let available = owl_core::scan_vst3_plugins();
            let mut opened_hwnd = 0usize;

            if let Some(ch) = state.find_channel_mut(ch_idx) {
                if slot_idx < ch.vst3_slots.len() {
                    let slot = &ch.vst3_slots[slot_idx];
                    let p_name = slot.plugin_name.clone().unwrap_or_else(|| "Parametric EQ".to_string());
                    if let Some(info) = available.into_iter().find(|p| p.name == p_name) {
                        let mut host = owl_core::Vst3PluginHost::load(info, 48000.0, 512);
                        if let Ok(hwnd) = host.open_editor(&ch.name) {
                            opened_hwnd = hwnd.0 as usize;
                        }
                    }
                }
            }

            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "success": true,
                    "channel_idx": ch_idx,
                    "slot_idx": slot_idx,
                    "hwnd": opened_hwnd
                })),
                error: None,
            };
            serde_json::to_string(&resp).ok()
        }

        _ => {
            let resp = JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: '{method}'"),
                }),
            };
            serde_json::to_string(&resp).ok()
        }
    }
}

/// `owl.addChannel` — create a channel, start its capture stream, register it in the matrix.
async fn rpc_add_channel(
    id: Option<serde_json::Value>,
    params: &serde_json::Value,
    state_lock: &Arc<tokio::sync::RwLock<ServerState>>,
) -> Option<String> {
    let name = parse_str_field(params, &["name"]).unwrap_or_else(|| "New Channel".to_string());
    let channel_type = parse_str_field(params, &["channel_type", "channelType"])
        .unwrap_or_else(|| "SoftwareProcess".to_string());
    let pid = parse_int_field(params, &["pid"]).map(|p| p as u32).unwrap_or(0);

    let is_mic = channel_type.eq_ignore_ascii_case("Microphone");
    let kind = if is_mic {
        CaptureKind::Microphone
    } else {
        CaptureKind::Process { pid }
    };

    // Phase 1: reserve the id and publish the channel under the lock, then release it.
    let (channel_id, supervisor) = {
        let mut state = state_lock.write().await;
        let channel_id = state.allocate_channel_id();
        state.add_channel(
            channel_id,
            name.clone(),
            if is_mic { "Microphone" } else { "SoftwareProcess" },
            pid,
            1.0,
            false,
        );
        state.broadcast_channels_list();
        (channel_id, state.supervisor.clone())
    };

    // Phase 2: start the real capture stream with the lock released (blocking WASAPI work).
    let mut started = false;
    let mut message = None;
    if let Some(supervisor) = supervisor {
        let spec = ChannelSpec {
            channel_id,
            name: name.clone(),
            kind,
        };
        match supervisor.add_channel(spec).await {
            Ok(ready) => {
                started = true;
                info!(
                    "addChannel: channel {channel_id} '{name}' capture started at {} Hz",
                    ready.sample_rate
                );
            }
            Err(err) => {
                warn!("addChannel: channel {channel_id} '{name}' capture failed: {err}");
                // Roll back: a channel that produces no audio must not appear in the UI as if it
                // worked. That is precisely the bug this daemon is fixing.
                let mut state = state_lock.write().await;
                state.channels.retain(|c| c.id != channel_id);
                state.broadcast_channels_list();
                message = Some(err);
            }
        }
    } else {
        // No audio core attached: no stream can be created, so the channel must not linger in
        // engine state as if it worked. Roll it back exactly like a failed capture start.
        message = Some("capture supervisor unavailable; channel registered without a stream".to_string());
        let mut state = state_lock.write().await;
        state.channels.retain(|c| c.id != channel_id);
        state.broadcast_channels_list();
    }

    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        // `success` must mean "a capture is actually running", not merely "the request was
        // understood". Reporting `true` while `capture_started` is false is exactly the
        // silent-failure the daemon is meant to eliminate: a client checking only `success`
        // would believe a channel that can never produce audio is live.
        result: Some(serde_json::json!({
            "success": started,
            "channel_id": channel_id,
            "capture_started": started,
            "message": message,
        })),
        error: None,
    };
    serde_json::to_string(&resp).ok()
}

/// `owl.removeChannel` — stop the capture stream and drop the channel from the matrix.
async fn rpc_remove_channel(
    id: Option<serde_json::Value>,
    params: &serde_json::Value,
    state_lock: &Arc<tokio::sync::RwLock<ServerState>>,
) -> Option<String> {
    let channel_id = parse_int_field(params, &["channel_id", "channelId", "channel_idx", "id"])
        .unwrap_or(0) as u32;

    // Resolve to a real channel id first: the UI may still send an index for the legacy rows.
    let (resolved_id, supervisor, removed) = {
        let mut state = state_lock.write().await;
        let resolved_id = state
            .find_channel_mut(channel_id as usize)
            .map(|c| c.id)
            .unwrap_or(channel_id);
        let before = state.channels.len();
        state.channels.retain(|c| c.id != resolved_id);
        let removed = state.channels.len() != before;
        (resolved_id, state.supervisor.clone(), removed)
    };

    if let Some(supervisor) = supervisor {
        if let Err(err) = supervisor.remove_channel(resolved_id).await {
            warn!("removeChannel: capture teardown failed for channel {resolved_id}: {err}");
        }
    }

    if removed {
        let state = state_lock.read().await;
        state.broadcast_channels_list();
    }

    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(serde_json::json!({
            "success": removed,
            "channel_id": resolved_id,
        })),
        error: None,
    };
    serde_json::to_string(&resp).ok()
}

/// `owl.createCable` — create (or rename) a virtual audio cable.
///
/// This is the entry point the UI calls when the user creates an input channel
/// or a mix: a cable with the same name appears in Windows Sound as both a
/// playback and a recording endpoint.
///
/// The cable set is the *whole* set the server exports, because the USB/IP
/// server exports a fixed device list for its lifetime. The caller therefore
/// passes the complete desired list, and the manager restarts the server with
/// it. Cables already registered as persistent devices are restored by the
/// driver automatically, so this needs no elevation after the first attach.
///
/// `names` is the preferred parameter (a full list). `name` is accepted as a
/// convenience for adding a single cable to the current set.
async fn rpc_create_cable(
    id: Option<serde_json::Value>,
    params: &serde_json::Value,
    state_lock: &Arc<tokio::sync::RwLock<ServerState>>,
) -> Option<String> {
    let manager = {
        let state = state_lock.read().await;
        state.cables.clone()
    };

    let Some(manager) = manager else {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(serde_json::json!({
                "success": false,
                "message": "virtual cable support is unavailable: virtualcables.exe or usbip.exe \
                            was not found. Install the usbip-win2 driver and place \
                            virtualcables.exe next to the daemon.",
            })),
            error: None,
        };
        return serde_json::to_string(&resp).ok();
    };

    // Either an explicit full list, or the current set plus one new name.
    let requested: Vec<String> = match params.get("names").and_then(|v| v.as_array()) {
        Some(array) => array
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        None => {
            let mut names = manager.cable_names();
            if let Some(name) = parse_str_field(params, &["name"]) {
                if !names.iter().any(|n| n == &name) {
                    names.push(name);
                }
            }
            names
        }
    };

    if requested.is_empty() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(serde_json::json!({
                "success": false,
                "message": "no cable names were provided",
            })),
            error: None,
        };
        return serde_json::to_string(&resp).ok();
    }

    // The server restart and the endpoint enumeration are blocking OS work, so
    // they run off the async runtime rather than stalling the RPC loop.
    let manager_for_task = Arc::clone(&manager);
    let names_for_task = requested.clone();
    let created = tokio::task::spawn_blocking(move || {
        manager_for_task.set_cables(&names_for_task)?;
        // Resolve the endpoints so the caller can bind to them immediately.
        let mut resolved = Vec::new();
        for name in &names_for_task {
            match manager_for_task.resolve_endpoints(name, std::time::Duration::from_secs(20)) {
                Ok(endpoints) => resolved.push(serde_json::json!({
                    "name": name,
                    "render": endpoints.render,
                    "capture": endpoints.capture,
                })),
                Err(err) => {
                    warn!("createCable: cable '{name}' did not produce endpoints: {err}");
                    resolved.push(serde_json::json!({
                        "name": name,
                        "render": serde_json::Value::Null,
                        "capture": serde_json::Value::Null,
                        "error": err.to_string(),
                    }));
                }
            }
        }
        Ok::<_, owl_core::CableError>(resolved)
    })
    .await;

    let resp = match created {
        Ok(Ok(cables)) => {
            let all_ready = cables.iter().all(|c| {
                let is_out = c["name"].as_str().map(|n| n.ends_with(":out")).unwrap_or(false);
                !c["render"].is_null() && (is_out || !c["capture"].is_null())
            });
            info!("createCable: {} cable(s) exported, all_ready={all_ready}", cables.len());
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "success": all_ready,
                    "cables": cables,
                })),
                error: None,
            }
        }
        Ok(Err(err)) => {
            warn!("createCable failed: {err}");
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "success": false,
                    "message": err.to_string(),
                })),
                error: None,
            }
        }
        Err(join_err) => {
            warn!("createCable task failed: {join_err}");
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(serde_json::json!({
                    "success": false,
                    "message": format!("cable task failed: {join_err}"),
                })),
                error: None,
            }
        }
    };
    serde_json::to_string(&resp).ok()
}

/// `owl.listCables` — names of the cables the daemon currently exports.
async fn rpc_list_cables(
    id: Option<serde_json::Value>,
    state_lock: &Arc<tokio::sync::RwLock<ServerState>>,
) -> Option<String> {
    let (available, names) = {
        let state = state_lock.read().await;
        match &state.cables {
            Some(manager) => (true, manager.cable_names()),
            None => (false, Vec::new()),
        }
    };

    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(serde_json::json!({
            "available": available,
            "cables": names,
        })),
        error: None,
    };
    serde_json::to_string(&resp).ok()
}

/// `owl.setOutputDevice` — bind the Monitor mix to a specific render endpoint.
///
/// This is the primary fix for "no sound": the system default render endpoint on this machine is
/// `Spotify (Elgato Virtual Audio)`, a virtual cable the user cannot hear.
async fn rpc_set_output_device(
    id: Option<serde_json::Value>,
    params: &serde_json::Value,
    state_lock: &Arc<tokio::sync::RwLock<ServerState>>,
) -> Option<String> {
    // Accept `endpoint_id` plus the camelCase and legacy `device_id` spellings.
    let endpoint_id = parse_str_field(params, &["endpoint_id", "endpointId", "device_id", "deviceId"])
        .filter(|s| !s.trim().is_empty());

    let supervisor = {
        let state = state_lock.read().await;
        state.supervisor.clone()
    };

    let Some(supervisor) = supervisor else {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(serde_json::json!({
                "success": false,
                "endpoint_id": endpoint_id.clone().unwrap_or_default(),
                "device_name": "",
                "message": "capture supervisor unavailable",
            })),
            error: None,
        };
        return serde_json::to_string(&resp).ok();
    };

    let result = supervisor.set_output_device(endpoint_id.clone()).await;

    let (success, device_id, device_name, message) = match result {
        Ok(device) => {
            let mut state = state_lock.write().await;
            state.output_device_id = device.id.clone();
            state.output_device_name = device.name.clone();
            let snapshot = serde_json::json!({
                "output_device_id": device.id,
                "output_device_name": device.name,
            });
            state.broadcast_notification("outputDeviceChanged", snapshot);
            info!("setOutputDevice: bound '{}'", device.name);
            (true, device.id, device.name, None)
        }
        Err(err) => {
            warn!("setOutputDevice failed: {err}");
            (
                false,
                endpoint_id.clone().unwrap_or_default(),
                String::new(),
                Some(err),
            )
        }
    };

    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(serde_json::json!({
            "success": success,
            "endpoint_id": device_id,
            "device_name": device_name,
            "message": message,
        })),
        error: None,
    };
    serde_json::to_string(&resp).ok()
}

fn parse_int_field(params: &serde_json::Value, keys: &[&str]) -> Option<usize> {
    for k in keys {
        if let Some(v) = params.get(k) {
            if let Some(n) = v.as_u64() {
                return Some(n as usize);
            }
            if let Some(n) = v.as_i64() {
                return Some(n as usize);
            }
        }
    }
    None
}

fn parse_float_field(params: &serde_json::Value, keys: &[&str]) -> Option<f32> {
    for k in keys {
        if let Some(v) = params.get(k)
            && let Some(n) = v.as_f64()
        {
            return Some(n as f32);
        }
    }
    None
}

fn parse_bool_field(params: &serde_json::Value, keys: &[&str]) -> Option<bool> {
    for k in keys {
        if let Some(v) = params.get(k)
            && let Some(b) = v.as_bool()
        {
            return Some(b);
        }
    }
    None
}

fn parse_str_field(params: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = params.get(k)
            && let Some(s) = v.as_str()
        {
            return Some(s.to_string());
        }
    }
    None
}

/// Write `%APPDATA%\OpenWaveLink\ws-info.json`
pub fn write_ws_info(port: u16) -> Result<PathBuf> {
    let base_dir = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));

    let dir = base_dir.join("OpenWaveLink");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("ws-info.json");

    let info = serde_json::json!({
        "port": port,
        "ws_url": format!("ws://127.0.0.1:{port}"),
        "pid": std::process::id(),
    });

    std::fs::write(&path, serde_json::to_string_pretty(&info)?)
        .context(format!("Failed to write {}", path.display()))?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;

    #[tokio::test]
    async fn test_ws_info_file_generation() {
        let temp_port = 19999;
        let path = write_ws_info(temp_port).expect("write ws info");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).expect("read ws info");
        assert!(content.contains("19999"));
        assert!(content.contains("ws://127.0.0.1:19999"));
    }

    #[tokio::test]
    async fn test_rpc_methods_handling() {
        let (prod, mut cons) = rtrb::RingBuffer::new(32);
        let (mut state, _) = ServerState::new(prod);
        state.add_channel(1, "Test Music", "SoftwareProcess", 1234, 0.5, false);

        let state_arc = Arc::new(tokio::sync::RwLock::new(state));

        // 1. owl.getChannels
        let req1 = r#"{"jsonrpc": "2.0", "id": 1, "method": "owl.getChannels"}"#;
        let res1 = handle_rpc(req1, &state_arc).await.expect("res1");
        assert!(res1.contains("Test Music"));
        assert!(res1.contains("1234"));

        // 2. owl.getMixes
        let req2 = r#"{"jsonrpc": "2.0", "id": 2, "method": "owl.getMixes"}"#;
        let res2 = handle_rpc(req2, &state_arc).await.expect("res2");
        assert!(res2.contains("Stream Mix"));
        assert!(res2.contains("Monitor Mix"));

        // 3. owl.setChannel
        let req3 = r#"{"jsonrpc": "2.0", "id": 3, "method": "owl.setChannel", "params": {"channel_idx": 0, "gain": 0.85, "is_muted": true}}"#;
        let res3 = handle_rpc(req3, &state_arc).await.expect("res3");
        assert!(res3.contains("success"));

        // Verify command popped in rtrb queue
        let cmd1 = cons.pop().expect("pop command 1");
        assert_eq!(
            cmd1,
            MatrixCommand::SetChannelGain {
                channel_idx: 0,
                slider: 0.85
            }
        );
        let cmd2 = cons.pop().expect("pop command 2");
        assert_eq!(
            cmd2,
            MatrixCommand::SetChannelMute {
                channel_idx: 0,
                muted: true
            }
        );

        // 4. owl.setSend
        let req4 = r#"{"jsonrpc": "2.0", "id": 4, "method": "owl.setSend", "params": {"channel_idx": 0, "mix_idx": 1, "gain": 0.40}}"#;
        let _ = handle_rpc(req4, &state_arc).await.expect("res4");
        let cmd3 = cons.pop().expect("pop command 3");
        assert_eq!(
            cmd3,
            MatrixCommand::SetSendGain {
                channel_idx: 0,
                mix_idx: 1,
                slider: 0.40
            }
        );

        // 5. owl.setMix
        let req5 = r#"{"jsonrpc": "2.0", "id": 5, "method": "owl.setMix", "params": {"mix_idx": 0, "gain": 0.95, "is_muted": false}}"#;
        let _ = handle_rpc(req5, &state_arc).await.expect("res5");
        let cmd4_gain = cons.pop().expect("pop command 4 (gain)");
        assert_eq!(
            cmd4_gain,
            MatrixCommand::SetMixGain {
                mix_idx: 0,
                slider: 0.95
            }
        );
        let cmd4_mute = cons.pop().expect("pop command 4 (mute)");
        assert_eq!(
            cmd4_mute,
            MatrixCommand::SetMixMute {
                mix_idx: 0,
                muted: false
            }
        );

        // 6. owl.assignProcess
        let req6 = r#"{"jsonrpc": "2.0", "id": 6, "method": "owl.assignProcess", "params": {"channel_idx": 0, "pid": 4321, "name": "cs2.exe"}}"#;
        let _ = handle_rpc(req6, &state_arc).await.expect("res6");
        let cmd5 = cons.pop().expect("pop command 5");
        assert_eq!(
            cmd5,
            MatrixCommand::AssignProcess {
                channel_idx: 0,
                pid: 4321
            }
        );

        // 7. owl.getVst3Plugins
        let req7 = r#"{"jsonrpc": "2.0", "id": 7, "method": "owl.getVst3Plugins"}"#;
        let res7 = handle_rpc(req7, &state_arc).await.expect("res7");
        assert!(res7.contains("plugins"));
        assert!(res7.contains("OpenWaveLink Broadcast Noise Gate") || res7.contains("Noise Gate"));

        // 8. owl.setVst3Slot
        let req8 = r#"{"jsonrpc": "2.0", "id": 8, "method": "owl.setVst3Slot", "params": {"channel_idx": 0, "slot_idx": 0, "plugin_id": "owl_noise_gate", "is_bypassed": false, "dry_wet": 0.8}}"#;
        let res8 = handle_rpc(req8, &state_arc).await.expect("res8");
        assert!(res8.contains("success"));

        let cmd6_load = cons.pop().expect("pop vst3 load");
        assert_eq!(
            cmd6_load,
            MatrixCommand::LoadVst3Plugin {
                channel_idx: 0,
                slot_idx: 0,
                plugin_id: "owl_noise_gate".to_string()
            }
        );
        let cmd6_bypass = cons.pop().expect("pop vst3 bypass");
        assert_eq!(
            cmd6_bypass,
            MatrixCommand::SetVst3Bypass {
                channel_idx: 0,
                slot_idx: 0,
                bypassed: false
            }
        );
        let cmd6_drywet = cons.pop().expect("pop vst3 drywet");
        assert_eq!(
            cmd6_drywet,
            MatrixCommand::SetVst3DryWet {
                channel_idx: 0,
                slot_idx: 0,
                dry_wet: 0.8
            }
        );
    }

    #[tokio::test]
    async fn test_ws_server_network_roundtrip() {
        let (prod, _cons) = rtrb::RingBuffer::new(32);
        let (mut state, broadcast_tx) = ServerState::new(prod);
        state.add_channel(1, "Test Game", "SoftwareProcess", 5555, 1.0, false);

        let server = WsServer::bind(0, state, broadcast_tx).await.expect("bind server");
        let port = server.port;

        let server_handle = tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let ws_url = format!("ws://127.0.0.1:{port}");
        let (mut client_ws, _) = connect_async(&ws_url).await.expect("connect client");

        let rpc_req = r#"{"jsonrpc":"2.0","id":100,"method":"owl.getChannels"}"#;
        client_ws.send(Message::text(rpc_req)).await.expect("send rpc");

        let resp_msg = client_ws.next().await.expect("some message").expect("valid message");
        if let Message::Text(resp_text) = resp_msg {
            assert!(resp_text.contains("Test Game"));
            assert!(resp_text.contains("5555"));
            assert!(resp_text.contains("100"));
        } else {
            panic!("Expected Message::Text response, got {:?}", resp_msg);
        }

        let set_req = r#"{"jsonrpc":"2.0","id":101,"method":"owl.setChannel","params":{"channel_idx":0,"gain":0.77}}"#;
        client_ws.send(Message::text(set_req)).await.expect("send setChannel");

        let mut got_response = false;
        let mut got_notification = false;

        for _ in 0..2 {
            let msg = client_ws.next().await.expect("next msg").expect("valid msg");
            if let Message::Text(txt) = msg {
                if txt.contains("\"id\":101") {
                    got_response = true;
                }
                if txt.contains("channelChanged") {
                    got_notification = true;
                }
            }
        }

        assert!(got_response, "Must receive response for request id 101");
        assert!(got_notification, "Must receive channelChanged notification");

        server_handle.abort();
    }

    /// Channel ids must be allocated monotonically and never reused, and the channel list must be
    /// broadcast — even when no audio core is attached.
    ///
    /// With no supervisor no capture stream can exist, so the channel is rolled back and
    /// `success` is `false`. That is deliberate: a channel that can never produce audio must not
    /// appear in engine state as if it worked. The id counter still advances, which is what this
    /// test exists to pin.
    #[tokio::test]
    async fn test_add_remove_channel_lifecycle() {
        let (prod, _cons) = rtrb::RingBuffer::new(32);
        let (state, broadcast_tx) = ServerState::new(prod);
        let state_arc = Arc::new(tokio::sync::RwLock::new(state));
        let mut rx = broadcast_tx.subscribe();

        // addChannel allocates id 1, reports honestly, and rolls back (no audio core attached).
        let add = r#"{"jsonrpc":"2.0","id":10,"method":"owl.addChannel","params":{"name":"chrome.exe","channel_type":"SoftwareProcess","pid":777}}"#;
        let res = handle_rpc(add, &state_arc).await.expect("addChannel");
        assert!(res.contains("\"channel_id\":1"), "got: {res}");
        assert!(
            res.contains("\"capture_started\":false"),
            "no supervisor means no capture, got: {res}"
        );
        assert!(
            res.contains("\"success\":false"),
            "success must not claim a live channel, got: {res}"
        );

        // Two broadcasts follow: the optimistic add, then the rollback. Drain to the LAST one and
        // assert the rolled-back channel is gone from the final state.
        let mut last = String::new();
        while let Ok(note) = rx.try_recv() {
            assert!(note.contains("channelsListChanged"), "unexpected note: {note}");
            last = note;
        }
        assert!(
            !last.is_empty(),
            "a channelsListChanged broadcast must accompany the add and its rollback"
        );
        assert!(
            !last.contains("chrome.exe"),
            "the final broadcast must not list the rolled-back channel: {last}"
        );

        // getEngineState still exposes the contract §5 output-device fields.
        let engine = r#"{"jsonrpc":"2.0","id":11,"method":"owl.getEngineState"}"#;
        let res = handle_rpc(engine, &state_arc).await.expect("getEngineState");
        assert!(res.contains("output_device_id"), "got: {res}");
        assert!(res.contains("output_device_name"));
        assert!(
            !res.contains("chrome.exe"),
            "a channel whose capture never started must not appear, got: {res}"
        );

        // A second addChannel must get a fresh id, never reuse 1.
        let add2 = r#"{"jsonrpc":"2.0","id":12,"method":"owl.addChannel","params":{"name":"game.exe","channel_type":"SoftwareProcess","pid":888}}"#;
        let res = handle_rpc(add2, &state_arc).await.expect("addChannel 2");
        assert!(res.contains("\"channel_id\":2"), "got: {res}");

        // Removing an unknown channel reports failure rather than pretending.
        let rem2 = r#"{"jsonrpc":"2.0","id":14,"method":"owl.removeChannel","params":{"channel_id":999}}"#;
        let res = handle_rpc(rem2, &state_arc).await.expect("removeChannel 999");
        assert!(res.contains("\"success\":false"), "got: {res}");
    }

    /// Without a capture supervisor, `owl.addChannel` cannot start a stream and must say so.
    ///
    /// A client that checks only `success` must not be told a channel is live when it can never
    /// produce audio. This pins the honest-failure contract for the dead-PID case as well:
    /// `success` mirrors `capture_started`, never a blanket `true`.
    #[tokio::test]
    async fn test_add_channel_reports_failure_when_capture_cannot_start() {
        let (prod, _cons) = rtrb::RingBuffer::new(32);
        let (state, _) = ServerState::new(prod);
        let state_arc = Arc::new(tokio::sync::RwLock::new(state));

        // No supervisor is attached, so no capture stream can be created.
        let req = r#"{"jsonrpc":"2.0","id":30,"method":"owl.addChannel","params":{"name":"ghost.exe","channel_type":"SoftwareProcess","pid":999999}}"#;
        let res = handle_rpc(req, &state_arc).await.expect("addChannel");

        assert!(
            res.contains("\"capture_started\":false"),
            "a channel with no capture must report capture_started:false, got: {res}"
        );
        assert!(
            res.contains("\"success\":false"),
            "success must not claim a channel is live when capture never started, got: {res}"
        );

        // The channel must not linger in the engine state as if it worked.
        let engine = r#"{"jsonrpc":"2.0","id":31,"method":"owl.getEngineState"}"#;
        let res = handle_rpc(engine, &state_arc).await.expect("getEngineState");
        assert!(
            !res.contains("ghost.exe"),
            "a channel whose capture failed must not appear in engine state, got: {res}"
        );
    }

    /// `owl.setOutputDevice` must report failure honestly when no audio core is attached.
    #[tokio::test]
    async fn test_set_output_device_without_supervisor() {
        let (prod, _cons) = rtrb::RingBuffer::new(32);
        let (state, _) = ServerState::new(prod);
        let state_arc = Arc::new(tokio::sync::RwLock::new(state));

        let req = r#"{"jsonrpc":"2.0","id":20,"method":"owl.setOutputDevice","params":{"endpoint_id":"{0.0.0.00000000}.{cca3f6ac-59d9-4b55-ad5b-0f6c645e7f2a}"}}"#;
        let res = handle_rpc(req, &state_arc).await.expect("setOutputDevice");
        assert!(res.contains("\"success\":false"), "got: {res}");
        assert!(res.contains("device_name"));
    }

    /// `owl.getEngineState` must return the CONTRACT §5 shape exactly — the UI reads these keys
    /// directly, and a mix `id` that disagrees with `owl.setMix`'s `mix_idx` would make the UI
    /// address the wrong bus.
    #[tokio::test]
    async fn test_get_engine_state_shape_matches_contract() {
        let (prod, _cons) = rtrb::RingBuffer::new(32);
        let (mut state, _) = ServerState::new(prod);
        state.add_channel(1, "chrome.exe", "SoftwareProcess", 4242, 0.75, false);
        state.output_device_id = "{0.0.0.00000000}.{cca3f6ac-59d9-4b55-ad5b-0f6c645e7f2a}".to_string();
        state.output_device_name = "Speakers (Sound Blaster Play! 3)".to_string();
        let state_arc = Arc::new(tokio::sync::RwLock::new(state));

        let req = r#"{"jsonrpc":"2.0","id":30,"method":"owl.getEngineState"}"#;
        let res = handle_rpc(req, &state_arc).await.expect("getEngineState");
        let v: serde_json::Value = serde_json::from_str(&res).expect("valid json");
        let result = &v["result"];

        // Channel keys required by EngineStateDto.
        let ch = &result["channels"][0];
        for key in ["id", "name", "channel_type", "pid", "gain", "is_muted"] {
            assert!(!ch[key].is_null(), "channel is missing key '{key}': {ch}");
        }
        assert_eq!(ch["id"], 1);
        assert_eq!(ch["name"], "chrome.exe");
        assert_eq!(ch["channel_type"], "SoftwareProcess");
        assert_eq!(ch["pid"], 4242);
        assert_eq!(ch["gain"], 0.75);

        // Mix keys, and the reported `id` must equal the `mix_idx` that `owl.setMix` accepts.
        assert!(result["mixes"].is_array());
        let mixes = result["mixes"].as_array().expect("mixes array");
        assert_eq!(mixes.len(), 5, "all five mix buses must be reported");
        for (idx, mix) in mixes.iter().enumerate() {
            for key in ["id", "name", "gain", "is_muted"] {
                assert!(!mix[key].is_null(), "mix is missing key '{key}': {mix}");
            }
            assert_eq!(
                mix["id"].as_u64(),
                Some(idx as u64),
                "mix id must equal the mix_idx accepted by owl.setMix"
            );
        }
        assert_eq!(mixes[0]["name"], "Stream Mix");
        assert_eq!(mixes[1]["name"], "Monitor Mix");

        // The ACTUALLY bound output device, not the requested one.
        assert_eq!(result["output_device_name"], "Speakers (Sound Blaster Play! 3)");
        assert_eq!(
            result["output_device_id"],
            "{0.0.0.00000000}.{cca3f6ac-59d9-4b55-ad5b-0f6c645e7f2a}"
        );

        // The reported mix id must be accepted by owl.setMix and reach the matrix verbatim.
        let (prod2, mut cons2) = rtrb::RingBuffer::new(32);
        let (state2, _) = ServerState::new(prod2);
        let state2 = Arc::new(tokio::sync::RwLock::new(state2));
        let reported_id = mixes[1]["id"].as_u64().expect("mix id");
        let set_mix = format!(
            r#"{{"jsonrpc":"2.0","id":31,"method":"owl.setMix","params":{{"mix_idx":{reported_id},"gain":0.5}}}}"#
        );
        let _ = handle_rpc(&set_mix, &state2).await.expect("setMix");
        assert_eq!(
            cons2.pop().expect("setMix command"),
            MatrixCommand::SetMixGain {
                mix_idx: reported_id as usize,
                slider: 0.5
            },
            "the mix id reported by getEngineState must address that same bus in owl.setMix"
        );
    }

    /// ID-first lookup: `channel_id == 2` must address the channel whose id is 2, not the row at
    /// index 2 — the bug that appears as soon as a channel is deleted.
    #[tokio::test]
    async fn test_channel_lookup_is_id_first() {        let (prod, _cons) = rtrb::RingBuffer::new(32);
        let (mut state, _) = ServerState::new(prod);
        state.add_channel(1, "one", "SoftwareProcess", 0, 1.0, false);
        state.add_channel(2, "two", "SoftwareProcess", 0, 1.0, false);
        state.add_channel(5, "five", "SoftwareProcess", 0, 1.0, false);

        // id 2 lives at index 1; index 2 holds id 5. ID-first must return "two".
        assert_eq!(state.find_channel_mut(2).map(|c| c.name.clone()), Some("two".to_string()));
        assert_eq!(state.find_channel_mut(5).map(|c| c.name.clone()), Some("five".to_string()));
        // Legacy index addressing for an id that does not exist still resolves by position.
        assert_eq!(state.find_channel_mut(0).map(|c| c.name.clone()), Some("one".to_string()));
    }
}
