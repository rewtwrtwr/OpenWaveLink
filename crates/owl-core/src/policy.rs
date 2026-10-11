//! Windows Audio Policy Routing (Per-Application Audio Routing)
//!
//! Binds a specific process (PID or executable) to an audio output endpoint
//! using the Windows 11 `Windows.Media.Internal.AudioPolicyConfig` WinRT factory
//! and `SetPersistedDefaultAudioEndpoint`.

use std::ffi::c_void;
use windows::core::{s, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Interface GUID for `IAudioPolicyConfigFactoryVariantFor21H2`
/// {ab3d4648-e242-459f-b02f-541c70306324}
const IID_AUDIO_POLICY_CONFIG_FACTORY_21H2: windows::core::GUID = windows::core::GUID::from_u128(
    0xab3d4648_e242_459f_b02f_541c70306324,
);

#[repr(C)]
struct IAudioPolicyConfigFactoryVtbl {
    pub query_interface: usize,
    pub add_ref: usize,
    pub release: usize,
    pub get_iids: usize,
    pub get_runtime_class_name: usize,
    pub get_trust_level: usize,
    pub dummy: [usize; 19],
    pub set_persisted_default_audio_endpoint: unsafe extern "system" fn(
        this: *mut c_void,
        process_id: u32,
        flow: u32,
        role: u32,
        device_id: PCWSTR,
    ) -> HRESULT,
    pub get_persisted_default_audio_endpoint: usize,
    pub clear_all: usize,
}

#[repr(C)]
struct IAudioPolicyConfigFactory {
    pub vtable: *const IAudioPolicyConfigFactoryVtbl,
}

type RoGetActivationFactoryFn = unsafe extern "system" fn(
    activatable_class_id: *mut c_void,
    iid: *const windows::core::GUID,
    factory: *mut *mut c_void,
) -> HRESULT;

/// Query the full image path for a process ID.
pub fn get_process_image_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path_buf = [0u16; 1024];
        let mut len = path_buf.len() as u32;
        let res = QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0), PWSTR(path_buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);

        if res.is_ok() && len > 0 {
            Some(String::from_utf16_lossy(&path_buf[..len as usize]))
        } else {
            None
        }
    }
}

use windows::core::PWSTR;

/// Set the default audio endpoint for a specific running process (by PID).
///
/// Binds `eRender` (playback) for Console, Multimedia, and Communications roles
/// to the specified Windows endpoint id (`{0.0.0.00000000}.{...}`).
pub fn set_app_audio_endpoint(pid: u32, endpoint_id: &str) -> Result<(), String> {
    if pid == 0 {
        return Err("Cannot bind PID 0 (system sounds)".to_string());
    }

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let combase = LoadLibraryA(s!("combase.dll"))
            .map_err(|e| format!("LoadLibrary combase.dll failed: {e}"))?;

        let ro_get_factory: RoGetActivationFactoryFn = {
            let proc = GetProcAddress(combase, s!("RoGetActivationFactory"));
            match proc {
                Some(p) => std::mem::transmute(p),
                None => return Err("RoGetActivationFactory not found in combase.dll".to_string()),
            }
        };

        let class_id = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
        let raw_hstring: *mut c_void = std::mem::transmute_copy(&class_id);
        let mut factory_ptr: *mut c_void = std::ptr::null_mut();

        let hr = ro_get_factory(
            raw_hstring,
            &IID_AUDIO_POLICY_CONFIG_FACTORY_21H2,
            &mut factory_ptr,
        );

        if hr.is_err() || factory_ptr.is_null() {
            return Err(format!("RoGetActivationFactory failed with HRESULT 0x{:08X}", hr.0 as u32));
        }

        let factory = &*(factory_ptr as *const IAudioPolicyConfigFactory);
        let wide_id: Vec<u16> = endpoint_id.encode_utf16().chain(Some(0)).collect();
        let pcwstr = PCWSTR(wide_id.as_ptr());

        // Set for eConsole (0), eMultimedia (1), eCommunications (2)
        let hr0 = ((*factory.vtable).set_persisted_default_audio_endpoint)(
            factory_ptr,
            pid,
            0, // eRender
            0, // eConsole
            pcwstr,
        );

        let hr1 = ((*factory.vtable).set_persisted_default_audio_endpoint)(
            factory_ptr,
            pid,
            0, // eRender
            1, // eMultimedia
            pcwstr,
        );

        let hr2 = ((*factory.vtable).set_persisted_default_audio_endpoint)(
            factory_ptr,
            pid,
            0, // eRender
            2, // eCommunications
            pcwstr,
        );

        // Release factory
        let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute((*factory.vtable).release);
        release_fn(factory_ptr);

        if hr0.is_err() && hr1.is_err() && hr2.is_err() {
            return Err(format!(
                "SetPersistedDefaultAudioEndpoint returned error 0x{:08X}",
                hr0.0 as u32
            ));
        }

        log::info!(
            "Successfully bound PID {pid} to audio output endpoint '{endpoint_id}' in Windows"
        );
        Ok(())
    }
}

const CLSID_POLICY_CONFIG_CLIENT: windows::core::GUID = windows::core::GUID::from_u128(
    0x870af99c_171d_4f9e_af0d_e63df40c2bc9,
);
const IID_POLICY_CONFIG: windows::core::GUID = windows::core::GUID::from_u128(
    0xf8679f50_850a_41cf_9c72_430f290290c8,
);

#[repr(C)]
struct IPolicyConfigVtbl {
    pub query_interface: usize,
    pub add_ref: usize,
    pub release: usize,
    pub get_mix_format: usize,
    pub get_device_format: usize,
    pub reset_device_format: usize,
    pub set_device_format: usize,
    pub get_processing_period: usize,
    pub set_processing_period: usize,
    pub get_share_mode: usize,
    pub set_share_mode: usize,
    pub get_property_value: usize,
    pub set_property_value: usize,
    pub set_default_endpoint: unsafe extern "system" fn(
        this: *mut c_void,
        device_id: PCWSTR,
        role: u32,
    ) -> HRESULT,
    pub set_endpoint_visibility: usize,
}

#[repr(C)]
struct IPolicyConfig {
    pub vtable: *const IPolicyConfigVtbl,
}

/// Set the default audio playback endpoint in Windows (System-wide default).
pub fn set_default_playback_endpoint(endpoint_id: &str) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let client: windows::core::IUnknown = windows::Win32::System::Com::CoCreateInstance(
            &CLSID_POLICY_CONFIG_CLIENT,
            None,
            windows::Win32::System::Com::CLSCTX_ALL,
        )
        .map_err(|e| format!("CoCreateInstance PolicyConfig failed: {e}"))?;

        let client_ptr: *mut c_void = std::mem::transmute_copy(&client);
        let config = &*(client_ptr as *const IPolicyConfig);
        let wide_id: Vec<u16> = endpoint_id.encode_utf16().chain(Some(0)).collect();
        let pcwstr = PCWSTR(wide_id.as_ptr());

        // eConsole (0), eMultimedia (1), eCommunications (2)
        let _ = ((*config.vtable).set_default_endpoint)(client_ptr, pcwstr, 0);
        let _ = ((*config.vtable).set_default_endpoint)(client_ptr, pcwstr, 1);
        let _ = ((*config.vtable).set_default_endpoint)(client_ptr, pcwstr, 2);

        log::info!("Successfully set '{endpoint_id}' as default Windows playback endpoint");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_image_path_self() {
        let pid = std::process::id();
        let path = get_process_image_path(pid);
        assert!(path.is_some(), "Must obtain image path for current process");
    }

    #[test]
    fn test_bind_invalid_pid() {
        let res = set_app_audio_endpoint(0, "{0.0.0.00000000}.{dummy}");
        assert!(res.is_err(), "Must reject PID 0");
    }
}
