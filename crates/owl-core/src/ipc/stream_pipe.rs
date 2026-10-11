//! Direct OBS Studio PCM Stream Mix Shared Memory Pipe
//!
//! High-speed, lock-free SPSC (Single Producer Single Consumer) ring buffer in Windows Shared Memory.
//! Zero-Driver low latency (< 0.1 ms) audio delivery to OBS Studio plugin (`obs-openwavelink-source`).

use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use anyhow::{bail, Context, Result};
use owl_ipc::{
    StreamPipeHeader, SHARED_STREAM_PIPE_NAME, SHARED_MICFX_PIPE_NAME, STREAM_PIPE_DEFAULT_CAPACITY_FRAMES,
    STREAM_PIPE_MAGIC,
};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

/// Lock-free Shared Memory SPSC Producer for OBS Studio
pub struct StreamPipePublisher {
    handle: HANDLE,
    header_ptr: NonNull<StreamPipeHeader>,
    data_ptr: *mut [f32; 2],
    capacity_frames: usize,
    _name: String,
}

unsafe impl Send for StreamPipePublisher {}
unsafe impl Sync for StreamPipePublisher {}

impl StreamPipePublisher {
    /// Create publisher on the standard shared memory pipe (`Local\OpenWaveLink_StreamMix_PCM`)
    pub fn new() -> Result<Self> {
        Self::with_capacity(SHARED_STREAM_PIPE_NAME, STREAM_PIPE_DEFAULT_CAPACITY_FRAMES)
    }

    /// Create publisher for processed mic (`Local\OpenWaveLink_MicFX_PCM`)
    pub fn new_mic_fx() -> Result<Self> {
        Self::with_capacity(SHARED_MICFX_PIPE_NAME, STREAM_PIPE_DEFAULT_CAPACITY_FRAMES)
    }

    /// Create publisher with custom name and capacity
    pub fn with_capacity(name: &str, capacity_frames: usize) -> Result<Self> {
        if capacity_frames == 0 || (capacity_frames & (capacity_frames - 1)) != 0 {
            bail!("capacity_frames must be a power of two, got {}", capacity_frames);
        }

        let header_size = std::mem::size_of::<StreamPipeHeader>();
        let data_size = capacity_frames * std::mem::size_of::<[f32; 2]>();
        let total_size = header_size + data_size;

        let wide_name = HSTRING::from(name);

        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                total_size as u32,
                PCWSTR(wide_name.as_ptr()),
            )
        }
        .context(format!("CreateFileMappingW failed for {name}"))?;

        if handle.is_invalid() {
            bail!("CreateFileMappingW returned invalid handle for {name}");
        }

        let view_address = unsafe {
            MapViewOfFile(
                handle,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                total_size,
            )
        };

        if view_address.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            bail!("MapViewOfFile failed for {name}");
        }

        let base_ptr = view_address.Value as *mut u8;
        let header_ptr = NonNull::new(base_ptr as *mut StreamPipeHeader)
            .expect("view_address is non-null");
        let data_ptr = unsafe { base_ptr.add(header_size) as *mut [f32; 2] };

        // Initialize header
        let initial_header = StreamPipeHeader {
            magic: STREAM_PIPE_MAGIC,
            sample_rate: 48000,
            channels: 2,
            capacity_frames: capacity_frames as u32,
            write_pos: 0,
            read_pos: 0,
            _pad: [0u8; 32],
        };

        unsafe {
            header_ptr.as_ptr().write_volatile(initial_header);
            // Clear sample buffer
            std::ptr::write_bytes(data_ptr, 0, capacity_frames);
        }

        Ok(Self {
            handle,
            header_ptr,
            data_ptr,
            capacity_frames,
            _name: name.to_string(),
        })
    }

    /// Access atomic write position in shared memory
    #[inline]
    fn write_pos_atomic(&self) -> &AtomicU64 {
        unsafe {
            let offset = std::mem::offset_of!(StreamPipeHeader, write_pos);
            let ptr = (self.header_ptr.as_ptr() as *const u8).add(offset) as *const AtomicU64;
            &*ptr
        }
    }

    /// Access atomic read position in shared memory
    #[inline]
    fn read_pos_atomic(&self) -> &AtomicU64 {
        unsafe {
            let offset = std::mem::offset_of!(StreamPipeHeader, read_pos);
            let ptr = (self.header_ptr.as_ptr() as *const u8).add(offset) as *const AtomicU64;
            &*ptr
        }
    }

    /// Continuous push of stereo Float32 frames from Stream Mix bus.
    ///
    /// Hard real-time discipline: Zero-allocation, non-blocking, lock-free SPSC.
    pub fn write_stream_frames(&mut self, frames: &[[f32; 2]]) {
        if frames.is_empty() {
            return;
        }

        let count = frames.len();
        let write_pos = self.write_pos_atomic().load(Ordering::Relaxed);
        let mask = (self.capacity_frames - 1) as u64;

        // Write frames directly to ring buffer slice
        for (i, frame) in frames.iter().enumerate() {
            let target_idx = ((write_pos + i as u64) & mask) as usize;
            unsafe {
                *self.data_ptr.add(target_idx) = *frame;
            }
        }

        // Release-store to publish newly written samples to consumer
        self.write_pos_atomic().store(write_pos + count as u64, Ordering::Release);
    }

    /// Push interleaved stereo f32 samples ([L0, R0, L1, R1, ...])
    pub fn write_interleaved(&mut self, samples: &[f32]) {
        let frame_count = samples.len() / 2;
        if frame_count == 0 {
            return;
        }

        let write_pos = self.write_pos_atomic().load(Ordering::Relaxed);
        let mask = (self.capacity_frames - 1) as u64;

        for f in 0..frame_count {
            let l = samples[f * 2];
            let r = samples[f * 2 + 1];
            let target_idx = ((write_pos + f as u64) & mask) as usize;
            unsafe {
                *self.data_ptr.add(target_idx) = [l, r];
            }
        }

        self.write_pos_atomic().store(write_pos + frame_count as u64, Ordering::Release);
    }

    /// Ring buffer capacity in frames
    pub fn capacity(&self) -> usize {
        self.capacity_frames
    }

    /// Number of frames currently pending for the consumer
    pub fn pending_frames(&self) -> usize {
        let w = self.write_pos_atomic().load(Ordering::Relaxed);
        let r = self.read_pos_atomic().load(Ordering::Acquire);
        if w >= r {
            (w - r) as usize
        } else {
            0
        }
    }
}

impl Drop for StreamPipePublisher {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.header_ptr.as_ptr() as *mut _,
            });
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Lock-free Shared Memory SPSC Consumer for OBS Studio or test readers
pub struct StreamPipeReader {
    handle: HANDLE,
    header_ptr: NonNull<StreamPipeHeader>,
    data_ptr: *const [f32; 2],
    capacity_frames: usize,
    local_read_pos: u64,
    _name: String,
}

unsafe impl Send for StreamPipeReader {}
unsafe impl Sync for StreamPipeReader {}

impl StreamPipeReader {
    /// Open reader on default shared memory pipe (`Local\OpenWaveLink_StreamMix_PCM`)
    pub fn open() -> Result<Self> {
        Self::open_named(SHARED_STREAM_PIPE_NAME)
    }

    /// Open reader on named shared memory pipe
    pub fn open_named(name: &str) -> Result<Self> {
        let wide_name = HSTRING::from(name);

        let handle = unsafe {
            OpenFileMappingW(
                FILE_MAP_ALL_ACCESS.0,
                false,
                PCWSTR(wide_name.as_ptr()),
            )
        }
        .context(format!("OpenFileMappingW failed for {name}"))?;

        if handle.is_invalid() {
            bail!("OpenFileMappingW returned invalid handle for {name}");
        }

        // Map view with full access so we can update read_pos
        let view_address = unsafe {
            MapViewOfFile(
                handle,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                0, // map entire file mapping
            )
        };

        if view_address.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            bail!("MapViewOfFile failed for {name}");
        }

        let base_ptr = view_address.Value as *mut u8;
        let header_ptr = NonNull::new(base_ptr as *mut StreamPipeHeader)
            .expect("view_address is non-null");

        let header = unsafe { header_ptr.as_ptr().read_volatile() };
        if header.magic != STREAM_PIPE_MAGIC {
            unsafe {
                let _ = UnmapViewOfFile(view_address);
                let _ = CloseHandle(handle);
            }
            bail!("Invalid StreamPipe magic: 0x{:08X}", header.magic);
        }

        let capacity = header.capacity_frames as usize;
        let header_size = std::mem::size_of::<StreamPipeHeader>();
        let data_ptr = unsafe { base_ptr.add(header_size) as *const [f32; 2] };

        let initial_read = header.read_pos;

        Ok(Self {
            handle,
            header_ptr,
            data_ptr,
            capacity_frames: capacity,
            local_read_pos: initial_read,
            _name: name.to_string(),
        })
    }

    #[inline]
    fn write_pos_atomic(&self) -> &AtomicU64 {
        unsafe {
            let offset = std::mem::offset_of!(StreamPipeHeader, write_pos);
            let ptr = (self.header_ptr.as_ptr() as *const u8).add(offset) as *const AtomicU64;
            &*ptr
        }
    }

    #[inline]
    fn read_pos_atomic(&self) -> &AtomicU64 {
        unsafe {
            let offset = std::mem::offset_of!(StreamPipeHeader, read_pos);
            let ptr = (self.header_ptr.as_ptr() as *const u8).add(offset) as *const AtomicU64;
            &*ptr
        }
    }

    /// Read available stereo frames into `out_frames`. Returns number of frames read.
    pub fn read_stream_frames(&mut self, out_frames: &mut [[f32; 2]]) -> usize {
        if out_frames.is_empty() {
            return 0;
        }

        let write_pos = self.write_pos_atomic().load(Ordering::Acquire);
        let mut read_pos = self.local_read_pos;

        // Check for buffer overrun (producer wrapped around past reader)
        let capacity = self.capacity_frames as u64;
        if write_pos > read_pos + capacity {
            // Overrun: skip unread samples to latest valid window
            read_pos = write_pos - capacity;
        }

        let available = (write_pos.saturating_sub(read_pos)) as usize;
        let to_read = available.min(out_frames.len());

        if to_read == 0 {
            return 0;
        }

        let mask = (self.capacity_frames - 1) as u64;

        for (i, slot) in out_frames[..to_read].iter_mut().enumerate() {
            let src_idx = ((read_pos + i as u64) & mask) as usize;
            unsafe {
                *slot = *self.data_ptr.add(src_idx);
            }
        }

        let new_read_pos = read_pos + to_read as u64;
        self.local_read_pos = new_read_pos;
        self.read_pos_atomic().store(new_read_pos, Ordering::Release);

        to_read
    }

    /// Check number of readable frames
    pub fn available_frames(&self) -> usize {
        let w = self.write_pos_atomic().load(Ordering::Acquire);
        let r = self.local_read_pos;
        (w.saturating_sub(r)) as usize
    }
}

impl Drop for StreamPipeReader {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.header_ptr.as_ptr() as *mut _,
            });
            let _ = CloseHandle(self.handle);
        }
    }
}
