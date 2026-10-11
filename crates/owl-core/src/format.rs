//! Audio format inspection and sample conversion for WASAPI streams

use windows::core::GUID;
use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVEFORMATEXTENSIBLE};

pub const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
pub const KSDATAFORMAT_SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);

const WAVE_FORMAT_PCM_TAG: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT_TAG: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE_TAG: u16 = 0xFFFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    Float32,
    Int16,
    Int24In32,
    Int24,
    Int32,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioFormatInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub bytes_per_frame: usize,
    pub sample_type: SampleType,
}

impl AudioFormatInfo {
    /// Inspect a raw WAVEFORMATEX pointer returned by WASAPI
    ///
    /// # Safety
    /// `pwfx` must be a valid, aligned pointer to WAVEFORMATEX allocated by Windows
    pub unsafe fn from_raw_wave_format(pwfx: *const WAVEFORMATEX) -> Result<Self, String> {
        if pwfx.is_null() {
            return Err("WAVEFORMATEX pointer is null".to_string());
        }

        unsafe {
            let wfx = &*pwfx;
            let channels = wfx.nChannels;
            let sample_rate = wfx.nSamplesPerSec;
            let bits_per_sample = wfx.wBitsPerSample;
            let bytes_per_frame = wfx.nBlockAlign as usize;

            let sample_type = match wfx.wFormatTag {
                WAVE_FORMAT_IEEE_FLOAT_TAG => {
                    if bits_per_sample == 32 {
                        SampleType::Float32
                    } else {
                        return Err(format!("Unsupported float bit depth: {bits_per_sample}"));
                    }
                }
                WAVE_FORMAT_PCM_TAG => match bits_per_sample {
                    16 => SampleType::Int16,
                    24 => SampleType::Int24,
                    32 => SampleType::Int32,
                    _ => return Err(format!("Unsupported PCM bit depth: {bits_per_sample}")),
                },
                WAVE_FORMAT_EXTENSIBLE_TAG => {
                    let ext = &*(pwfx as *const WAVEFORMATEXTENSIBLE);
                    let sub_format = std::ptr::addr_of!(ext.SubFormat).read_unaligned();

                    if sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                        SampleType::Float32
                    } else if sub_format == KSDATAFORMAT_SUBTYPE_PCM {
                        match bits_per_sample {
                            16 => SampleType::Int16,
                            24 => {
                                if bytes_per_frame == (channels as usize) * 4 {
                                    SampleType::Int24In32
                                } else {
                                    SampleType::Int24
                                }
                            }
                            32 => SampleType::Int32,
                            _ => return Err(format!("Unsupported extensible PCM bit depth: {bits_per_sample}")),
                        }
                    } else {
                        return Err(format!("Unsupported extensible SubFormat GUID: {sub_format:?}"));
                    }
                }
                other => return Err(format!("Unknown WAVEFORMATEX format tag: 0x{other:04X}")),
            };

            Ok(Self {
                channels,
                sample_rate,
                bits_per_sample,
                bytes_per_frame,
                sample_type,
            })
        }
    }

    /// Read (left, right) sample as f32 from a specific frame offset in byte buffer
    ///
    /// # Safety
    /// `frame_ptr` must point to at least `bytes_per_frame` valid bytes
    #[inline(always)]
    pub unsafe fn read_stereo_frame(&self, frame_ptr: *const u8) -> (f32, f32) {
        unsafe {
            match self.sample_type {
                SampleType::Float32 => {
                    let f_ptr = frame_ptr as *const f32;
                    let l = *f_ptr;
                    let r = if self.channels > 1 { *f_ptr.add(1) } else { l };
                    (l, r)
                }
                SampleType::Int16 => {
                    let s_ptr = frame_ptr as *const i16;
                    // Fix Bug 2: Proper MAX_INT division
                    let l = (*s_ptr as f32) / 32767.0;
                    let r = if self.channels > 1 { (*s_ptr.add(1) as f32) / 32767.0 } else { l };
                    (l, r)
                }
                SampleType::Int24In32 => {
                    let s_ptr = frame_ptr as *const i32;
                    let l = ((*s_ptr >> 8) as f32) / 8388607.0;
                    let r = if self.channels > 1 { ((*s_ptr.add(1) >> 8) as f32) / 8388607.0 } else { l };
                    (l, r)
                }
                SampleType::Int32 => {
                    let s_ptr = frame_ptr as *const i32;
                    let l = (*s_ptr as f32) / 2147483647.0;
                    let r = if self.channels > 1 { (*s_ptr.add(1) as f32) / 2147483647.0 } else { l };
                    (l, r)
                }
                SampleType::Int24 => {
                    let l_bytes = [0u8, *frame_ptr, *frame_ptr.add(1), *frame_ptr.add(2)];
                    let l_val = i32::from_ne_bytes(l_bytes) >> 8;
                    let l = (l_val as f32) / 8388607.0;

                    let r = if self.channels > 1 {
                        let r_ptr = frame_ptr.add(3);
                        let r_bytes = [0u8, *r_ptr, *r_ptr.add(1), *r_ptr.add(2)];
                        let r_val = i32::from_ne_bytes(r_bytes) >> 8;
                        (r_val as f32) / 8388607.0
                    } else {
                        l
                    };
                    (l, r)
                }
            }
        }
    }

    /// Write (left, right) sample as f32 into a specific frame offset in byte buffer
    ///
    /// # Safety
    /// `frame_ptr` must point to at least `bytes_per_frame` valid, writable bytes
    #[inline(always)]
    pub unsafe fn write_stereo_frame(&self, frame_ptr: *mut u8, left: f32, right: f32) {
        unsafe {
            match self.sample_type {
                SampleType::Float32 => {
                    let f_ptr = frame_ptr as *mut f32;
                    *f_ptr = left;
                    if self.channels > 1 {
                        *f_ptr.add(1) = right;
                    }
                }
                SampleType::Int16 => {
                    let s_ptr = frame_ptr as *mut i16;
                    let l = (left.clamp(-1.0, 1.0) * 32767.0) as i16;
                    *s_ptr = l;
                    if self.channels > 1 {
                        let r = (right.clamp(-1.0, 1.0) * 32767.0) as i16;
                        *s_ptr.add(1) = r;
                    }
                }
                SampleType::Int32 => {
                    let s_ptr = frame_ptr as *mut i32;
                    let l = (left.clamp(-1.0, 1.0) * 2147483647.0) as i32;
                    *s_ptr = l;
                    if self.channels > 1 {
                        let r = (right.clamp(-1.0, 1.0) * 2147483647.0) as i32;
                        *s_ptr.add(1) = r;
                    }
                }
                SampleType::Int24In32 => {
                    let s_ptr = frame_ptr as *mut i32;
                    let l = ((left.clamp(-1.0, 1.0) * 8388607.0) as i32) << 8;
                    *s_ptr = l;
                    if self.channels > 1 {
                        let r = ((right.clamp(-1.0, 1.0) * 8388607.0) as i32) << 8;
                        *s_ptr.add(1) = r;
                    }
                }
                SampleType::Int24 => {
                    let l = (left.clamp(-1.0, 1.0) * 8388607.0) as i32;
                    let l_bytes = l.to_ne_bytes();
                    *frame_ptr = l_bytes[0];
                    *frame_ptr.add(1) = l_bytes[1];
                    *frame_ptr.add(2) = l_bytes[2];
                    if self.channels > 1 {
                        let r = (right.clamp(-1.0, 1.0) * 8388607.0) as i32;
                        let r_bytes = r.to_ne_bytes();
                        let r_ptr = frame_ptr.add(3);
                        *r_ptr = r_bytes[0];
                        *r_ptr.add(1) = r_bytes[1];
                        *r_ptr.add(2) = r_bytes[2];
                    }
                }
            }
        }
    }
}
