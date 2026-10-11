//! Windows Audio Session Enumerator (IAudioSessionEnumerator)

use std::path::Path;
use windows::core::{Interface, PWSTR, HRESULT};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionStateActive, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};

use serde::{Deserialize, Serialize};

use crate::error::SessionError;

/// Information about a process with an active or inactive Windows audio session
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioProcessInfo {
    pub pid: u32,
    pub name: String,
    pub is_active: bool,
    pub is_system: bool,
}

/// DTO for active audio processes currently playing sound in Windows 11
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioProcessDto {
    pub pid: u32,
    pub name: String,
    pub process_name: String,
    pub is_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Return ONLY real processes that are actively rendering/playing sound right now
pub fn get_active_audio_sessions() -> Result<Vec<AudioProcessDto>, SessionError> {
    let sessions = enumerate_audio_sessions()?;
    let active_only: Vec<AudioProcessDto> = sessions
        .into_iter()
        .filter(|s| s.is_active && s.pid > 0)
        .map(|s| {
            let icon = crate::apps::get_app_icon_data_url(Some(s.pid), Some(&s.name));
            AudioProcessDto {
                pid: s.pid,
                process_name: s.name.clone(),
                name: s.name,
                is_active: s.is_active,
                icon,
            }
        })
        .collect();

    Ok(active_only)
}

/// Enumerate all audio sessions registered across active playback audio endpoints
pub fn enumerate_audio_sessions() -> Result<Vec<AudioProcessInfo>, SessionError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let enumerator: IMMDeviceEnumerator = unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(SessionError::WindowsCom)?
    };

    let mut endpoints = Vec::new();
    if let Ok(dev_collection) = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) } {
        if let Ok(count) = unsafe { dev_collection.GetCount() } {
            for idx in 0..count {
                if let Ok(dev) = unsafe { dev_collection.Item(idx) } {
                    endpoints.push(dev);
                }
            }
        }
    }

    if endpoints.is_empty() {
        if let Ok(def) = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) } {
            endpoints.push(def);
        }
    }

    let mut sessions: Vec<AudioProcessInfo> = Vec::new();

    for endpoint in endpoints {
        let session_manager: IAudioSessionManager2 = match unsafe { endpoint.Activate(CLSCTX_ALL, None) } {
            Ok(mgr) => mgr,
            Err(_) => continue,
        };

        let session_enum = match unsafe { session_manager.GetSessionEnumerator() } {
            Ok(e) => e,
            Err(_) => continue,
        };

        let count = match unsafe { session_enum.GetCount() } {
            Ok(c) => c,
            Err(_) => continue,
        };

        for i in 0..count {
            let session_ctrl: IAudioSessionControl = match unsafe { session_enum.GetSession(i) } {
                Ok(s) => s,
                Err(_) => continue,
            };

            let session_ctrl2: IAudioSessionControl2 = match session_ctrl.cast() {
                Ok(s2) => s2,
                Err(_) => continue,
            };

            let pid = match unsafe { session_ctrl2.GetProcessId() } {
                Ok(p) => p,
                Err(_) => continue,
            };

            let is_system = unsafe { session_ctrl2.IsSystemSoundsSession() } == HRESULT(0); // S_OK

            let state_active = match unsafe { session_ctrl.GetState() } {
                Ok(state) => state == AudioSessionStateActive,
                Err(_) => false,
            };

            let meter: Option<IAudioMeterInformation> = session_ctrl.cast().ok();
            let peak = meter.and_then(|m| unsafe { m.GetPeakValue().ok() }).unwrap_or(0.0);

            let is_active = state_active || peak > 0.0001;

            let name = if is_system || pid == 0 {
                "System Sounds".to_string()
            } else {
                get_process_name_by_pid(pid).unwrap_or_else(|| format!("PID {pid}"))
            };

            // If PID already in list, update active state if this session is active
            if let Some(existing) = sessions.iter_mut().find(|s: &&mut AudioProcessInfo| s.pid == pid) {
                if is_active {
                    existing.is_active = true;
                }
            } else {
                sessions.push(AudioProcessInfo {
                    pid,
                    name,
                    is_active,
                    is_system,
                });
            }
        }
    }

    // Sort active sessions first, then alphabetically by name
    sessions.sort_by(|a, b| {
        b.is_active
            .cmp(&a.is_active)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(sessions)
}

/// Retrieve executable name from Windows process ID
fn get_process_name_by_pid(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;

        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );

        let _ = CloseHandle(handle);

        if res.is_ok() && len > 0 {
            let full_path = String::from_utf16_lossy(&buf[..len as usize]);
            let file_name = Path::new(&full_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&full_path);
            Some(file_name.to_string())
        } else {
            None
        }
    }
}
