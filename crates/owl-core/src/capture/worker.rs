//! Real-Time Audio Capture Worker Thread
//!
//! Complies with Hard Real-Time Audio discipline:
//! - MMCSS "Pro Audio" registration with time-critical thread priority
//! - Zero heap allocations inside the audio loop
//! - Zero blocking locks
//! - Zero disk/network I/O

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, AUDCLNT_BUFFERFLAGS_SILENT,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, GetCurrentThread,
    SetThreadPriority, WaitForMultipleObjects, THREAD_PRIORITY_TIME_CRITICAL,
};

use crate::format::AudioFormatInfo;
use crate::meter::AudioMeterState;

pub struct AudioWorkerParams {
    pub client: IAudioClient,
    pub capture_client: IAudioCaptureClient,
    pub audio_event: HANDLE,
    pub stop_event: HANDLE,
    pub format: AudioFormatInfo,
    pub meter: Arc<AudioMeterState>,
    pub is_running: Arc<AtomicBool>,
    pub audio_producer: Option<rtrb::Producer<f32>>,
}

// Safety: IAudioClient obtained via AgileObject completion handler in MTA is agile/free-threaded.
// Kernel event HANDLEs are thread-safe across Win32 threads.
unsafe impl Send for AudioWorkerParams {}

pub fn run_capture_worker(params: AudioWorkerParams) {
    let AudioWorkerParams {
        client,
        capture_client,
        audio_event,
        stop_event,
        format,
        meter,
        is_running,
        mut audio_producer,
    } = params;

    // 1. Elevate thread to Pro Audio MMCSS task
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

    log::info!("Audio capture worker thread initialized with MMCSS Pro Audio");

    let events = [stop_event, audio_event];

    let target_sample_rate = 48000.0;
    let ratio = format.sample_rate as f32 / target_sample_rate;
    let mut resample_phase = 0.0f32;
    let mut last_l = 0.0f32;
    let mut last_r = 0.0f32;

    while is_running.load(Ordering::Acquire) {
        // Wait up to 50ms for next audio buffer or stop signal
        let wait_res = unsafe { WaitForMultipleObjects(&events, false, 50) };

        if wait_res == WAIT_OBJECT_0 {
            // stop_event was signaled
            break;
        }

        let mut batch_peak_l = 0.0f32;
        let mut batch_peak_r = 0.0f32;
        let mut batch_sum_sq_l = 0.0f64;
        let mut batch_sum_sq_r = 0.0f64;
        let mut total_frames_read = 0usize;

        // Drain all available packets from WASAPI buffer
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
                        if let Some(ref mut prod) = audio_producer {
                            let _ = prod.push(out_l);
                            let _ = prod.push(out_r);
                        }
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
                        if let Some(ref mut prod) = audio_producer {
                            let _ = prod.push(out_l);
                            let _ = prod.push(out_r);
                        }
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
            // Decay meters smoothly when process is silent
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

    log::info!("Audio capture worker loop exited, stopping client...");

    unsafe {
        let _ = client.Stop();
        if let Some(h) = mmcss_handle {
            let _ = AvRevertMmThreadCharacteristics(h);
        }
        let _ = CloseHandle(audio_event);
        let _ = CloseHandle(stop_event);
    }
}
