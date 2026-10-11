//! Process Loopback Audio Capture implementation for Windows 11

pub mod activation;
pub mod mic;
pub mod worker;

pub use mic::MicrophoneCapture;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

use crate::error::CaptureError;
use crate::format::AudioFormatInfo;
use crate::meter::AudioMeterState;
use self::activation::activate_process_loopback;
use self::worker::{run_capture_worker, AudioWorkerParams};

pub struct ProcessAudioCapture {
    pub target_pid: u32,
    pub format: Option<AudioFormatInfo>,
    meter: Arc<AudioMeterState>,
    is_running: Arc<AtomicBool>,
    consumer: Option<rtrb::Consumer<f32>>,
    stop_event: Option<HANDLE>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
}

// Safety: ProcessAudioCapture manages its own thread-safe handles and atomic state
unsafe impl Send for ProcessAudioCapture {}
unsafe impl Sync for ProcessAudioCapture {}

impl ProcessAudioCapture {
    pub fn new(target_pid: u32) -> Self {
        Self {
            target_pid,
            format: None,
            meter: Arc::new(AudioMeterState::new()),
            is_running: Arc::new(AtomicBool::new(false)),
            consumer: None,
            stop_event: None,
            worker_handle: None,
        }
    }

    /// Access meter state for real-time VU monitoring
    pub fn meter(&self) -> Arc<AudioMeterState> {
        Arc::clone(&self.meter)
    }

    /// Take the lock-free consumer to read captured 32-bit float stereo samples
    pub fn take_consumer(&mut self) -> Option<rtrb::Consumer<f32>> {
        self.consumer.take()
    }

    /// Check if audio capture thread is currently running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Acquire)
    }

    /// Start process loopback capture
    pub fn start(&mut self) -> Result<(), CaptureError> {
        if self.is_running() {
            return Ok(());
        }

        // Fail honestly for a PID that does not exist.
        //
        // `ActivateAudioInterfaceAsync` in process-loopback mode succeeds for a dead PID and
        // hands back a valid IAudioClient that simply never delivers a sample, so without this
        // check the daemon would report `capture_started: true` for a channel that can never
        // produce audio — exactly the silent-failure class this engine is meant to eliminate.
        if self.target_pid != 0 && !process_is_alive(self.target_pid) {
            return Err(CaptureError::ProcessNotFound(self.target_pid));
        }

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        // 1. Fetch system default render mix format
        let device_enum: IMMDeviceEnumerator = unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(CaptureError::WindowsCom)?
        };

        // 1. Resolve endpoint for capture
        // When PID is 0 (System channel), we MUST capture the dedicated OWL System virtual cable,
        // and NEVER the user's physical playback headphones/speakers (which causes an infinite acoustic feedback squeal).
        let capture_endpoint = if self.target_pid == 0 {
            let mut resolved_sys_endpoint = None;
            if let Ok(devices) = crate::devices::get_system_audio_devices() {
                for out in &devices.outputs {
                    let name_lower = out.name.to_lowercase();
                    // Match virtual cable named "OWL System" or "System"
                    if (name_lower.contains("owl system") || name_lower.contains("system"))
                        && !name_lower.contains("sound blaster")
                        && !name_lower.contains("realtek")
                        && !name_lower.contains("headphones")
                    {
                        if let Ok(dev) = unsafe {
                            device_enum.GetDevice(&windows::core::HSTRING::from(&out.id))
                        } {
                            log::info!("System capture bound to virtual cable endpoint: {}", out.name);
                            resolved_sys_endpoint = Some(dev);
                            break;
                        }
                    }
                }
            }

            match resolved_sys_endpoint {
                Some(dev) => dev,
                None => {
                    let def_endpoint = unsafe {
                        device_enum
                            .GetDefaultAudioEndpoint(eRender, eMultimedia)
                            .map_err(CaptureError::WindowsCom)?
                    };
                    let def_id = unsafe {
                        def_endpoint.GetId().ok().map(|p| p.to_string().unwrap_or_default())
                    }
                    .unwrap_or_default();
                    let def_name = crate::devices::resolve_endpoint_name(&def_id)
                        .unwrap_or_default()
                        .to_lowercase();

                    // Block feedback loop on physical monitoring hardware
                    if def_name.contains("sound blaster")
                        || def_name.contains("realtek")
                        || def_name.contains("headphones")
                    {
                        log::warn!(
                            "Default audio endpoint is a physical monitor device ('{def_name}'). \
                             Capture blocked to prevent feedback loop! Please ensure OWL System cable is created."
                        );
                        return Err(CaptureError::ActivationFailed(
                            "Feedback loop prevention: physical monitor device cannot be looped back".to_string(),
                        ));
                    }
                    def_endpoint
                }
            }
        } else {
            unsafe {
                device_enum
                    .GetDefaultAudioEndpoint(eRender, eMultimedia)
                    .map_err(CaptureError::WindowsCom)?
            }
        };

        let default_client: IAudioClient = unsafe {
            capture_endpoint
                .Activate(CLSCTX_ALL, None)
                .map_err(CaptureError::WindowsCom)?
        };

        let pwfx = unsafe {
            default_client
                .GetMixFormat()
                .map_err(CaptureError::WindowsCom)?
        };

        let format_info = unsafe {
            AudioFormatInfo::from_raw_wave_format(pwfx)
                .map_err(CaptureError::UnsupportedFormat)?
        };
        self.format = Some(format_info);

        log::info!(
            "Default Render Format: {} channels, {} Hz, {} bits, type {:?}",
            format_info.channels,
            format_info.sample_rate,
            format_info.bits_per_sample,
            format_info.sample_type
        );

        // 2. Activate IAudioClient via Process Loopback API or standard Render Loopback (for System PID 0)
        let audio_client = if self.target_pid == 0 {
            default_client
        } else {
            activate_process_loopback(self.target_pid, 5000)?
        };

        // 3. Create synchronization events
        let audio_event = unsafe {
            CreateEventW(None, false, false, None)
                .map_err(CaptureError::WindowsCom)?
        };

        let stop_event = unsafe {
            CreateEventW(None, false, false, None)
                .map_err(|e| {
                    let _ = CloseHandle(audio_event);
                    CaptureError::WindowsCom(e)
                })?
        };

        // Duplicate stop_event handle so main thread can signal it on stop()
        let stop_event_for_worker = stop_event;
        self.stop_event = Some(stop_event);

        // 4. Initialize IAudioClient with 10 ms buffer (100_000 hns)
        let init_res = unsafe {
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                100_000,
                0,
                pwfx,
                None,
            )
        };

        // Free WAVEFORMATEX memory allocated by COM
        unsafe {
            CoTaskMemFree(Some(pwfx as *const _));
        }

        if let Err(e) = init_res {
            unsafe {
                let _ = CloseHandle(audio_event);
                let _ = CloseHandle(stop_event_for_worker);
            }
            return Err(CaptureError::WindowsCom(e));
        }

        // 5. Associate event callback
        unsafe {
            audio_client
                .SetEventHandle(audio_event)
                .map_err(|e| {
                    let _ = CloseHandle(audio_event);
                    let _ = CloseHandle(stop_event_for_worker);
                    CaptureError::WindowsCom(e)
                })?;
        }

        // 6. Acquire capture client service
        let capture_client: IAudioCaptureClient = unsafe {
            audio_client
                .GetService()
                .map_err(|e| {
                    let _ = CloseHandle(audio_event);
                    let _ = CloseHandle(stop_event_for_worker);
                    CaptureError::WindowsCom(e)
                })?
        };

        // 7. Start audio capture client
        unsafe {
            audio_client
                .Start()
                .map_err(|e| {
                    let _ = CloseHandle(audio_event);
                    let _ = CloseHandle(stop_event_for_worker);
                    CaptureError::WindowsCom(e)
                })?;
        }

        self.is_running.store(true, Ordering::Release);

        // 2048 samples ringbuffer (~21 ms stereo @ 48kHz for low latency)
        let (producer, consumer) = rtrb::RingBuffer::new(2048);
        self.consumer = Some(consumer);

        let worker_params = AudioWorkerParams {
            client: audio_client,
            capture_client,
            audio_event,
            stop_event: stop_event_for_worker,
            format: format_info,
            meter: Arc::clone(&self.meter),
            is_running: Arc::clone(&self.is_running),
            audio_producer: Some(producer),
        };

        let target_pid = self.target_pid;
        let handle = std::thread::Builder::new()
            .name(format!("owl-capture-pid-{target_pid}"))
            .spawn(move || {
                run_capture_worker(worker_params);
            })
            .map_err(|_| CaptureError::WorkerThreadError)?;

        self.worker_handle = Some(handle);

        log::info!("Started ProcessAudioCapture for PID {}", self.target_pid);
        Ok(())
    }

    /// Stop process loopback capture and wait for worker thread to finish
    pub fn stop(&mut self) {
        if !self.is_running.swap(false, Ordering::AcqRel) {
            return;
        }

        if let Some(stop_evt) = self.stop_event.take() {
            unsafe {
                let _ = SetEvent(stop_evt);
            }
        }

        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }

        self.meter.reset();
        log::info!("Stopped ProcessAudioCapture for PID {}", self.target_pid);
    }
}

impl Drop for ProcessAudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Is `pid` a live process on this machine?
///
/// Uses `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetExitCodeProcess`, which is the
/// least-privileged pair that still answers the question for processes we do not own.
///
/// The two failure modes must be told apart:
/// - `ERROR_ACCESS_DENIED` (5) — the process exists but is protected. Report **alive**, so we
///   never refuse a capture we might legitimately be able to make.
/// - `ERROR_INVALID_PARAMETER` (87) — no such PID. Report **gone**, so `start()` fails loudly.
pub fn process_is_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// `STILL_ACTIVE` from `GetExitCodeProcess` — the process has not exited.
    const STILL_ACTIVE: u32 = 259;

    if pid == 0 {
        return false;
    }

    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => h,
            Err(e) => {
                // Only a protected-but-existing process counts as alive. Any other failure
                // (invalid PID, etc.) means there is nothing to capture.
                return e.code() == ERROR_ACCESS_DENIED.to_hresult();
            }
        };

        let mut exit_code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut exit_code).is_ok();
        let _ = CloseHandle(handle);

        if !ok {
            // We opened it, so it exists; we just cannot read its exit code.
            return true;
        }

        exit_code == STILL_ACTIVE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_is_alive_for_self() {
        assert!(
            process_is_alive(std::process::id()),
            "the current process must be reported alive"
        );
    }

    #[test]
    fn test_process_is_alive_rejects_pid_zero() {
        assert!(!process_is_alive(0), "PID 0 is never a capture target");
    }

    #[test]
    fn test_process_is_alive_rejects_impossible_pid() {
        // A PID far above any real process table entry must be reported as gone, so that
        // `ProcessAudioCapture::start` fails loudly instead of claiming a capture started.
        assert!(
            !process_is_alive(u32::MAX - 1),
            "an impossible PID must not be reported alive"
        );
    }

    #[test]
    fn test_start_fails_for_dead_pid() {
        let mut capture = ProcessAudioCapture::new(u32::MAX - 1);
        match capture.start() {
            Err(CaptureError::ProcessNotFound(pid)) => assert_eq!(pid, u32::MAX - 1),
            other => panic!("expected ProcessNotFound for a dead PID, got {other:?}"),
        }
        assert!(!capture.is_running());
    }
}
