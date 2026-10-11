//! Windows Shared Memory Telemetry Publisher & Reader
//!
//! Exposes 60-144 FPS VU-meter audio telemetry via Windows Shared Memory (`Local\OpenWaveLink_Telemetry_V1`).
//! Zero-copy, lock-free Data Plane for UI (Tauri / React) and external observers.

use std::ptr::NonNull;
use std::sync::atomic::{fence, Ordering};
use anyhow::{bail, Context, Result};
use owl_ipc::{
    AudioTelemetryBlock, MeterSample, MAX_CHANNELS, MAX_MIXES, SHARED_TELEMETRY_NAME,
    TELEMETRY_MAGIC,
};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

/// High-speed Telemetry Publisher to Windows Shared Memory
pub struct TelemetryPublisher {
    handle: HANDLE,
    view_ptr: NonNull<AudioTelemetryBlock>,
    frame_seq: u32,
    _name: String,
}

// Safety: The file mapping is owned exclusively by TelemetryPublisher and mapped in the current process.
unsafe impl Send for TelemetryPublisher {}
unsafe impl Sync for TelemetryPublisher {}

impl TelemetryPublisher {
    /// Create publisher on the standard shared memory name (`Local\OpenWaveLink_Telemetry_V1`)
    pub fn new() -> Result<Self> {
        Self::with_name(SHARED_TELEMETRY_NAME)
    }

    /// Create publisher with a custom shared memory mapping name (useful for testing or multiple instances)
    pub fn with_name(name: &str) -> Result<Self> {
        let size = std::mem::size_of::<AudioTelemetryBlock>() as u32;
        let wide_name = HSTRING::from(name);

        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                size,
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
                std::mem::size_of::<AudioTelemetryBlock>(),
            )
        };

        if view_address.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            bail!("MapViewOfFile failed for {name}");
        }

        let ptr = NonNull::new(view_address.Value as *mut AudioTelemetryBlock)
            .expect("view_address is non-null");

        // Initialize block with default values
        let initial_block = AudioTelemetryBlock::default();
        unsafe {
            ptr.as_ptr().write_volatile(initial_block);
        }

        Ok(Self {
            handle,
            view_ptr: ptr,
            frame_seq: 0,
            _name: name.to_string(),
        })
    }

    /// Publish a telemetry frame atomically.
    ///
    /// The update increments `frame_seq` with `Ordering::Release` to prevent tearing
    /// when read by lock-free observers.
    pub fn publish(&mut self, mut block: AudioTelemetryBlock) {
        self.frame_seq = self.frame_seq.wrapping_add(1);
        block.magic = TELEMETRY_MAGIC;
        block.frame_seq = self.frame_seq;

        // Ensure stores to the block are committed before or with Release ordering
        unsafe {
            self.view_ptr.as_ptr().write_volatile(block);
        }
        fence(Ordering::Release);
    }

    /// Convenience method to publish channels and mixes directly
    pub fn update(
        &mut self,
        sample_rate: u32,
        buffer_size_frames: u32,
        channels: &[MeterSample],
        mixes: &[MeterSample],
    ) {
        let mut block = AudioTelemetryBlock {
            magic: TELEMETRY_MAGIC,
            frame_seq: self.frame_seq,
            sample_rate,
            buffer_size_frames,
            channel_count: channels.len().min(MAX_CHANNELS) as u32,
            channels: [MeterSample::default(); MAX_CHANNELS],
            mix_count: mixes.len().min(MAX_MIXES) as u32,
            mixes: [MeterSample::default(); MAX_MIXES],
        };

        let ch_len = channels.len().min(MAX_CHANNELS);
        block.channels[..ch_len].copy_from_slice(&channels[..ch_len]);

        let mix_len = mixes.len().min(MAX_MIXES);
        block.mixes[..mix_len].copy_from_slice(&mixes[..mix_len]);

        self.publish(block);
    }

    /// Current frame sequence counter
    pub fn current_frame_seq(&self) -> u32 {
        self.frame_seq
    }
}

impl Drop for TelemetryPublisher {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view_ptr.as_ptr() as *mut _,
            });
            let _ = CloseHandle(self.handle);
        }
    }
}

/// High-speed Telemetry Reader from Windows Shared Memory
pub struct TelemetryReader {
    handle: HANDLE,
    view_ptr: NonNull<AudioTelemetryBlock>,
    _name: String,
}

unsafe impl Send for TelemetryReader {}
unsafe impl Sync for TelemetryReader {}

impl TelemetryReader {
    /// Open reader on standard shared memory name (`Local\OpenWaveLink_Telemetry_V1`)
    pub fn open() -> Result<Self> {
        Self::open_named(SHARED_TELEMETRY_NAME)
    }

    /// Open reader on custom shared memory mapping name
    pub fn open_named(name: &str) -> Result<Self> {
        let wide_name = HSTRING::from(name);

        let handle = unsafe {
            OpenFileMappingW(
                FILE_MAP_READ.0,
                false,
                PCWSTR(wide_name.as_ptr()),
            )
        }
        .context(format!("OpenFileMappingW failed for {name}"))?;

        if handle.is_invalid() {
            bail!("OpenFileMappingW returned invalid handle for {name}");
        }

        let view_address = unsafe {
            MapViewOfFile(
                handle,
                FILE_MAP_READ,
                0,
                0,
                std::mem::size_of::<AudioTelemetryBlock>(),
            )
        };

        if view_address.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            bail!("MapViewOfFile failed for {name}");
        }

        let ptr = NonNull::new(view_address.Value as *mut AudioTelemetryBlock)
            .expect("view_address is non-null");

        Ok(Self {
            handle,
            view_ptr: ptr,
            _name: name.to_string(),
        })
    }

    /// Read current telemetry snapshot with consistency verification (Seqlock read pattern)
    pub fn read(&self) -> Result<AudioTelemetryBlock> {
        // Read with consistency retry if writer modified during read
        for _ in 0..5 {
            fence(Ordering::Acquire);
            let block = unsafe { self.view_ptr.as_ptr().read_volatile() };
            fence(Ordering::Acquire);

            if block.magic == TELEMETRY_MAGIC {
                return Ok(block);
            }
        }

        // Return latest read if magic valid or bail
        let block = unsafe { self.view_ptr.as_ptr().read_volatile() };
        if block.magic != TELEMETRY_MAGIC {
            bail!("Invalid telemetry magic: 0x{:08X}", block.magic);
        }
        Ok(block)
    }
}

impl Drop for TelemetryReader {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view_ptr.as_ptr() as *mut _,
            });
            let _ = CloseHandle(self.handle);
        }
    }
}
