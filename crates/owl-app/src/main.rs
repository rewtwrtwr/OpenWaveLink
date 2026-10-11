// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hotkeys;
mod rpc_client;
mod telemetry;
mod win32_window;

use std::net::TcpStream;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use log::{info, warn};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};
use windows::Win32::Foundation::HWND;

const CREATE_NO_WINDOW: u32 = 0x08000000;

fn ensure_daemon_running() {
    // Check if daemon is already listening on 127.0.0.1:1824
    if TcpStream::connect_timeout(
        &"127.0.0.1:1824".parse().unwrap(),
        Duration::from_millis(200),
    ).is_ok() {
        info!("owl-daemon is already running on 127.0.0.1:1824");
        return;
    }

    // Collect every owl-daemon.exe we could launch.
    let mut candidates = Vec::new();
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            candidates.push(parent.join("owl-daemon.exe"));
            candidates.push(parent.join("..").join("owl-daemon.exe"));
        }
    }
    candidates.push(PathBuf::from("target/release/owl-daemon.exe"));
    candidates.push(PathBuf::from("target/debug/owl-daemon.exe"));
    candidates.push(PathBuf::from("installer/release_payload/owl-daemon.exe"));

    // Pick the NEWEST existing binary rather than a fixed preference order.
    //
    // A fixed order silently defeats the audio fix: after `cargo build --workspace`
    // the freshly built `target/debug/owl-daemon.exe` carries the new engine, but a
    // stale `target/release/owl-daemon.exe` from an earlier build would be launched
    // instead, and the UI would appear to prove that the fix does not work.
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    for candidate in candidates {
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(_, best_time)| mtime > *best_time) {
            best = Some((candidate, mtime));
        }
    }

    let Some((candidate, mtime)) = best else {
        warn!("owl-daemon.exe not found automatically. Please run owl-daemon separately.");
        return;
    };

    let age = std::time::SystemTime::now()
        .duration_since(mtime)
        .map(|d| format!("{}s old", d.as_secs()))
        .unwrap_or_else(|_| "timestamp in the future".to_string());

    info!(
        "Spawning background audio daemon: {} (built {}, {})",
        candidate.display(),
        humantime_or_raw(mtime),
        age
    );

    match std::process::Command::new(&candidate)
        .arg("--auto")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(child) => {
            info!("owl-daemon background process launched successfully");
            
            unsafe {
                use windows::Win32::System::JobObjects::{
                    CreateJobObjectW, SetInformationJobObject, AssignProcessToJobObject,
                    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                };
                use windows::Win32::Foundation::HANDLE;
                use std::os::windows::io::AsRawHandle;

                if let Ok(job_handle) = CreateJobObjectW(None, None) {
                    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                    
                    let res = SetInformationJobObject(
                        job_handle,
                        JobObjectExtendedLimitInformation,
                        &info as *const _ as *const std::ffi::c_void,
                        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    );
                    
                    if res.is_ok() {
                        let process_handle = HANDLE(child.as_raw_handle() as *mut _);
                        let _ = AssignProcessToJobObject(job_handle, process_handle);
                    }
                }
            }

            // Short wait for daemon to bind WebSocket
            std::thread::sleep(Duration::from_millis(300));
        }
        Err(e) => {
            warn!("Failed to spawn owl-daemon from {}: {e}", candidate.display());
        }
    }
}

/// Best-effort human-readable rendering of a filesystem timestamp.
fn humantime_or_raw(t: std::time::SystemTime) -> String {
    match t.duration_since(std::time::SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("{}s since epoch", d.as_secs()),
        Err(_) => "unknown".to_string(),
    }
}

use hotkeys::{HotkeyManager, HudNotification};
use rpc_client::RpcClient;
use telemetry::{TelemetryService, TelemetrySnapshotDto};

struct AppState {
    rpc: RpcClient,
    telemetry: Arc<TelemetryService>,
    _hotkeys: Arc<HotkeyManager>,
}

#[tauri::command]
async fn set_channel_gain(
    channel_idx: usize,
    gain: f32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .set_channel_gain(channel_idx, gain)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_channel_mute(
    channel_idx: usize,
    is_muted: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .set_channel_mute(channel_idx, is_muted)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_send_gain(
    channel_idx: usize,
    mix_idx: usize,
    gain: f32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .set_send_gain(channel_idx, mix_idx, gain)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_send_mute(
    channel_idx: usize,
    mix_idx: usize,
    is_muted: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .set_send_mute(channel_idx, mix_idx, is_muted)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_mix_gain(
    mix_idx: usize,
    gain: f32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .set_mix_gain(mix_idx, gain)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_mix_mute(
    mix_idx: usize,
    is_muted: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .set_mix_mute(mix_idx, is_muted)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn assign_process(
    channel_idx: usize,
    pid: u32,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .assign_process(channel_idx, pid, &name)
        .await
        .map_err(|e| e.to_string())
}

/// Create a real input channel in the audio engine and return its channel id.
///
/// For a `SoftwareProcess` channel the daemon starts a Process-Loopback capture
/// for `pid`, so audio begins flowing for a newly added app. Returns an error
/// when the daemon is unreachable or does not report a numeric `channel_id`.
#[tauri::command]
async fn add_channel(
    name: String,
    channel_type: String,
    pid: u32,
    state: State<'_, AppState>,
) -> Result<u32, String> {
    state
        .rpc
        .add_channel(&name, &channel_type, pid)
        .await
        .map_err(|e| e.to_string())
}

/// Remove a channel and stop its capture.
#[tauri::command]
async fn remove_channel(channel_id: u32, state: State<'_, AppState>) -> Result<bool, String> {
    state
        .rpc
        .remove_channel(channel_id)
        .await
        .map_err(|e| e.to_string())
}

/// Switch the physical playback endpoint (the `on <device>` selector).
///
/// `endpoint_id` is a real `IMMDeviceEnumerator` render endpoint id. The daemon
/// tears down the old `WasapiPlayback` and binds the new endpoint, so the monitor
/// mix is rendered to the selected physical device. Returns the resolved endpoint
/// name, or an error when the endpoint cannot be bound.
#[tauri::command]
async fn set_output_device(
    endpoint_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state
        .rpc
        .set_output_device(&endpoint_id)
        .await
        .map_err(|e| e.to_string())
}

/// Authoritative engine state: channels, mixes and the bound output device.
#[tauri::command]
async fn get_engine_state(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    state
        .rpc
        .call("owl.getEngineState", serde_json::json!({}))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_vst3_plugins() -> Result<Vec<owl_core::Vst3PluginInfo>, String> {
    Ok(owl_core::scan_vst3_plugins())
}

/// Create (or rename) virtual audio cables so they appear in Windows Sound.
///
/// `names` is the complete desired cable set. The daemon restarts its USB/IP
/// server with exactly these cables and returns the render/capture endpoint ids
/// each cable produced, so the caller can bind a channel or mix to them.
#[tauri::command]
async fn create_cables(
    names: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::rpc_client::CableEndpointsDto>, String> {
    state
        .rpc
        .create_cables(&names)
        .await
        .map_err(|e| e.to_string())
}

/// Names of the virtual cables the daemon currently exports.
#[tauri::command]
async fn list_cables(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    state.rpc.list_cables().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_vst3_slot(
    channel_idx: usize,
    slot_idx: usize,
    plugin_id: Option<String>,
    is_bypassed: Option<bool>,
    dry_wet: Option<f32>,
    clear: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .set_vst3_slot(channel_idx, slot_idx, plugin_id, is_bypassed, dry_wet, clear)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_vst3_editor(
    channel_idx: usize,
    slot_idx: usize,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .rpc
        .open_vst3_editor(channel_idx, slot_idx)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_telemetry_snapshot(state: State<'_, AppState>) -> Result<TelemetrySnapshotDto, String> {
    let tele_state = state.telemetry.state();
    let lock = tele_state.read().await;
    Ok(lock.latest_dto.clone())
}

#[tauri::command]
fn trigger_hud_overlay(
    title: String,
    detail: String,
    volume: f32,
    is_muted: bool,
    icon: String,
    app: AppHandle,
) {
    hotkeys::trigger_hud_notification(
        &app,
        HudNotification {
            title,
            detail,
            volume,
            is_muted,
            icon,
        },
    );
}

#[tauri::command]
async fn set_default_playback_device(endpoint_id: String) -> Result<bool, String> {
    owl_core::set_default_playback_endpoint(&endpoint_id).map(|_| true)
}

#[tauri::command]
async fn bind_app_audio(pid: u32, endpoint_id: String) -> Result<bool, String> {
    owl_core::set_app_audio_endpoint(pid, &endpoint_id).map(|_| true)
}

#[tauri::command]
async fn get_app_icon(pid: Option<u32>, process_name: Option<String>) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || {
        owl_core::apps::get_app_icon_data_url(pid, process_name.as_deref())
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_active_audio_sessions() -> Result<Vec<owl_core::AudioProcessDto>, String> {
    owl_core::get_active_audio_sessions().map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_system_audio_devices() -> Result<owl_core::SystemAudioDevicesDto, String> {
    owl_core::get_system_audio_devices()
}

fn get_documents_config_path() -> Result<std::path::PathBuf, String> {
    let docs_dir = std::env::var("USERPROFILE")
        .map(|p| std::path::PathBuf::from(p).join("Documents"))
        .or_else(|_| {
            std::env::var("HOME").map(|p| std::path::PathBuf::from(p).join("Documents"))
        })
        .map_err(|_| "Could not resolve Documents directory".to_string())?;

    let owl_dir = docs_dir.join("OpenWaveLink");
    std::fs::create_dir_all(&owl_dir).map_err(|e| e.to_string())?;
    Ok(owl_dir.join("config.json"))
}

#[tauri::command]
async fn save_app_config(config_json: String) -> Result<bool, String> {
    let path = get_documents_config_path()?;
    std::fs::write(&path, config_json.as_bytes()).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
async fn load_app_config() -> Result<Option<String>, String> {
    let path = get_documents_config_path()?;
    if path.is_file() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        Ok(Some(content))
    } else {
        Ok(None)
    }
}

#[tauri::command]
async fn get_installed_applications() -> Result<Vec<owl_core::InstalledAppDto>, String> {
    Ok(owl_core::get_installed_applications())
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    info!("Starting OpenWaveLink Tauri 2.x Application");

    ensure_daemon_running();

    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();

            // 1. Initialize Rpc Client to ws://127.0.0.1:1824
            let rpc = RpcClient::new(handle.clone());

            // 2. Initialize Telemetry Service reading Shared Memory
            let telemetry = Arc::new(TelemetryService::new());
            telemetry.start(handle.clone());

            // 3. Initialize Win32 Global Hotkeys and WH_MOUSE_LL hook
            let hotkeys = Arc::new(HotkeyManager::new());
            hotkeys.start(handle.clone());

            // 4. Store State
            app.manage(AppState {
                rpc,
                telemetry,
                _hotkeys: hotkeys,
            });

            // 5. Configure Windows (Mica Alt & Overlay Styles)
            if let Some(main_win) = app.get_webview_window("main") {
                if let Ok(hwnd) = main_win.hwnd() {
                    win32_window::apply_fluent_backdrop(HWND(hwnd.0 as *mut _), true);
                }
            }

            if let Some(flyout_win) = app.get_webview_window("flyout") {
                if let Ok(hwnd) = flyout_win.hwnd() {
                    win32_window::apply_fluent_backdrop(HWND(hwnd.0 as *mut _), false);
                }
            }

            // 6. Build System Tray Icon and Menu
            let toggle_main = MenuItem::with_id(app, "toggle_main", "Show Matrix Mixer", true, None::<&str>)?;
            let toggle_flyout = MenuItem::with_id(app, "toggle_flyout", "Quick Mixer (Flyout)", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Exit OpenWaveLink", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&toggle_main, &toggle_flyout, &quit_item])?;

            let _tray = TrayIconBuilder::with_id("owl-tray")
                .tooltip("OpenWaveLink - Windows 11 Audio Mixer")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "toggle_main" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.unminimize();
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "toggle_flyout" => {
                        toggle_flyout_window(app);
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        toggle_flyout_window(app);
                    }
                })
                .build(app)?;

            info!("OpenWaveLink Desktop setup completed successfully");
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "flyout" {
                if let WindowEvent::Focused(false) = event {
                    // Automatically hide Taskbar Flyout when losing focus
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            set_channel_gain,
            set_channel_mute,
            set_send_gain,
            set_send_mute,
            set_mix_gain,
            set_mix_mute,
            assign_process,
            add_channel,
            remove_channel,
            set_output_device,
            create_cables,
            list_cables,
            get_engine_state,
            get_telemetry_snapshot,
            trigger_hud_overlay,
            get_vst3_plugins,
            set_vst3_slot,
            open_vst3_editor,
            get_active_audio_sessions,
            get_system_audio_devices,
            get_installed_applications,
            bind_app_audio,
            get_app_icon,
            set_default_playback_device,
            save_app_config,
            load_app_config,
        ])
        .run(tauri::generate_context!())
        .expect("Failed to run OpenWaveLink Tauri application");
}

/// Show or hide Taskbar Flyout with neat placement near the system clock
fn toggle_flyout_window(app: &AppHandle) {
    if let Some(flyout) = app.get_webview_window("flyout") {
        if flyout.is_visible().unwrap_or(false) {
            let _ = flyout.hide();
        } else {
            if let Ok(hwnd) = flyout.hwnd() {
                win32_window::position_flyout_near_tray(HWND(hwnd.0 as *mut _), 360, 480);
            }
            let _ = flyout.show();
            let _ = flyout.set_focus();
        }
    }
}
