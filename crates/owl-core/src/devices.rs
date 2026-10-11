//! Windows 11 CoreAudio Device Enumerator (IMMDeviceEnumerator)
//!
//! Enumerates real system audio hardware devices for eCapture (inputs) and eRender (outputs).

use serde::{Deserialize, Serialize};
use windows::Win32::Media::Audio::{
    eCapture, eMultimedia, eRender, EDataFlow, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

/// A physical or virtual system audio device endpoint in Windows 11
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDeviceDto {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub flow: String, // "input" | "output"
}

/// Substrings that identify a *virtual* (loopback / cable) endpoint by name.
///
/// These are matched case-insensitively against the endpoint's friendly name.
/// Deliberately name-based rather than GUID-based: endpoint GUIDs are per-machine
/// and change when a device is reinstalled, so hardcoding them would rot.
const VIRTUAL_ENDPOINT_MARKERS: [&str; 5] = [
    "elgato virtual audio",
    "vb-audio",
    "cable",
    "virtual",
    "fifine virtual",
];

/// Heuristic: is this endpoint name a virtual cable / loopback device?
///
/// Used to warn when the monitor mix would be rendered into a cable that the
/// user cannot hear. This is a *heuristic*, not a guarantee — Windows exposes no
/// authoritative "is virtual" flag through `IMMDeviceEnumerator`.
pub fn is_virtual_endpoint_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIRTUAL_ENDPOINT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

// PKEY_Device_FriendlyName: {a45c254e-df1c-4efd-8020-67d146a850e0}, 14
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

// PKEY_Device_DeviceDesc: {a45c254e-df1c-4efd-8020-67d146a850e0}, 2
const PKEY_DEVICE_DEVICE_DESC: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 2,
};

// PKEY_DeviceInterface_FriendlyName: {b3f8fa53-0004-438e-9003-51a46e139bfc}, 6
const PKEY_DEVICE_INTERFACE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xb3f8fa53_0004_438e_9003_51a46e139bfc),
    pid: 6,
};

/// Format full device friendly name combining container/interface labels if needed
fn format_device_full_name(store: &windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore, fallback_id: &str) -> String {
    let friendly = unsafe {
        store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME)
            .ok()
            .map(|p| format!("{p}").trim().to_string())
            .unwrap_or_default()
    };

    let iface_desc = unsafe {
        store.GetValue(&PKEY_DEVICE_INTERFACE_FRIENDLY_NAME)
            .ok()
            .map(|p| format!("{p}").trim().to_string())
            .unwrap_or_default()
    };

    let dev_desc = unsafe {
        store.GetValue(&PKEY_DEVICE_DEVICE_DESC)
            .ok()
            .map(|p| format!("{p}").trim().to_string())
            .unwrap_or_default()
    };

    if !iface_desc.is_empty() {
        let base = if !friendly.is_empty() && friendly != iface_desc {
            friendly
        } else if !dev_desc.is_empty() {
            dev_desc
        } else {
            "Audio Device".to_string()
        };

        if base.contains(&iface_desc) {
            base
        } else {
            format!("{base} ({iface_desc})")
        }
    } else if !friendly.is_empty() {
        friendly
    } else if !dev_desc.is_empty() {
        dev_desc
    } else {
        fallback_id.to_string()
    }
}

/// Resolve the friendly name of a render endpoint by its `IMMDeviceEnumerator` id.
///
/// Returns `None` when the id does not resolve to an active endpoint.
pub fn resolve_endpoint_name(endpoint_id: &str) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()? };

    let device = unsafe {
        enumerator
            .GetDevice(&windows::core::HSTRING::from(endpoint_id))
            .ok()?
    };

    unsafe {
        match device.OpenPropertyStore(STGM_READ) {
            Ok(store) => {
                let name = format_device_full_name(&store, endpoint_id);
                if name.is_empty() || name == endpoint_id {
                    None
                } else {
                    Some(name)
                }
            }
            Err(_) => None,
        }
    }
}

/// Collection of available system audio inputs and outputs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemAudioDevicesDto {
    pub inputs: Vec<AudioDeviceDto>,
    pub outputs: Vec<AudioDeviceDto>,
}

/// Enumerate all active audio devices (inputs and outputs) in Windows 11
pub fn get_system_audio_devices() -> Result<SystemAudioDevicesDto, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let enumerator: IMMDeviceEnumerator = unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| format!("Failed to create MMDeviceEnumerator: {e}"))?
    };

    let default_capture_id = unsafe {
        enumerator
            .GetDefaultAudioEndpoint(eCapture, eMultimedia)
            .and_then(|dev| dev.GetId())
            .map(|pwstr| {
                let s = pwstr.to_string().unwrap_or_default();
                CoTaskMemFree(Some(pwstr.as_ptr() as *const _));
                s
            })
            .unwrap_or_default()
    };

    let default_render_id = unsafe {
        enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .and_then(|dev| dev.GetId())
            .map(|pwstr| {
                let s = pwstr.to_string().unwrap_or_default();
                CoTaskMemFree(Some(pwstr.as_ptr() as *const _));
                s
            })
            .unwrap_or_default()
    };

    let inputs = enumerate_flow_devices(&enumerator, eCapture, &default_capture_id, "input")?;
    let outputs = enumerate_flow_devices(&enumerator, eRender, &default_render_id, "output")?;

    Ok(SystemAudioDevicesDto { inputs, outputs })
}

fn enumerate_flow_devices(
    enumerator: &IMMDeviceEnumerator,
    flow: EDataFlow,
    default_id: &str,
    flow_str: &str,
) -> Result<Vec<AudioDeviceDto>, String> {
    let collection = unsafe {
        enumerator
            .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
            .map_err(|e| format!("Failed to enum audio endpoints: {e}"))?
    };

    let count = unsafe {
        collection
            .GetCount()
            .map_err(|e| format!("Failed to get device count: {e}"))?
    };

    let mut devices = Vec::with_capacity(count as usize);

    for i in 0..count {
        let device = match unsafe { collection.Item(i) } {
            Ok(d) => d,
            Err(_) => continue,
        };

        let id = unsafe {
            match device.GetId() {
                Ok(pwstr) => {
                    let s = pwstr.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(pwstr.as_ptr() as *const _));
                    s
                }
                Err(_) => continue,
            }
        };

        let is_default = !default_id.is_empty() && id == default_id;

        // Retrieve friendly name from property store
        let name = unsafe {
            match device.OpenPropertyStore(STGM_READ) {
                Ok(store) => format_device_full_name(&store, &id),
                Err(_) => id.clone(),
            }
        };

        devices.push(AudioDeviceDto {
            id,
            name,
            is_default,
            flow: flow_str.to_string(),
        });
    }

    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enumerate_devices() {
        let devs = get_system_audio_devices();
        assert!(devs.is_ok(), "Device enumeration should succeed: {:?}", devs.err());
        let list = devs.unwrap();
        println!("Found {} input audio devices:", list.inputs.len());
        for input in &list.inputs {
            println!("  [IN]  {} (ID: {}, default: {})", input.name, input.id, input.is_default);
        }
        println!("Found {} output audio devices:", list.outputs.len());
        for output in &list.outputs {
            println!("  [OUT] {} (ID: {}, default: {})", output.name, output.id, output.is_default);
        }
        assert!(!list.inputs.is_empty() || !list.outputs.is_empty(), "Expected at least one audio endpoint");
    }
}
