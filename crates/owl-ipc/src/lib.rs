//! OpenWaveLink IPC - Shared Memory Data Structures & Telemetry
//!
//! Windows 11 Real-Time Audio Telemetry and Matrix Configuration Layouts.
//! All memory structures are #[repr(C)] aligned for zero-copy memory mapping.

pub type ChannelId = u32;
pub type MixId = u32;
pub type ProcessId = u32;

/// System identifier for shared memory telemetry mapping
pub const SHARED_TELEMETRY_NAME: &str = "Local\\OpenWaveLink_Telemetry_V1";
pub const TELEMETRY_MAGIC: u32 = 0x57415645; // "WAVE" in ASCII Little-Endian
pub const MAX_CHANNELS: usize = 32;
pub const MAX_MIXES: usize = 8;

pub const SHARED_STREAM_PIPE_NAME: &str = "Local\\OpenWaveLink_StreamMix_PCM";
pub const SHARED_MICFX_PIPE_NAME: &str = "Local\\OpenWaveLink_MicFX_PCM";
pub const STREAM_PIPE_MAGIC: u32 = 0x50434D31; // "PCM1"
pub const STREAM_PIPE_DEFAULT_CAPACITY_FRAMES: usize = 16384; // ~341 ms @ 48 kHz

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StreamPipeHeader {
    pub magic: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub capacity_frames: u32,
    pub write_pos: u64,
    pub read_pos: u64,
    pub _pad: [u8; 32],
}

impl Default for StreamPipeHeader {
    fn default() -> Self {
        Self {
            magic: STREAM_PIPE_MAGIC,
            sample_rate: 48000,
            channels: 2,
            capacity_frames: STREAM_PIPE_DEFAULT_CAPACITY_FRAMES as u32,
            write_pos: 0,
            read_pos: 0,
            _pad: [0u8; 32],
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelType {
    #[default]
    HardwareInput = 0,    // Физический USB/XLR микрофон
    ProcessLoopback = 1,  // Захват приложения по PID
    VirtualCable = 2,     // Вход с WaveRT драйвера
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputEndpointType {
    #[default]
    HardwareDevice = 0,   // Наушники / Колонки (WASAPI Shared/Exclusive)
    VirtualWaveRT = 1,    // Выходная виртуальная линия для OBS Studio / Discord
}

/// Входной канал (Channel Config)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ChannelConfig {
    pub id: ChannelId,
    pub name: [u8; 64],          // UTF-8 имя канала с нуль-терминатором
    pub channel_type: ChannelType,
    pub target_pid: ProcessId,   // PID процесса для Process Loopback
    pub is_muted: bool,
    pub _pad: [u8; 3],           // 4-byte alignment
    pub master_gain: f32,        // Линейный коэффициент пре-фейдера [0.0..2.0]
    pub color_rgb: u32,          // 0x00RRGGBB
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            id: 0,
            name: [0u8; 64],
            channel_type: ChannelType::default(),
            target_pid: 0,
            is_muted: false,
            _pad: [0u8; 3],
            master_gain: 1.0,
            color_rgb: 0x007ACC,
        }
    }
}

/// Выходной микс (Mix Config)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MixConfig {
    pub id: MixId,
    pub name: [u8; 64],
    pub is_muted: bool,
    pub _pad: [u8; 3],
    pub master_gain: f32,        // Мастер-громкость шины [0.0..1.0]
    pub endpoint_type: OutputEndpointType,
    pub endpoint_device_id: [u8; 128], // WASAPI Endpoint GUID string
}

impl Default for MixConfig {
    fn default() -> Self {
        Self {
            id: 0,
            name: [0u8; 64],
            is_muted: false,
            _pad: [0u8; 3],
            master_gain: 1.0,
            endpoint_type: OutputEndpointType::default(),
            endpoint_device_id: [0u8; 128],
        }
    }
}

/// Элемент матрицы маршрутизации (Send Node)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatrixSendNode {
    pub gain: f32,               // Коэффициент посыла канала в данный микс [0.0..1.0]
    pub pan: f32,                // Панорама [-1.0 (Left) .. 0.0 (Center) .. +1.0 (Right)]
    pub is_muted: bool,          // Mute в конкретном миксе
    pub _pad: [u8; 3],
}

impl Default for MatrixSendNode {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            is_muted: false,
            _pad: [0u8; 3],
        }
    }
}

/// Показания VU-метра стереопары
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeterSample {
    pub peak_l: u32,    // f32::to_bits(linear_amplitude) [0.0 .. 1.0]
    pub peak_r: u32,
    pub rms_l: u32,
    pub rms_r: u32,
    pub clip_flag: u32, // 1 если сработал Clipguard/Limit, иначе 0
}

impl MeterSample {
    pub fn new(peak_l: f32, peak_r: f32, rms_l: f32, rms_r: f32, clip: bool) -> Self {
        Self {
            peak_l: peak_l.clamp(0.0, 1.0).to_bits(),
            peak_r: peak_r.clamp(0.0, 1.0).to_bits(),
            rms_l: rms_l.clamp(0.0, 1.0).to_bits(),
            rms_r: rms_r.clamp(0.0, 1.0).to_bits(),
            clip_flag: if clip { 1 } else { 0 },
        }
    }

    #[inline]
    pub fn peak_l_f32(&self) -> f32 {
        f32::from_bits(self.peak_l)
    }

    #[inline]
    pub fn peak_r_f32(&self) -> f32 {
        f32::from_bits(self.peak_r)
    }

    #[inline]
    pub fn rms_l_f32(&self) -> f32 {
        f32::from_bits(self.rms_l)
    }

    #[inline]
    pub fn rms_r_f32(&self) -> f32 {
        f32::from_bits(self.rms_r)
    }

    #[inline]
    pub fn is_clipping(&self) -> bool {
        self.clip_flag != 0
    }
}

/// Блок телеметрии в Shared Memory (4 КБ)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioTelemetryBlock {
    pub magic: u32,               // 0x57415645 ("WAVE")
    pub frame_seq: u32,           // Циклический счётчик кадров (инкремент на каждом обновлении)
    pub sample_rate: u32,         // 48000
    pub buffer_size_frames: u32,  // 128 / 256 / 480
    
    pub channel_count: u32,
    pub channels: [MeterSample; MAX_CHANNELS],
    
    pub mix_count: u32,
    pub mixes: [MeterSample; MAX_MIXES],
}

impl AudioTelemetryBlock {
    #[inline]
    pub fn sequence_counter(&self) -> u32 {
        self.frame_seq
    }
}

impl Default for AudioTelemetryBlock {
    fn default() -> Self {
        Self {
            magic: TELEMETRY_MAGIC,
            frame_seq: 0,
            sample_rate: 48000,
            buffer_size_frames: 480, // ~10 ms @ 48 kHz
            channel_count: 0,
            channels: [MeterSample::default(); MAX_CHANNELS],
            mix_count: 0,
            mixes: [MeterSample::default(); MAX_MIXES],
        }
    }
}
