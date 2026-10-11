//! Windows 11 Process-Loopback COM Activation

use std::sync::{Arc, Mutex};
use windows::core::{implement, Interface, HRESULT};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioClient, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
};
use windows::Win32::System::Com::{IAgileObject, IAgileObject_Impl};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

use crate::error::CaptureError;

/// Binary representation of PROPVARIANT with VT_BLOB layout
#[repr(C)]
struct PropVariantBlob {
    vt: u16,        // VT_BLOB = 65
    w_reserved1: u16,
    w_reserved2: u16,
    w_reserved3: u16,
    cb_size: u32,
    p_blob_data: *mut u8,
}

struct ActivationResult(Option<Result<IAudioClient, CaptureError>>);
unsafe impl Send for ActivationResult {}
unsafe impl Sync for ActivationResult {}

/// COM completion handler implementing IActivateAudioInterfaceCompletionHandler & IAgileObject.
/// IAgileObject is essential to prevent deadlocks across MTA/STA Windows Audio threads.
#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
pub struct ProcessActivationHandler {
    event: HANDLE,
    result: Arc<Mutex<ActivationResult>>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ProcessActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        operation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let op = match operation {
            Some(op) => op,
            None => {
                let mut guard = self.result.lock().unwrap();
                guard.0 = Some(Err(CaptureError::ActivationFailed(
                    "Async operation was null in completion callback".to_string(),
                )));
                unsafe { let _ = SetEvent(self.event); };
                return Ok(());
            }
        };

        let mut hr = HRESULT(0);
        let mut unk = None;

        let res = unsafe {
            match op.GetActivateResult(&mut hr, &mut unk) {
                Ok(()) => match hr.ok() {
                    Ok(()) => match unk {
                        Some(u) => match u.cast::<IAudioClient>() {
                            Ok(client) => Ok(client),
                            Err(e) => Err(CaptureError::WindowsCom(e)),
                        },
                        None => Err(CaptureError::ActivationFailed(
                            "GetActivateResult returned null IUnknown pointer".to_string(),
                        )),
                    },
                    Err(e) => Err(CaptureError::WindowsCom(e)),
                },
                Err(e) => Err(CaptureError::WindowsCom(e)),
            }
        };

        let mut guard = self.result.lock().unwrap();
        guard.0 = Some(res);
        unsafe { let _ = SetEvent(self.event); };
        Ok(())
    }
}

impl IAgileObject_Impl for ProcessActivationHandler_Impl {}

/// Activate IAudioClient in Process-Loopback mode for the specified Target PID
pub fn activate_process_loopback(
    target_pid: u32,
    timeout_ms: u32,
) -> Result<IAudioClient, CaptureError> {
    let completion_event = unsafe {
        CreateEventW(None, false, false, None)
            .map_err(CaptureError::WindowsCom)?
    };

    let result_holder = Arc::new(Mutex::new(ActivationResult(None)));

    let handler: IActivateAudioInterfaceCompletionHandler = ProcessActivationHandler {
        event: completion_event,
        result: Arc::clone(&result_holder),
    }
    .into();

    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: target_pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };

    let raw_pv = PropVariantBlob {
        vt: 65, // VT_BLOB
        w_reserved1: 0,
        w_reserved2: 0,
        w_reserved3: 0,
        cb_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
        p_blob_data: &mut params as *mut _ as *mut u8,
    };

    let pv_ptr = &raw_pv as *const _ as *const windows::core::PROPVARIANT;

    let _async_op = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(pv_ptr),
            &handler,
        )
        .map_err(|e| {
            let _ = CloseHandle(completion_event);
            CaptureError::WindowsCom(e)
        })?
    };

    let wait_res = unsafe { WaitForSingleObject(completion_event, timeout_ms) };
    unsafe { let _ = CloseHandle(completion_event); };

    if wait_res == WAIT_TIMEOUT {
        return Err(CaptureError::ActivationTimeout);
    } else if wait_res != WAIT_OBJECT_0 {
        return Err(CaptureError::ActivationFailed(format!(
            "WaitForSingleObject failed with code {wait_res:?}"
        )));
    }

    let mut guard = result_holder.lock().unwrap();
    guard.0.take().unwrap_or_else(|| {
        Err(CaptureError::ActivationFailed(
            "Activation completed but no result was stored".to_string(),
        ))
    })
}
