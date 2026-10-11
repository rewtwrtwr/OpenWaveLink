//! Localhost JSON-RPC 2.0 WebSocket Client for owl-daemon (ws://127.0.0.1:1824)
//!
//! Provides two-way communication between Tauri UI and the Audio Core Daemon.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

pub const DEFAULT_DAEMON_WS_URL: &str = "ws://127.0.0.1:1824";

/// How long a request/response RPC call waits for the daemon's reply.
const RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// Pending request/response calls, keyed by JSON-RPC request id.
type PendingMap = Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(dead_code)]
pub struct ChannelInfoDto {
    pub id: u32,
    pub name: String,
    pub channel_type: String,
    pub pid: u32,
    pub gain: f32,
    pub is_muted: bool,
    pub sends: Vec<SendInfoDto>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(dead_code)]
pub struct SendInfoDto {
    pub mix_id: usize,
    pub name: String,
    pub gain: f32,
    pub is_muted: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(dead_code)]
pub struct MixInfoDto {
    pub id: usize,
    pub name: String,
    pub gain: f32,
    pub is_muted: bool,
}

#[derive(Clone)]
pub struct RpcClient {
    tx: mpsc::Sender<String>,
    req_id: Arc<AtomicU64>,
    pending: PendingMap,
}

impl RpcClient {
    pub fn new(app_handle: AppHandle) -> Self {
        let (tx, mut rx) = mpsc::channel::<String>(256);
        let req_id = Arc::new(AtomicU64::new(1));
        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));

        let app_clone = app_handle.clone();
        let pending_reader = Arc::clone(&pending);
        let req_id_writer = Arc::clone(&req_id);
        tauri::async_runtime::spawn(async move {
            loop {
                info!("Attempting connection to owl-daemon at {}", DEFAULT_DAEMON_WS_URL);
                match connect_async(DEFAULT_DAEMON_WS_URL).await {
                    Ok((ws_stream, _)) => {
                        info!("Connected to owl-daemon WebSocket");
                        let _ = app_clone.emit("daemon-connection-status", true);

                        let (mut write, mut read) = ws_stream.split();

                        // Warm-up queries. These MUST take their ids from the same counter as
                        // `call()`. Using literal ids here would collide with the first
                        // `call()` (whose counter also starts at 1), and the bootstrap reply
                        // would then be delivered to an unrelated pending waiter — the UI would
                        // receive `owl.getChannels` data where it expected `owl.getEngineState`
                        // and silently fall back to stale local state.
                        for method in ["owl.getChannels", "owl.getMixes"] {
                            let id = req_id_writer.fetch_add(1, Ordering::SeqCst);
                            let req = json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "method": method
                            });
                            let _ = write.send(Message::Text(req.to_string().into())).await;
                        }

                        loop {
                            tokio::select! {
                                Some(outgoing) = rx.recv() => {
                                    if let Err(e) = write.send(Message::Text(outgoing.into())).await {
                                        error!("Failed to send WS message: {e}");
                                        break;
                                    }
                                }
                                incoming = read.next() => {
                                    match incoming {
                                        Some(Ok(Message::Text(text))) => {
                                            // Resolve a pending request/response call if this
                                            // frame is a reply carrying a matching id.
                                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                                                if value.get("method").is_none() {
                                                    if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                                                        let waiter = pending_reader
                                                            .lock()
                                                            .ok()
                                                            .and_then(|mut map| map.remove(&id));
                                                        if let Some(tx) = waiter {
                                                            let _ = tx.send(value);
                                                        }
                                                    }
                                                }
                                            }
                                            // Forward daemon notification/response to WebView
                                            let _ = app_clone.emit("daemon-rpc-message", text.to_string());
                                        }
                                        Some(Ok(Message::Close(_))) | None => {
                                            warn!("Daemon WS connection closed");
                                            break;
                                        }
                                        Some(Err(e)) => {
                                            error!("Daemon WS error: {e}");
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }

                        let _ = app_clone.emit("daemon-connection-status", false);
                    }
                    Err(_) => {
                        // Daemon not yet running; retry backoff
                        let _ = app_clone.emit("daemon-connection-status", false);
                    }
                }

                // The socket is gone: fail every in-flight call instead of hanging.
                if let Ok(mut map) = pending_reader.lock() {
                    map.clear();
                }

                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
        });

        Self { tx, req_id, pending }
    }

    /// Send a JSON-RPC request and await the daemon's reply.
    ///
    /// Used for calls whose result the UI needs (a newly assigned `channel_id`,
    /// a resolved output-device name). Times out after [`RPC_TIMEOUT`] so the UI
    /// never hangs forever when the daemon is absent.
    pub async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let id = self.req_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        if let Ok(mut map) = self.pending.lock() {
            map.insert(id, tx);
        } else {
            anyhow::bail!("RPC pending map is poisoned");
        }

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        if let Err(e) = self.tx.send(req.to_string()).await {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&id);
            }
            return Err(anyhow::Error::new(e).context("Failed to queue RPC message"));
        }

        match tokio::time::timeout(RPC_TIMEOUT, rx).await {
            Ok(Ok(value)) => {
                if let Some(err) = value.get("error") {
                    anyhow::bail!("daemon returned error for {method}: {err}");
                }
                Ok(value.get("result").cloned().unwrap_or(serde_json::Value::Null))
            }
            Ok(Err(_)) => anyhow::bail!("daemon dropped the request for {method}"),
            Err(_) => {
                if let Ok(mut map) = self.pending.lock() {
                    map.remove(&id);
                }
                anyhow::bail!("daemon did not answer {method} within {:?}", RPC_TIMEOUT)
            }
        }
    }

    pub async fn send_rpc(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let id = self.req_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.tx
            .send(req.to_string())
            .await
            .context("Failed to queue RPC message")?;
        Ok(())
    }

    pub async fn set_channel_gain(&self, channel_idx: usize, gain: f32) -> Result<()> {
        self.send_rpc("owl.setChannel", json!({ "channel_idx": channel_idx, "gain": gain }))
            .await
    }

    pub async fn set_channel_mute(&self, channel_idx: usize, is_muted: bool) -> Result<()> {
        self.send_rpc("owl.setChannel", json!({ "channel_idx": channel_idx, "is_muted": is_muted }))
            .await
    }

    pub async fn set_send_gain(
        &self,
        channel_idx: usize,
        mix_idx: usize,
        gain: f32,
    ) -> Result<()> {
        self.send_rpc(
            "owl.setSend",
            json!({ "channel_idx": channel_idx, "mix_idx": mix_idx, "gain": gain }),
        )
        .await
    }

    pub async fn set_send_mute(
        &self,
        channel_idx: usize,
        mix_idx: usize,
        is_muted: bool,
    ) -> Result<()> {
        self.send_rpc(
            "owl.setSend",
            json!({ "channel_idx": channel_idx, "mix_idx": mix_idx, "is_muted": is_muted }),
        )
        .await
    }

    pub async fn set_mix_gain(&self, mix_idx: usize, gain: f32) -> Result<()> {
        self.send_rpc("owl.setMix", json!({ "mix_idx": mix_idx, "gain": gain }))
            .await
    }

    pub async fn set_mix_mute(&self, mix_idx: usize, is_muted: bool) -> Result<()> {
        self.send_rpc("owl.setMix", json!({ "mix_idx": mix_idx, "is_muted": is_muted }))
            .await
    }

    pub async fn assign_process(&self, channel_idx: usize, pid: u32, name: &str) -> Result<()> {
        self.send_rpc(
            "owl.assignProcess",
            json!({ "channel_idx": channel_idx, "pid": pid, "name": name }),
        )
        .await
    }

    /// Create a real input channel in the audio engine.
    ///
    /// For a `SoftwareProcess` channel this genuinely starts a Process-Loopback
    /// capture for `pid` on the daemon side. Returns the daemon-assigned channel id.
    pub async fn add_channel(&self, name: &str, channel_type: &str, pid: u32) -> Result<u32> {
        let result = self
            .call(
                "owl.addChannel",
                json!({ "name": name, "channel_type": channel_type, "pid": pid }),
            )
            .await?;

        result
            .get("channel_id")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .context("owl.addChannel reply did not contain a numeric channel_id")
    }

    /// Remove a channel and stop its capture.
    pub async fn remove_channel(&self, channel_id: u32) -> Result<bool> {
        let result = self
            .call("owl.removeChannel", json!({ "channel_id": channel_id }))
            .await?;

        Ok(result
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Create (or rename) virtual audio cables so they appear in Windows Sound.
    ///
    /// `names` is the complete desired cable set: the daemon restarts its
    /// USB/IP server with exactly these cables. Returns the endpoint ids each
    /// cable produced, so the UI can bind a channel or mix to them immediately.
    pub async fn create_cables(&self, names: &[String]) -> Result<Vec<CableEndpointsDto>> {
        let result = self
            .call("owl.createCable", json!({ "names": names }))
            .await?;

        let success = result
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !success {
            let message = result
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("the daemon could not create the virtual cables");
            anyhow::bail!("{message}");
        }

        let cables = result
            .get("cables")
            .and_then(|v| v.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|entry| {
                        let name = entry.get("name")?.as_str()?.to_string();
                        let render = entry
                            .get("render")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let capture = entry
                            .get("capture")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        Some(CableEndpointsDto {
                            name,
                            render,
                            capture,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(cables)
    }

    /// Names of the virtual cables the daemon currently exports.
    pub async fn list_cables(&self) -> Result<Vec<String>> {
        let result = self.call("owl.listCables", json!({})).await?;

        Ok(result
            .get("cables")
            .and_then(|v| v.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Switch the physical WASAPI render endpoint used for the Monitor mix.
    ///
    /// Returns the human-readable name of the endpoint the daemon actually bound.
    pub async fn set_output_device(&self, endpoint_id: &str) -> Result<String> {
        let result = self
            .call("owl.setOutputDevice", json!({ "endpoint_id": endpoint_id }))
            .await?;

        Ok(result
            .get("device_name")
            .and_then(|v| v.as_str())
            .unwrap_or(endpoint_id)
            .to_string())
    }

    pub async fn set_vst3_slot(
        &self,
        channel_idx: usize,
        slot_idx: usize,
        plugin_id: Option<String>,
        is_bypassed: Option<bool>,
        dry_wet: Option<f32>,
        clear: Option<bool>,
    ) -> Result<()> {
        let mut params = json!({
            "channel_idx": channel_idx,
            "slot_idx": slot_idx,
        });
        if let Some(pid) = plugin_id {
            params["plugin_id"] = json!(pid);
        }
        if let Some(b) = is_bypassed {
            params["is_bypassed"] = json!(b);
        }
        if let Some(dw) = dry_wet {
            params["dry_wet"] = json!(dw);
        }
        if let Some(c) = clear {
            params["clear"] = json!(c);
        }
        self.send_rpc("owl.setVst3Slot", params).await
    }

    pub async fn open_vst3_editor(&self, channel_idx: usize, slot_idx: usize) -> Result<()> {
        self.send_rpc(
            "owl.openVst3Editor",
            json!({ "channel_idx": channel_idx, "slot_idx": slot_idx }),
        )
        .await
    }
}

/// One virtual cable as reported by the daemon: its name plus the Windows
/// endpoint ids it produced. `render` is where audio is written; `capture` is
/// where other applications read it from.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CableEndpointsDto {
    pub name: String,
    pub render: Option<String>,
    pub capture: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_json_structure() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "owl.setChannel",
            "params": {
                "channel_idx": 1,
                "gain": 0.85
            }
        });

        assert_eq!(req["method"], "owl.setChannel");
        assert_eq!(req["params"]["channel_idx"], 1);
        assert!((req["params"]["gain"].as_f64().unwrap() - 0.85).abs() < 1e-4);
    }
}
