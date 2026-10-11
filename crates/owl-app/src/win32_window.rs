//! Win32 Window Styles & DirectComposition Windows 11 Integration
//!
//! Handles Mica Alt / Acrylic backdrop, Immersive Dark Mode, Click-through
//! non-activating In-Game HUD overlay styles, and Taskbar Flyout positioning.

use std::ffi::c_void;
use log::{error, info};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

// DWM Backdrop constants for Windows 11
const DWMSBT_TABBEDWINDOW: u32 = 4; // Mica Alt (Tabbed window material)
const DWMSBT_TRANSIENTWINDOW: u32 = 3; // Acrylic
const DWMWCP_ROUND: u32 = 2; // Windows 11 Rounded corners

/// Apply Windows 11 Fluent Design styling: Mica Alt / Dark Mode / Rounded Corners
pub fn apply_fluent_backdrop(hwnd: HWND, mica_alt: bool) {
    unsafe {
        // 1. Force Immersive Dark Mode
        let dark_mode: u32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_mode as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );

        // 2. Rounded Window Corners (Windows 11 standard)
        let corner_pref: u32 = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner_pref as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );

        // 3. System Backdrop: Mica Alt (DWMSBT_TABBEDWINDOW = 4) or Acrylic (DWMSBT_TRANSIENTWINDOW = 3)
        let backdrop_type = if mica_alt {
            DWMSBT_TABBEDWINDOW
        } else {
            DWMSBT_TRANSIENTWINDOW
        };
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop_type as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );

        info!(
            "Applied Fluent Design attributes to HWND 0x{:X} (Mica Alt: {})",
            hwnd.0 as usize, mica_alt
        );
    }
}

/// Apply Click-Through, Non-Activating In-Game Overlay styles to window.
///
/// Ensures fullscreen DirectX 11/12 and Vulkan games never lose focus
/// and all mouse clicks pass straight through to the game engine.
#[allow(dead_code)]
pub fn apply_overlay_styles(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let overlay_style = ex_style
            | (WS_EX_TRANSPARENT.0 as i32)
            | (WS_EX_LAYERED.0 as i32)
            | (WS_EX_NOACTIVATE.0 as i32)
            | (WS_EX_TOPMOST.0 as i32)
            | (WS_EX_TOOLWINDOW.0 as i32); // Hide from Alt+Tab switcher

        SetWindowLongW(hwnd, GWL_EXSTYLE, overlay_style);

        // Apply dark mode & rounded corners for overlay window
        let dark_mode: u32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_mode as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );

        let corner_pref: u32 = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner_pref as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );

        info!(
            "Configured HWND 0x{:X} as click-through non-activating in-game HUD overlay",
            hwnd.0 as usize
        );
    }
}

/// Position Taskbar Flyout window neatly above the notification tray near the clock
pub fn position_flyout_near_tray(hwnd: HWND, width: i32, height: i32) {
    unsafe {
        let mut cursor_pt = POINT { x: 0, y: 0 };
        let _ = GetCursorPos(&mut cursor_pt);

        let monitor = MonitorFromPoint(cursor_pt, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };

        if GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            let work = monitor_info.rcWork;
            // Position above taskbar in bottom right corner with 12px margin
            let margin = 12;
            let x = (work.right - width - margin).max(work.left);
            let y = (work.bottom - height - margin).max(work.top);

            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_SHOWWINDOW,
            );
        } else {
            error!("Failed to retrieve monitor info for Taskbar Flyout positioning");
        }
    }
}

/// Position In-Game HUD overlay centered at the top of the active gaming screen
#[allow(dead_code)]
pub fn position_hud_overlay(hwnd: HWND, width: i32, height: i32) {
    unsafe {
        let mut cursor_pt = POINT { x: 0, y: 0 };
        let _ = GetCursorPos(&mut cursor_pt);

        let monitor = MonitorFromPoint(cursor_pt, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };

        if GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            let work = monitor_info.rcWork;
            // Center horizontally, 36px from top of screen
            let x = work.left + ((work.right - work.left) - width) / 2;
            let y = work.top + 36;

            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
        }
    }
}
