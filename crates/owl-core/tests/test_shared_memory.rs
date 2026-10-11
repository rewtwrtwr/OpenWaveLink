//! High-Speed Shared Memory & IPC Integration Tests for OpenWaveLink
//!
//! Verifies:
//! 1. Windows Shared Memory Telemetry Publisher & Reader (`Local\OpenWaveLink_Telemetry_V1`)
//!    - Win32 API file mapping, magic number (0x57415645 "WAVE"), frame sequence integrity
//! 2. OBS Studio Direct PCM Stream Pipe (`Local\OpenWaveLink_StreamMix_PCM`)
//!    - Lock-Free SPSC Shared Memory RingBuffer, 48 kHz stereo Float32 sample streaming
//! 3. Real-Time Lock-Free `MatrixCommand` Queue Execution

use owl_core::{
    InputChannelType, MatrixCommand, MatrixInputChannel, MixBusId, RoutingMatrix,
    StreamPipePublisher, StreamPipeReader, TelemetryPublisher, TelemetryReader,
};
use owl_ipc::{AudioTelemetryBlock, MeterSample, TELEMETRY_MAGIC};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Memory::{
    OpenFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_READ,
};

#[test]
fn test_telemetry_shared_memory_publisher_and_reader() {
    let mapping_name = "Local\\OpenWaveLink_Test_Telemetry_Unique";

    // 1. Initialize Publisher
    let mut publisher = TelemetryPublisher::with_name(mapping_name)
        .expect("Failed to create TelemetryPublisher in shared memory");

    // 2. Initialize Reader
    let reader = TelemetryReader::open_named(mapping_name)
        .expect("Failed to open TelemetryReader on shared memory");

    // Verify initial state
    let initial_block = reader.read().expect("read initial block");
    assert_eq!(
        initial_block.magic, TELEMETRY_MAGIC,
        "Magic must be 0x57415645 ('WAVE')"
    );
    assert_eq!(initial_block.frame_seq, 0);

    // 3. Publish test telemetry frames with realistic levels
    for seq in 1..=100 {
        let test_channels = [
            MeterSample::new(0.75, 0.70, 0.50, 0.45, false),
            MeterSample::new(0.99, 1.00, 0.80, 0.82, true), // clipping mic
        ];

        let test_mixes = [
            MeterSample::new(0.60, 0.60, 0.40, 0.40, false), // Stream Mix
            MeterSample::new(0.80, 0.80, 0.55, 0.55, false), // Monitor Mix
            MeterSample::new(0.00, 0.00, 0.00, 0.00, false), // VOD Track
            MeterSample::new(0.30, 0.30, 0.20, 0.20, false), // Discord
            MeterSample::new(0.00, 0.00, 0.00, 0.00, false), // Aux
        ];

        publisher.update(48000, 256, &test_channels, &test_mixes);

        let snapshot = reader.read().expect("read telemetry snapshot");
        assert_eq!(snapshot.magic, TELEMETRY_MAGIC);
        assert_eq!(snapshot.frame_seq, seq);
        assert_eq!(snapshot.sample_rate, 48000);
        assert_eq!(snapshot.buffer_size_frames, 256);
        assert_eq!(snapshot.channel_count, 2);
        assert_eq!(snapshot.mix_count, 5);

        // Verify channel 0 (Music)
        assert!((snapshot.channels[0].peak_l_f32() - 0.75).abs() < 1e-4);
        assert!((snapshot.channels[0].peak_r_f32() - 0.70).abs() < 1e-4);
        assert!(!snapshot.channels[0].is_clipping());

        // Verify channel 1 (Mic clipping)
        assert!((snapshot.channels[1].peak_r_f32() - 1.00).abs() < 1e-4);
        assert!(snapshot.channels[1].is_clipping());

        // Verify mix 0 (Stream Mix) and mix 1 (Monitor Mix)
        assert!((snapshot.mixes[0].peak_l_f32() - 0.60).abs() < 1e-4);
        assert!((snapshot.mixes[1].peak_l_f32() - 0.80).abs() < 1e-4);
    }
}

#[test]
fn test_telemetry_raw_win32_api_verification() {
    let mapping_name = "Local\\OpenWaveLink_Test_Win32_Raw";

    let mut publisher = TelemetryPublisher::with_name(mapping_name)
        .expect("Create publisher");

    let test_channels = [MeterSample::new(0.42, 0.42, 0.30, 0.30, false)];
    let test_mixes = [MeterSample::new(0.88, 0.88, 0.60, 0.60, false)];
    publisher.update(48000, 128, &test_channels, &test_mixes);

    // Direct, independent Win32 API read without TelemetryReader wrapper
    unsafe {
        let wide_name = HSTRING::from(mapping_name);
        let handle = OpenFileMappingW(
            FILE_MAP_READ.0,
            false,
            PCWSTR(wide_name.as_ptr()),
        )
        .expect("Win32 OpenFileMappingW failed");

        assert!(!handle.is_invalid());

        let view = MapViewOfFile(
            handle,
            FILE_MAP_READ,
            0,
            0,
            std::mem::size_of::<AudioTelemetryBlock>(),
        );

        assert!(!view.Value.is_null());

        let block_ptr = view.Value as *const AudioTelemetryBlock;
        let raw_block = block_ptr.read_volatile();

        assert_eq!(raw_block.magic, TELEMETRY_MAGIC);
        assert_eq!(raw_block.sample_rate, 48000);
        assert_eq!(raw_block.buffer_size_frames, 128);
        assert_eq!(raw_block.channel_count, 1);
        assert!((raw_block.channels[0].peak_l_f32() - 0.42).abs() < 1e-4);

        let _ = UnmapViewOfFile(view);
        let _ = CloseHandle(handle);
    }
}

#[test]
fn test_obs_direct_pcm_stream_pipe_spsc() {
    let pipe_name = "Local\\OpenWaveLink_Test_StreamMix_PCM_Pipe";
    const CAPACITY: usize = 1024; // 1024 frames = 2048 f32 samples

    let mut publisher = StreamPipePublisher::with_capacity(pipe_name, CAPACITY)
        .expect("Create StreamPipePublisher");
    let mut reader = StreamPipeReader::open_named(pipe_name)
        .expect("Open StreamPipeReader");

    assert_eq!(publisher.capacity(), CAPACITY);
    assert_eq!(reader.available_frames(), 0);

    // 1. Write small block (64 frames)
    let mut in_frames = vec![[0.0f32; 2]; 64];
    for (i, frame) in in_frames.iter_mut().enumerate() {
        let phase = i as f32 / 64.0;
        frame[0] = (phase * std::f32::consts::TAU).sin() * 0.5;
        frame[1] = (phase * std::f32::consts::TAU).cos() * 0.5;
    }

    publisher.write_stream_frames(&in_frames);
    assert_eq!(reader.available_frames(), 64);

    let mut out_frames = vec![[0.0f32; 2]; 64];
    let read_count = reader.read_stream_frames(&mut out_frames);
    assert_eq!(read_count, 64);
    assert_eq!(reader.available_frames(), 0);

    for i in 0..64 {
        assert_eq!(out_frames[i][0], in_frames[i][0]);
        assert_eq!(out_frames[i][1], in_frames[i][1]);
    }

    // 2. Stream several blocks in a continuous loop to test circular wraparound
    for cycle in 0..10 {
        let block_len = 128;
        let mut block = vec![[0.0f32; 2]; block_len];
        for (i, f) in block.iter_mut().enumerate() {
            f[0] = (cycle * 1000 + i) as f32;
            f[1] = -((cycle * 1000 + i) as f32);
        }

        publisher.write_stream_frames(&block);

        let mut read_buf = vec![[0.0f32; 2]; block_len];
        let read = reader.read_stream_frames(&mut read_buf);
        assert_eq!(read, block_len);

        for i in 0..block_len {
            assert_eq!(read_buf[i][0], block[i][0]);
            assert_eq!(read_buf[i][1], block[i][1]);
        }
    }
}

#[test]
fn test_lockfree_matrix_commands_realtime_application() {
    let sample_rate = 48000;
    let mut matrix = RoutingMatrix::new(sample_rate);

    let mut ch0 = MatrixInputChannel::new(
        1,
        "Game Audio",
        InputChannelType::SoftwareProcess { pid: 1234 },
        sample_rate,
    );
    ch0.channel_fader.reset(0.5);
    matrix.add_channel(ch0);

    let mut ch1 = MatrixInputChannel::new(
        2,
        "Microphone",
        InputChannelType::Microphone,
        sample_rate,
    );
    ch1.channel_fader.reset(1.0);
    matrix.add_channel(ch1);

    // Attach lock-free command queue
    let mut producer = matrix.attach_command_queue(64);

    // Enqueue set of commands
    producer
        .push(MatrixCommand::SetChannelGain {
            channel_idx: 0,
            slider: 0.80,
        })
        .expect("push");
    producer
        .push(MatrixCommand::SetChannelMute {
            channel_idx: 0,
            muted: true,
        })
        .expect("push");
    producer
        .push(MatrixCommand::SetSendGain {
            channel_idx: 0,
            mix_idx: 0, // Stream Mix
            slider: 0.65,
        })
        .expect("push");
    producer
        .push(MatrixCommand::SetSendMute {
            channel_idx: 0,
            mix_idx: 1, // Monitor Mix
            muted: true,
        })
        .expect("push");
    producer
        .push(MatrixCommand::SetMixGain {
            mix_idx: 0,
            slider: 0.90,
        })
        .expect("push");
    producer
        .push(MatrixCommand::SetMixMute {
            mix_idx: 2, // VOD
            muted: true,
        })
        .expect("push");
    producer
        .push(MatrixCommand::AssignProcess {
            channel_idx: 0,
            pid: 5678,
        })
        .expect("push");

    // Process single block (zero-allocation)
    let frames = 128;
    let mut in_proc_l = vec![0.5f32; frames];
    let mut in_proc_r = vec![0.5f32; frames];
    let mut in_mic_l = vec![0.0f32; frames];
    let mut in_mic_r = vec![0.0f32; frames];

    let mut out_stream = vec![0.0f32; frames * 2];
    let mut out_monitor = vec![0.0f32; frames * 2];
    let mut out_vod = vec![0.0f32; frames * 2];
    let mut out_discord = vec![0.0f32; frames * 2];
    let mut out_aux = vec![0.0f32; frames * 2];

    {
        let mut inputs = [
            (1, in_proc_l.as_mut_slice(), in_proc_r.as_mut_slice()),
            (2, in_mic_l.as_mut_slice(), in_mic_r.as_mut_slice()),
        ];
        let mut outputs = [
            out_stream.as_mut_slice(),
            out_monitor.as_mut_slice(),
            out_vod.as_mut_slice(),
            out_discord.as_mut_slice(),
            out_aux.as_mut_slice(),
        ];

        // This call automatically drains the queue and applies all commands
        matrix.process_interleaved(&mut inputs, &mut outputs);
    }

    // Verify all commands took effect
    let c0 = matrix.channel(1).expect("channel 1");
    assert_eq!(c0.is_channel_muted, true);
    assert_eq!(
        c0.channel_type,
        InputChannelType::SoftwareProcess { pid: 5678 }
    );
    assert!((c0.channel_fader.target() - owl_core::slider_to_gain(0.80)).abs() < 1e-4);
    assert!((c0.sends[0].gain_smoother.target() - owl_core::slider_to_gain(0.65)).abs() < 1e-4);
    assert_eq!(c0.sends[1].is_muted, true);

    let mix_stream = matrix.bus(MixBusId::Monitor);
    assert!((mix_stream.master_fader.target() - owl_core::slider_to_gain(0.90)).abs() < 1e-4);

    let mix_vod = matrix.bus(MixBusId::Vod);
    assert_eq!(mix_vod.is_muted, true);
}

