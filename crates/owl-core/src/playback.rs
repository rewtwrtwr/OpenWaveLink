//! WASAPI Low-Latency Audio Playback (Monitor Mix)
//!
//! Renders 32-bit float audio to the default Windows playback endpoint (eRender)
//! with event-driven callback buffer filling and MMCSS Pro Audio scheduling.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eMultimedia, eRender, IAudioClient, IAudioRenderClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    GetCurrentThread, SetEvent, SetThreadPriority, WaitForMultipleObjects,
    THREAD_PRIORITY_TIME_CRITICAL,
};

use crate::error::CaptureError;
use crate::format::AudioFormatInfo;

pub struct WasapiPlayback {
    pub format: Option<AudioFormatInfo>,
    is_running: Arc<AtomicBool>,
    producer: Option<rtrb::Producer<f32>>,
    stop_event: Option<HANDLE>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
    /// Endpoint id requested at construction (`None` = system default eRender/eConsole).
    requested_endpoint_id: Option<String>,
    /// Endpoint id actually bound after `start()`.
    bound_endpoint_id: Option<String>,
    /// Friendly name of the endpoint bound after `start()`.
    device_name: Option<String>,
}

// Safety: WasapiPlayback manages its own thread-safe handles and atomic state
unsafe impl Send for WasapiPlayback {}
unsafe impl Sync for WasapiPlayback {}

impl WasapiPlayback {
    /// Create a new WasapiPlayback instance for default audio output
    pub fn new() -> Result<Self, CaptureError> {
        Self::new_with_device(None)
    }

    /// Bind to a specific CoreAudio render endpoint id (`IMMDeviceEnumerator::GetDevice`).
    ///
    /// `None` means the default `eRender`/`eConsole` endpoint (identical to `new()`).
    /// The endpoint is resolved in `start()` — this constructor performs no COM work.
    pub fn new_with_device(endpoint_id: Option<&str>) -> Result<Self, CaptureError> {
        Ok(Self {
            format: None,
            is_running: Arc::new(AtomicBool::new(false)),
            producer: None,
            stop_event: None,
            worker_handle: None,
            requested_endpoint_id: endpoint_id.map(|s| s.to_string()),
            bound_endpoint_id: None,
            device_name: None,
        })
    }

    /// Human-readable name of the endpoint currently bound (after `start()`).
    pub fn device_name(&self) -> Option<&str> {
        self.device_name.as_deref()
    }

    /// Endpoint id actually bound (after `start()`).
    ///
    /// For a default-endpoint binding this is the *resolved* id, which is what the control
    /// plane needs to report back to the UI.
    pub fn endpoint_id(&self) -> Option<&str> {
        self.bound_endpoint_id.as_deref()
    }

    /// Check if audio playback thread is running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Acquire)
    }

    /// Take the lock-free audio producer to feed 32-bit float stereo samples into playback
    pub fn take_producer(&mut self) -> Option<rtrb::Producer<f32>> {
        self.producer.take()
    }

    /// Start low-latency playback stream
    pub fn start(&mut self) -> Result<(), CaptureError> {
        if self.is_running() {
            return Ok(());
        }

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let device_enum: IMMDeviceEnumerator = unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(CaptureError::WindowsCom)?
        };

        let endpoint = match self.requested_endpoint_id.as_deref() {
            // Explicit endpoint: a failure here must surface, never silently fall back to the
            // default — the UI would otherwise show one device while audio plays to another.
            Some(id) => unsafe {
                device_enum
                    .GetDevice(&HSTRING::from(id))
                    .map_err(CaptureError::WindowsCom)?
            },
            None => unsafe {
                device_enum
                    .GetDefaultAudioEndpoint(eRender, eConsole)
                    .or_else(|_| device_enum.GetDefaultAudioEndpoint(eRender, eMultimedia))
                    .map_err(CaptureError::WindowsCom)?
            },
        };

        // Record what we actually bound, for `device_name()` / `endpoint_id()`.
        let bound_id = unsafe {
            endpoint.GetId().ok().map(|pwstr| {
                let s = pwstr.to_string().unwrap_or_default();
                CoTaskMemFree(Some(pwstr.as_ptr() as *const _));
                s
            })
        };
        let bound_name = bound_id
            .as_deref()
            .and_then(crate::devices::resolve_endpoint_name);
        self.bound_endpoint_id = bound_id;
        self.device_name = bound_name;

        let client: IAudioClient = unsafe {
            endpoint
                .Activate(CLSCTX_ALL, None)
                .map_err(CaptureError::WindowsCom)?
        };

        let pwfx = unsafe {
            client
                .GetMixFormat()
                .map_err(CaptureError::WindowsCom)?
        };

        let format_info = unsafe {
            AudioFormatInfo::from_raw_wave_format(pwfx)
                .map_err(CaptureError::UnsupportedFormat)?
        };
        self.format = Some(format_info);

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

        let stop_event_for_worker = stop_event;
        self.stop_event = Some(stop_event);

        // 10 ms buffer period (100_000 hns)
        let init_res = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                100_000,
                0,
                pwfx,
                None,
            )
        };

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

        unsafe {
            client.SetEventHandle(audio_event).map_err(|e| {
                let _ = CloseHandle(audio_event);
                let _ = CloseHandle(stop_event_for_worker);
                CaptureError::WindowsCom(e)
            })?;
        }

        let buffer_size_frames = unsafe {
            client.GetBufferSize().map_err(|e| {
                let _ = CloseHandle(audio_event);
                let _ = CloseHandle(stop_event_for_worker);
                CaptureError::WindowsCom(e)
            })?
        };

        let render_client: IAudioRenderClient = unsafe {
            client.GetService().map_err(|e| {
                let _ = CloseHandle(audio_event);
                let _ = CloseHandle(stop_event_for_worker);
                CaptureError::WindowsCom(e)
            })?
        };

        // Pre-fill buffer with silence before starting playback to avoid initial underrun
        unsafe {
            if let Ok(p_data) = render_client.GetBuffer(buffer_size_frames)
                && !p_data.is_null() {
                    std::ptr::write_bytes(p_data, 0, buffer_size_frames as usize * format_info.bytes_per_frame);
                    let _ = render_client.ReleaseBuffer(buffer_size_frames, 0);
                }
        }

        unsafe {
            client.Start().map_err(|e| {
                let _ = CloseHandle(audio_event);
                let _ = CloseHandle(stop_event_for_worker);
                CaptureError::WindowsCom(e)
            })?;
        }

        self.is_running.store(true, Ordering::Release);

        // 2048 samples ringbuffer (~21 ms stereo @ 48kHz for low latency)
        let (producer, consumer) = rtrb::RingBuffer::new(2048);
        self.producer = Some(producer);

        let is_running = Arc::clone(&self.is_running);

        let worker_params = PlaybackWorkerParams {
            client,
            render_client,
            audio_event,
            stop_event: stop_event_for_worker,
            format: format_info,
            buffer_size_frames,
            is_running,
            consumer,
        };

        let handle = std::thread::Builder::new()
            .name("owl-playback-render".to_string())
            .spawn(move || {
                run_playback_worker(worker_params);
            })
            .map_err(|_| CaptureError::WorkerThreadError)?;

        self.worker_handle = Some(handle);
        // Name the endpoint actually bound. Saying "default render device" unconditionally is
        // actively misleading when an explicit endpoint was requested — and this log line is the
        // first thing read when diagnosing "no sound".
        match self.device_name.as_deref() {
            Some(name) => log::info!(
                "Started WasapiPlayback on '{name}' (buffer: {buffer_size_frames} frames)"
            ),
            None => log::info!(
                "Started WasapiPlayback on the default render device (buffer: {buffer_size_frames} frames)"
            ),
        }
        Ok(())
    }

    /// Stop audio playback
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

        log::info!("Stopped WasapiPlayback");
    }
}

impl Drop for WasapiPlayback {
    fn drop(&mut self) {
        self.stop();
    }
}

struct PlaybackWorkerParams {
    client: IAudioClient,
    render_client: IAudioRenderClient,
    audio_event: HANDLE,
    stop_event: HANDLE,
    format: AudioFormatInfo,
    buffer_size_frames: u32,
    is_running: Arc<AtomicBool>,
    consumer: rtrb::Consumer<f32>,
}

unsafe impl Send for PlaybackWorkerParams {}

fn run_playback_worker(params: PlaybackWorkerParams) {
    let PlaybackWorkerParams {
        client,
        render_client,
        audio_event,
        stop_event,
        format,
        buffer_size_frames,
        is_running,
        mut consumer,
    } = params;

    let mut task_index = 0u32;
    let mmcss_handle = unsafe {
        let h = AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index);
        if h.is_err() {
            AvSetMmThreadCharacteristicsW(w!("Audio"), &mut task_index).ok()
        } else {
            h.ok()
        }
    };

    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }

    let events = [stop_event, audio_event];

    while is_running.load(Ordering::Acquire) {
        let wait_res = unsafe { WaitForMultipleObjects(&events, false, 50) };
        if wait_res == WAIT_OBJECT_0 {
            break;
        }

        let padding = unsafe { client.GetCurrentPadding().unwrap_or(buffer_size_frames) };
        let num_frames_needed = buffer_size_frames.saturating_sub(padding);

        if num_frames_needed > 0
            && let Ok(p_data) = unsafe { render_client.GetBuffer(num_frames_needed) }
                && !p_data.is_null() {
                    let frame_size = format.bytes_per_frame;

                    for frame_idx in 0..num_frames_needed as usize {
                        let (s_l, s_r) = match (consumer.pop(), consumer.pop()) {
                            (Ok(l), Ok(r)) => (l, r),
                            (Ok(l), Err(_)) => (l, 0.0),
                            _ => (0.0, 0.0), // Underrun / silence
                        };

                        let frame_ptr = unsafe { p_data.add(frame_idx * frame_size) };
                        unsafe {
                            format.write_stereo_frame(frame_ptr, s_l, s_r);
                        }
                    }

                    let _ = unsafe { render_client.ReleaseBuffer(num_frames_needed, 0) };
                }
    }

    unsafe {
        let _ = client.Stop();
        if let Some(h) = mmcss_handle {
            let _ = AvRevertMmThreadCharacteristics(h);
        }
        let _ = CloseHandle(audio_event);
        let _ = CloseHandle(stop_event);
    }
}
