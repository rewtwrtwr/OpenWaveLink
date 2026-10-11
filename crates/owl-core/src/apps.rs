//! Windows 11 Installed Applications Scanner
//!
//! Scans Windows Registry (HKLM & HKCU App Paths / Uninstall) and Start Menu shortcuts
//! to enumerate real applications and games installed on the user's PC.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ,
};

/// Information about an installed application or game on Windows 11
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledAppDto {
    pub name: String,
    pub exe_name: String,
    pub path: String,
    pub icon_path: Option<String>,
}

/// Scan Windows 11 registry App Paths and return deduplicated installed apps
pub fn get_installed_applications() -> Vec<InstalledAppDto> {
    let mut apps_map: BTreeMap<String, InstalledAppDto> = BTreeMap::new();

    // 1. Scan HKEY_LOCAL_MACHINE App Paths
    scan_app_paths(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths"), &mut apps_map);
    // 2. Scan HKEY_LOCAL_MACHINE 32-bit (WOW6432Node) App Paths
    scan_app_paths(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths"), &mut apps_map);
    // 3. Scan HKEY_CURRENT_USER App Paths
    scan_app_paths(HKEY_CURRENT_USER, w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths"), &mut apps_map);

    // 4. Scan Start Menu Programs folders for .lnk/.exe files
    scan_start_menu_directories(&mut apps_map);

    // Convert map to sorted Vec
    let mut list: Vec<InstalledAppDto> = apps_map.into_values().collect();
    list.sort_by_key(|a| a.name.to_lowercase());
    list
}

fn scan_app_paths(root: HKEY, subkey: PCWSTR, map: &mut BTreeMap<String, InstalledAppDto>) {
    unsafe {
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(root, subkey, 0, KEY_READ, &mut hkey).is_err() {
            return;
        }

        let mut index = 0u32;
        let mut key_name_buf = [0u16; 260];

        loop {
            let mut key_name_len = key_name_buf.len() as u32;
            let res = RegEnumKeyExW(
                hkey,
                index,
                PWSTR(key_name_buf.as_mut_ptr()),
                &mut key_name_len,
                None,
                PWSTR::null(),
                None,
                None,
            );

            if res != WIN32_ERROR(0) {
                break;
            }

            let app_key_name = String::from_utf16_lossy(&key_name_buf[..key_name_len as usize]);
            index += 1;

            if !app_key_name.to_lowercase().ends_with(".exe") {
                continue;
            }

            // Open this subkey and read default value (full path to exe)
            let mut child_hkey = HKEY::default();
            let mut wide_child_name: Vec<u16> = app_key_name.encode_utf16().chain(std::iter::once(0)).collect();
            if RegOpenKeyExW(hkey, PCWSTR(wide_child_name.as_mut_ptr()), 0, KEY_READ, &mut child_hkey).is_ok() {
                let mut path_buf = [0u16; 512];
                let mut path_bytes_len = (path_buf.len() * 2) as u32;

                let val_res = RegQueryValueExW(
                    child_hkey,
                    PCWSTR::null(), // default value
                    None,
                    None,
                    Some(path_buf.as_mut_ptr() as *mut u8),
                    Some(&mut path_bytes_len),
                );

                let _ = RegCloseKey(child_hkey);

                if val_res == WIN32_ERROR(0) {
                    let chars_len = (path_bytes_len / 2) as usize;
                    let raw_path = String::from_utf16_lossy(&path_buf[..chars_len])
                        .trim_matches('\0')
                        .trim_matches('"')
                        .to_string();

                    if !raw_path.is_empty() {
                        let exe_lower = app_key_name.to_lowercase();
                        let clean_name = format_display_name(&app_key_name);

                        map.entry(exe_lower.clone()).or_insert_with(|| InstalledAppDto {
                            name: clean_name,
                            exe_name: app_key_name.clone(),
                            path: raw_path.clone(),
                            icon_path: Some(raw_path),
                        });
                    }
                }
            }
        }

        let _ = RegCloseKey(hkey);
    }
}

fn scan_start_menu_directories(map: &mut BTreeMap<String, InstalledAppDto>) {
    let mut dirs_to_check = Vec::new();

    if let Ok(appdata) = std::env::var("APPDATA") {
        dirs_to_check.push(PathBuf::from(appdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Ok(programdata) = std::env::var("PROGRAMDATA") {
        dirs_to_check.push(PathBuf::from(programdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }

    for dir in dirs_to_check {
        if dir.exists() {
            walk_dir_for_apps(&dir, map, 0);
        }
    }
}

fn walk_dir_for_apps(dir: &Path, map: &mut BTreeMap<String, InstalledAppDto>, depth: usize) {
    if depth > 4 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir_for_apps(&path, map, depth + 1);
        } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            let ext_lower = ext.to_lowercase();
            if ext_lower == "lnk" {
                // Use file stem as app name
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let stem_clean = stem.trim();
                    let exe_name = format!("{}.exe", stem_clean.replace(' ', ""));
                    let exe_lower = exe_name.to_lowercase();

                    if !map.contains_key(&exe_lower) && !stem_clean.to_lowercase().contains("uninstall") {
                        map.insert(
                            exe_lower,
                            InstalledAppDto {
                                name: stem_clean.to_string(),
                                exe_name,
                                path: path.to_string_lossy().to_string(),
                                icon_path: Some(path.to_string_lossy().to_string()),
                            },
                        );
                    }
                }
            }
        }
    }
}

/// Convert an executable name like "Spotify.exe" or "discord.exe" to human-friendly "Spotify"
fn format_display_name(exe_filename: &str) -> String {
    let stem = exe_filename.strip_suffix(".exe").unwrap_or(exe_filename);
    let mut chars: Vec<char> = stem.chars().collect();
    if let Some(first) = chars.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    chars.into_iter().collect()
}

/// Locate the executable path for a process name (e.g. "chrome.exe" or "Spotify.exe")
pub fn find_app_exe_path(process_name: &str) -> Option<String> {
    let lower = process_name.to_lowercase();
    let installed = get_installed_applications();
    for app in installed {
        if app.exe_name.to_lowercase() == lower || app.name.to_lowercase() == lower {
            if Path::new(&app.path).exists() {
                return Some(app.path);
            }
        }
    }

    // Common system fallback locations
    for env_var in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
        if let Ok(base) = std::env::var(env_var) {
            let candidate = PathBuf::from(base).join(&process_name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }

    None
}

fn base64_encode(data: &[u8]) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(CHARSET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARSET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARSET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARSET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Extract an application's Windows icon and return it as a `data:image/bmp;base64,...` URL.
pub fn extract_icon_as_base64(exe_path: &str) -> Option<String> {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

    unsafe {
        let wide_path: Vec<u16> = exe_path.encode_utf16().chain(Some(0)).collect();
        let mut shfi = SHFILEINFOW::default();
        let res = SHGetFileInfoW(
            PCWSTR(wide_path.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut shfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );

        if res == 0 || shfi.hIcon.is_invalid() {
            return None;
        }

        let mut icon_info = ICONINFO::default();
        if GetIconInfo(shfi.hIcon, &mut icon_info).is_err() {
            let _ = DestroyIcon(shfi.hIcon);
            return None;
        }

        let mut bmp = BITMAP::default();
        let hbm = if !icon_info.hbmColor.is_invalid() {
            icon_info.hbmColor
        } else {
            icon_info.hbmMask
        };

        if GetObjectW(
            hbm,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut _ as *mut c_void),
        ) == 0
        {
            if !icon_info.hbmColor.is_invalid() {
                let _ = DeleteObject(icon_info.hbmColor);
            }
            if !icon_info.hbmMask.is_invalid() {
                let _ = DeleteObject(icon_info.hbmMask);
            }
            let _ = DestroyIcon(shfi.hIcon);
            return None;
        }

        let width = bmp.bmWidth;
        let height = bmp.bmHeight.abs();
        if width <= 0 || height <= 0 || width > 256 || height > 256 {
            if !icon_info.hbmColor.is_invalid() {
                let _ = DeleteObject(icon_info.hbmColor);
            }
            if !icon_info.hbmMask.is_invalid() {
                let _ = DeleteObject(icon_info.hbmMask);
            }
            let _ = DestroyIcon(shfi.hIcon);
            return None;
        }

        let hdc = CreateCompatibleDC(None);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down bitmap
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (width * height * 4) as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [windows::Win32::Graphics::Gdi::RGBQUAD::default()],
        };

        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            hdc,
            hbm,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        let _ = DeleteDC(hdc);
        if !icon_info.hbmColor.is_invalid() {
            let _ = DeleteObject(icon_info.hbmColor);
        }
        if !icon_info.hbmMask.is_invalid() {
            let _ = DeleteObject(icon_info.hbmMask);
        }
        let _ = DestroyIcon(shfi.hIcon);

        if lines == 0 {
            return None;
        }

        // Set alpha channel to 255 if color icon has 0 alpha
        let mut has_non_zero_alpha = false;
        for chunk in pixels.chunks_exact(4) {
            if chunk[3] != 0 {
                has_non_zero_alpha = true;
                break;
            }
        }
        if !has_non_zero_alpha {
            for chunk in pixels.chunks_exact_mut(4) {
                if chunk[0] != 0 || chunk[1] != 0 || chunk[2] != 0 {
                    chunk[3] = 255;
                }
            }
        }

        // Build BMP byte array (14-byte file header + 40-byte info header + pixel data)
        let file_size = 14 + 40 + pixels.len() as u32;
        let mut bmp_data = Vec::with_capacity(file_size as usize);
        bmp_data.extend_from_slice(b"BM");
        bmp_data.extend_from_slice(&file_size.to_le_bytes());
        bmp_data.extend_from_slice(&[0u8; 4]);
        bmp_data.extend_from_slice(&54u32.to_le_bytes());
        bmp_data.extend_from_slice(&40u32.to_le_bytes());
        bmp_data.extend_from_slice(&width.to_le_bytes());
        bmp_data.extend_from_slice(&(-height).to_le_bytes());
        bmp_data.extend_from_slice(&1u16.to_le_bytes());
        bmp_data.extend_from_slice(&32u16.to_le_bytes());
        bmp_data.extend_from_slice(&0u32.to_le_bytes());
        bmp_data.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bmp_data.extend_from_slice(&[0u8; 16]);
        bmp_data.extend_from_slice(&pixels);

        Some(format!("data:image/bmp;base64,{}", base64_encode(&bmp_data)))
    }
}

/// Resolve icon data URL for an app given either its PID or its process name
pub fn get_app_icon_data_url(pid: Option<u32>, process_name: Option<&str>) -> Option<String> {
    if let Some(p) = pid.filter(|&id| id > 0) {
        if let Some(path) = crate::policy::get_process_image_path(p) {
            if let Some(icon) = extract_icon_as_base64(&path) {
                return Some(icon);
            }
        }
    }

    if let Some(name) = process_name {
        if let Some(path) = find_app_exe_path(name) {
            if let Some(icon) = extract_icon_as_base64(&path) {
                return Some(icon);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_installed_apps() {
        let apps = get_installed_applications();
        println!("Scanned {} installed applications from Windows 11:", apps.len());
        for app in apps.iter().take(15) {
            println!("  - {} ({}) -> {}", app.name, app.exe_name, app.path);
        }
        assert!(!apps.is_empty(), "Should find installed apps on Windows 11 system");
    }
}
