//! Hardware WASAPI Microphone Capture
//!
//! Captures audio from the default physical input device (eCapture) using
//! low-latency event-driven shared mode with MMCSS Pro Audio priority and
//! streams 32-bit float stereo samples into a lock-free SPSC ring buffer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::w;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eMultimedia, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
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
use crate::meter::AudioMeterState;

pub struct MicrophoneCapture {
    pub format: Option<AudioFormatInfo>,
    meter: Arc<AudioMeterState>,
    is_running: Arc<AtomicBool>,
    consumer: Option<rtrb::Consumer<f32>>,
    stop_event: Option<HANDLE>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
    /// Explicit capture endpoint id. `None` means the system default.
    endpoint_id: Option<String>,
}

// Safety: MicrophoneCapture manages its own thread-safe handles and atomic state
unsafe impl Send for MicrophoneCapture {}
unsafe impl Sync for MicrophoneCapture {}

impl MicrophoneCapture {
    /// Create a new MicrophoneCapture instance bound to the system default
    /// capture endpoint.
    pub fn new() -> Result<Self, CaptureError> {
        Ok(Self {
            format: None,
            meter: Arc::new(AudioMeterState::new()),
            is_running: Arc::new(AtomicBool::new(false)),
            consumer: None,
            stop_event: None,
            worker_handle: None,
            endpoint_id: None,
        })
    }

    /// Create a capture stream bound to a specific capture endpoint.
    ///
    /// This is how a virtual cable is read: the cable exposes an ordinary
    /// capture endpoint, and its endpoint id is passed here instead of relying
    /// on the system default microphone.
    pub fn new_with_device(endpoint_id: &str) -> Result<Self, CaptureError> {
        let mut capture = Self::new()?;
        capture.endpoint_id = Some(endpoint_id.to_string());
        Ok(capture)
    }

    /// Access meter state for real-time VU monitoring
    pub fn meter(&self) -> Arc<AudioMeterState> {
        Arc::clone(&self.meter)
    }

    /// Check if audio capture thread is currently running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Acquire)
    }

    /// Take the lock-free consumer to read captured 32-bit float stereo samples
    pub fn take_consumer(&mut self) -> Option<rtrb::Consumer<f32>> {
        self.consumer.take()
    }

    /// Start microphone audio capture
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

        // An explicit endpoint id (a virtual cable) wins over the system
        // default, so a cable is captured by identity rather than by luck.
        let endpoint = match self.endpoint_id.as_deref() {
            Some(id) => unsafe {
                device_enum
                    .GetDevice(&HSTRING::from(id))
                    .map_err(CaptureError::WindowsCom)?
            },
            None => unsafe {
                device_enum
                    .GetDefaultAudioEndpoint(eCapture, eConsole)
                    .or_else(|_| device_enum.GetDefaultAudioEndpoint(eCapture, eMultimedia))
                    .map_err(CaptureError::WindowsCom)?
            },
        };

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

        // Initialize with 10 ms buffer (100_000 hns)
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

        let capture_client: IAudioCaptureClient = unsafe {
            client.GetService().map_err(|e| {
                let _ = CloseHandle(audio_event);
                let _ = CloseHandle(stop_event_for_worker);
                CaptureError::WindowsCom(e)
            })?
        };

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
        self.consumer = Some(consumer);

        let worker_params = MicWorkerParams {
            client,
            capture_client,
            audio_event,
            stop_event: stop_event_for_worker,
            format: format_info,
            meter: Arc::clone(&self.meter),
            is_running: Arc::clone(&self.is_running),
            producer,
        };

        let handle = std::thread::Builder::new()
            .name("owl-mic-capture".to_string())
            .spawn(move || {
                run_mic_capture_worker(worker_params);
            })
            .map_err(|_| CaptureError::WorkerThreadError)?;

        self.worker_handle = Some(handle);
        log::info!("Started MicrophoneCapture on default input device");
        Ok(())
    }

    /// Stop microphone audio capture
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
        log::info!("Stopped MicrophoneCapture");
    }
}

impl Drop for MicrophoneCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

struct MicWorkerParams {
    client: IAudioClient,
    capture_client: IAudioCaptureClient,
    audio_event: HANDLE,
    stop_event: HANDLE,
    format: AudioFormatInfo,
    meter: Arc<AudioMeterState>,
    is_running: Arc<AtomicBool>,
    producer: rtrb::Producer<f32>,
}

unsafe impl Send for MicWorkerParams {}

fn run_mic_capture_worker(params: MicWorkerParams) {
    let MicWorkerParams {
        client,
        capture_client,
        audio_event,
        stop_event,
        format,
        meter,
        is_running,
        mut producer,
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

    let target_sample_rate = 48000.0;
    let ratio = format.sample_rate as f32 / target_sample_rate;
    let mut resample_phase = 0.0f32;
    let mut last_l = 0.0f32;
    let mut last_r = 0.0f32;

    while is_running.load(Ordering::Acquire) {
        let wait_res = unsafe { WaitForMultipleObjects(&events, false, 50) };
        if wait_res == WAIT_OBJECT_0 {
            break;
        }

        let mut batch_peak_l = 0.0f32;
        let mut batch_peak_r = 0.0f32;
        let mut batch_sum_sq_l = 0.0f64;
        let mut batch_sum_sq_r = 0.0f64;
        let mut total_frames_read = 0usize;

        loop {
            // Fix Bug 2: Proper `GetNextPacketSize` validation before GetBuffer
            let packet_length = unsafe { capture_client.GetNextPacketSize() }.unwrap_or(0);
            if packet_length == 0 {
                break;
            }

            let mut p_data: *mut u8 = std::ptr::null_mut();
            let mut num_frames: u32 = 0;
            let mut flags: u32 = 0;

            let hr = unsafe {
                capture_client.GetBuffer(
                    &mut p_data,
                    &mut num_frames,
                    &mut flags,
                    None,
                    None,
                )
            };

            if hr.is_err() || num_frames == 0 {
                break;
            }

            let silent = (flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)) != 0 || p_data.is_null();

            if !silent {
                let frame_size = format.bytes_per_frame;
                for frame_idx in 0..num_frames as usize {
                    let frame_ptr = unsafe { p_data.add(frame_idx * frame_size) };
                    let (s_l, s_r) = unsafe { format.read_stereo_frame(frame_ptr) };

                    // Fix Bug 2: Simple linear resampler for sample rate mismatch
                    while resample_phase < 1.0 {
                        let out_l = last_l + (s_l - last_l) * resample_phase;
                        let out_r = last_r + (s_r - last_r) * resample_phase;
                        let _ = producer.push(out_l);
                        let _ = producer.push(out_r);
                        resample_phase += ratio;
                    }
                    resample_phase -= 1.0;
                    last_l = s_l;
                    last_r = s_r;

                    let abs_l = s_l.abs();
                    let abs_r = s_r.abs();
                    if abs_l > batch_peak_l {
                        batch_peak_l = abs_l;
                    }
                    if abs_r > batch_peak_r {
                        batch_peak_r = abs_r;
                    }
                    batch_sum_sq_l += (s_l * s_l) as f64;
                    batch_sum_sq_r += (s_r * s_r) as f64;
                }
            } else {
                for _ in 0..num_frames as usize {
                    let (s_l, s_r) = (0.0, 0.0);
                    // Fix Bug 2: Simple linear resampler for sample rate mismatch
                    while resample_phase < 1.0 {
                        let out_l = last_l + (s_l - last_l) * resample_phase;
                        let out_r = last_r + (s_r - last_r) * resample_phase;
                        let _ = producer.push(out_l);
                        let _ = producer.push(out_r);
                        resample_phase += ratio;
                    }
                    resample_phase -= 1.0;
                    last_l = s_l;
                    last_r = s_r;
                }
            }

            total_frames_read += num_frames as usize;
            let _ = unsafe { capture_client.ReleaseBuffer(num_frames) };
        }

        if total_frames_read > 0 {
            let rms_l = (batch_sum_sq_l / (total_frames_read as f64)).sqrt() as f32;
            let rms_r = (batch_sum_sq_r / (total_frames_read as f64)).sqrt() as f32;
            let clip = batch_peak_l >= 0.999 || batch_peak_r >= 0.999;
            meter.update(batch_peak_l, batch_peak_r, rms_l, rms_r, clip);
        } else if wait_res == WAIT_TIMEOUT {
            let prev = meter.read();
            if prev.peak_l > 0.0001 || prev.peak_r > 0.0001 {
                let decayed_peak_l = (prev.peak_l * 0.88).max(0.0);
                let decayed_peak_r = (prev.peak_r * 0.88).max(0.0);
                let decayed_rms_l = (prev.rms_l * 0.85).max(0.0);
                let decayed_rms_r = (prev.rms_r * 0.85).max(0.0);
                meter.update(decayed_peak_l, decayed_peak_r, decayed_rms_l, decayed_rms_r, false);
            }
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
