//! Low-Level Win32 Input Subsystem: Global Hotkeys & Mouse Hooks
//!
//! Handles:
//! - Win32 `RegisterHotKey` (Ctrl+Shift+M for instant mic mute, Ctrl+Shift+PgUp/Down for Monitor Mix)
//! - Low-level mouse hook `WH_MOUSE_LL` (Alt + MouseWheel volume adjust, Mouse 4/5 Push-to-Mute)
//! - Non-blocking dispatch to JSON-RPC Client and HUD Overlay Trigger.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use log::{error, info};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
    VK_MENU, VK_NEXT, VK_PRIOR,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, HHOOK, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_HOTKEY, WM_MOUSEWHEEL,
    WM_QUIT, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

pub const HOTKEY_ID_MIC_MUTE: i32 = 1001;
pub const HOTKEY_ID_VOL_UP: i32 = 1002;
pub const HOTKEY_ID_VOL_DOWN: i32 = 1003;

#[derive(Debug, Clone, Serialize)]
pub struct HudNotification {
    pub title: String,
    pub detail: String,
    pub volume: f32,
    pub is_muted: bool,
    pub icon: String, // "mic", "headphones", "game"
}

// Wrapper for thread-safe hook pointer storage
#[derive(Clone, Copy, Default)]
struct SafeHook(isize);
unsafe impl Send for SafeHook {}
unsafe impl Sync for SafeHook {}

static MOUSE_HOOK_HANDLE: Mutex<SafeHook> = Mutex::new(SafeHook(0));
static GLOBAL_APP_HANDLE: Mutex<Option<AppHandle>> = Mutex::new(None);

pub struct HotkeyManager {
    is_running: Arc<AtomicBool>,
    thread_id: Arc<Mutex<u32>>,
}

impl HotkeyManager {
    pub fn new() -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            thread_id: Arc::new(Mutex::new(0)),
        }
    }

    /// Start Win32 Message Loop with RegisterHotKey and WH_MOUSE_LL
    pub fn start(&self, app_handle: AppHandle) {
        if self.is_running.swap(true, Ordering::SeqCst) {
            return;
        }

        {
            let mut global_app = GLOBAL_APP_HANDLE.lock().unwrap();
            *global_app = Some(app_handle.clone());
        }

        let is_running = self.is_running.clone();
        let thread_id_arc = self.thread_id.clone();

        std::thread::Builder::new()
            .name("owl-win32-hotkeys".to_string())
            .spawn(move || {
                let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
                {
                    let mut lock = thread_id_arc.lock().unwrap();
                    *lock = tid;
                }

                info!("Initializing Win32 Hotkeys and WH_MOUSE_LL on thread ID {}", tid);

                // 1. Register Global Hotkeys
                unsafe {
                    // Ctrl + Shift + M -> Mic Mute
                    let _ = RegisterHotKey(
                        HWND(std::ptr::null_mut()),
                        HOTKEY_ID_MIC_MUTE,
                        MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                        0x4D, // 'M'
                    );

                    // Ctrl + Shift + Page Up -> Monitor Mix Volume Up
                    let _ = RegisterHotKey(
                        HWND(std::ptr::null_mut()),
                        HOTKEY_ID_VOL_UP,
                        MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                        VK_PRIOR.0 as u32,
                    );

                    // Ctrl + Shift + Page Down -> Monitor Mix Volume Down
                    let _ = RegisterHotKey(
                        HWND(std::ptr::null_mut()),
                        HOTKEY_ID_VOL_DOWN,
                        MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                        VK_NEXT.0 as u32,
                    );
                }

                // 2. Install WH_MOUSE_LL hook for Alt + Scroll and XButtons
                unsafe {
                    let hook_res = SetWindowsHookExW(
                        WH_MOUSE_LL,
                        Some(mouse_ll_proc),
                        HINSTANCE(std::ptr::null_mut()),
                        0,
                    );

                    match hook_res {
                        Ok(h) => {
                            info!("WH_MOUSE_LL low-level mouse hook successfully installed");
                            let mut lock = MOUSE_HOOK_HANDLE.lock().unwrap();
                            *lock = SafeHook(h.0 as isize);
                        }
                        Err(e) => {
                            error!("Failed to install WH_MOUSE_LL hook: {e}");
                        }
                    }
                }

                // 3. Message Loop
                let mut msg = MSG::default();
                while is_running.load(Ordering::Relaxed) {
                    let res = unsafe { GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0) };
                    if res.0 <= 0 {
                        break;
                    }

                    if msg.message == WM_HOTKEY {
                        let hotkey_id = msg.wParam.0 as i32;
                        handle_hotkey_event(hotkey_id, &app_handle);
                    }

                    unsafe {
                        DispatchMessageW(&msg);
                    }
                }

                // Cleanup
                unsafe {
                    let _ = UnregisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_ID_MIC_MUTE);
                    let _ = UnregisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_ID_VOL_UP);
                    let _ = UnregisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_ID_VOL_DOWN);

                    let mut lock = MOUSE_HOOK_HANDLE.lock().unwrap();
                    let raw_hook = lock.0;
                    if raw_hook != 0 {
                        let _ = UnhookWindowsHookEx(HHOOK(raw_hook as *mut _));
                        *lock = SafeHook(0);
                        info!("WH_MOUSE_LL hook uninstalled");
                    }
                }

                info!("Win32 Hotkey thread terminated cleanly");
            })
            .expect("Failed to spawn Hotkey worker thread");
    }

    #[allow(dead_code)]
    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
        let tid = *self.thread_id.lock().unwrap();
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Handle triggered Win32 HotKey
fn handle_hotkey_event(hotkey_id: i32, app_handle: &AppHandle) {
    match hotkey_id {
        HOTKEY_ID_MIC_MUTE => {
            info!("Global Hotkey triggered: [Ctrl+Shift+M] Mic Mute");
            trigger_hud_notification(
                app_handle,
                HudNotification {
                    title: "Microphone".to_string(),
                    detail: "Hardware Mute Toggled".to_string(),
                    volume: 0.0,
                    is_muted: true,
                    icon: "mic".to_string(),
                },
            );
            let _ = app_handle.emit("action-mic-mute-toggle", ());
        }
        HOTKEY_ID_VOL_UP => {
            info!("Global Hotkey triggered: [Ctrl+Shift+PgUp] Monitor Vol +2%");
            trigger_hud_notification(
                app_handle,
                HudNotification {
                    title: "Monitor Mix".to_string(),
                    detail: "+2% Step".to_string(),
                    volume: 0.82,
                    is_muted: false,
                    icon: "headphones".to_string(),
                },
            );
            let _ = app_handle.emit("action-monitor-vol-step", 0.02f32);
        }
        HOTKEY_ID_VOL_DOWN => {
            info!("Global Hotkey triggered: [Ctrl+Shift+PgDn] Monitor Vol -2%");
            trigger_hud_notification(
                app_handle,
                HudNotification {
                    title: "Monitor Mix".to_string(),
                    detail: "-2% Step".to_string(),
                    volume: 0.78,
                    is_muted: false,
                    icon: "headphones".to_string(),
                },
            );
            let _ = app_handle.emit("action-monitor-vol-step", -0.02f32);
        }
        _ => {}
    }
}

/// Show In-Game HUD overlay temporarily for 2 seconds (disabled per user request)
pub fn trigger_hud_notification(_app: &AppHandle, _notif: HudNotification) {
    // Intentionally disabled — overlay disabled by user request.
}

/// Low-Level Mouse Hook callback (WH_MOUSE_LL)
unsafe extern "system" fn mouse_ll_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code >= 0 {
        let msg_id = w_param.0 as u32;

        if msg_id == WM_MOUSEWHEEL {
            // Check if Alt key is pressed (VK_MENU)
            let is_alt_pressed = unsafe { (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 };

            if is_alt_pressed {
                let hook_struct = unsafe { *(l_param.0 as *const MSLLHOOKSTRUCT) };
                let delta = (hook_struct.mouseData >> 16) as i16;
                let step = if delta > 0 { 0.02f32 } else { -0.02f32 };

                if let Some(app) = GLOBAL_APP_HANDLE.lock().unwrap().as_ref() {
                    let _ = app.emit("action-scroll-volume-step", step);

                    trigger_hud_notification(
                        app,
                        HudNotification {
                            title: "Game / Active Focus".to_string(),
                            detail: format!("Volume {:+}%", (step * 100.0) as i32),
                            volume: 0.75 + step,
                            is_muted: false,
                            icon: "game".to_string(),
                        },
                    );
                }

                // Consume the mouse wheel message so games don't switch weapons/zoom
                return LRESULT(1);
            }
        } else if msg_id == WM_XBUTTONDOWN {
            let hook_struct = unsafe { *(l_param.0 as *const MSLLHOOKSTRUCT) };
            let button = (hook_struct.mouseData >> 16) as u16;

            if button == 1 || button == 2 {
                // Mouse 4 or Mouse 5 (Push-to-Mute / Push-to-Talk)
                if let Some(app) = GLOBAL_APP_HANDLE.lock().unwrap().as_ref() {
                    let _ = app.emit("action-push-to-mute", true);
                    trigger_hud_notification(
                        app,
                        HudNotification {
                            title: "Microphone".to_string(),
                            detail: "Push-To-Mute ACTIVE".to_string(),
                            volume: 0.0,
                            is_muted: true,
                            icon: "mic".to_string(),
                        },
                    );
                }
            }
        } else if msg_id == WM_XBUTTONUP {
            let hook_struct = unsafe { *(l_param.0 as *const MSLLHOOKSTRUCT) };
            let button = (hook_struct.mouseData >> 16) as u16;

            if button == 1 || button == 2 {
                if let Some(app) = GLOBAL_APP_HANDLE.lock().unwrap().as_ref() {
                    let _ = app.emit("action-push-to-mute", false);
                }
            }
        }
    }

    let hook_raw = MOUSE_HOOK_HANDLE.lock().unwrap().0;
    let hook = if hook_raw != 0 {
        HHOOK(hook_raw as *mut _)
    } else {
        HHOOK(std::ptr::null_mut())
    };
    unsafe { CallNextHookEx(hook, n_code, w_param, l_param) }
}
