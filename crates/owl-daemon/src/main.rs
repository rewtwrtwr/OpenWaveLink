//! OpenWaveLink CLI Daemon & 32-bit Float Routing Matrix Monitor
//!
//! Windows 11 WASAPI Process-Loopback, Hardware Mic Capture, OpenClipguard 2.0,
//! Routing Matrix summing, and real-time WASAPI headphone playback demonstration.
//!
//! # Architecture
//!
//! * The real-time thread owns the [`AudioEngine`] (matrix + dynamic capture slots + playback
//!   producer) and is the only place audio is processed.
//! * The [`capture_pool::CaptureSupervisor`] thread owns every live `ProcessAudioCapture`,
//!   `MicrophoneCapture` and `WasapiPlayback` value. Those types stop and **join their worker
//!   thread** on `Drop`, so they must outlive the stream they represent — the supervisor is
//!   what guarantees that.
//! * The JSON-RPC control plane never touches the matrix: it pushes `EngineEvent`s into a
//!   lock-free `rtrb` queue that the real-time thread drains.

pub mod capture_pool;
pub mod server;

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use anyhow::{Context, Result};
use owl_core::engine::AudioEngine;
use owl_core::{
    enumerate_audio_sessions, AudioProcessInfo, MixBusId, StreamPipePublisher, TelemetryPublisher,
    NUM_MIX_BUSES,
};
use owl_ipc::{MAX_CHANNELS, MAX_MIXES};
use windows::core::w;
use windows::Win32::System::Console::{
    GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, GetCurrentThread,
    SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};

use capture_pool::{CaptureKind, CaptureSupervisor, ChannelSpec, DEFAULT_SAMPLE_RATE};

/// Capacity of the lock-free engine control queue.
const ENGINE_QUEUE_CAPACITY: usize = 256;
/// Capacity of the matrix parameter queue (gain / mute / sends / VST3).
const MATRIX_QUEUE_CAPACITY: usize = 256;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let show_console = args.iter().any(|a| a == "--console" || a == "--debug" || a == "-v");

    #[cfg(windows)]
    if !show_console {
        unsafe {
            use windows::Win32::System::Console::GetConsoleWindow;
            use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
            let hwnd = GetConsoleWindow();
            if !hwnd.is_invalid() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    enable_virtual_terminal_processing();
    let mut target_pid: Option<u32> = None;
    let mut auto_select = true;
    let mut list_only = false;
    let mut no_mic = false;
    let mut no_playback = false;
    let mut output_endpoint: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pid" | "-p" => {
                if i + 1 < args.len() {
                    target_pid = args[i + 1].parse().ok();
                    i += 1;
                }
            }
            "--auto" | "-a" => {
                auto_select = true;
            }
            "--list" | "-l" => {
                list_only = true;
            }
            "--no-mic" => {
                no_mic = true;
            }
            "--no-playback" => {
                no_playback = true;
            }
            "--output" | "-o" => {
                if i + 1 < args.len() {
                    output_endpoint = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => {
                if let Ok(pid) = other.parse::<u32>() {
                    target_pid = Some(pid);
                }
            }
        }
        i += 1;
    }

    // 1. Enumerate Windows 11 Audio Sessions
    let sessions = enumerate_audio_sessions()
        .context("Failed to enumerate Windows audio sessions")?;

    print_sessions_table(&sessions);

    if list_only {
        return Ok(());
    }

    // 2. Select target PID
    let pid = match target_pid {
        Some(p) => p,
        None => {
            if auto_select || !atty_check() {
                sessions
                    .iter()
                    .find(|s| s.is_active && s.pid != 0)
                    .or_else(|| sessions.iter().find(|s| s.pid != 0))
                    .map(|s| s.pid)
                    .unwrap_or(std::process::id())
            } else {
                prompt_for_pid(&sessions)?
            }
        }
    };

    let target_name = sessions
        .iter()
        .find(|s| s.pid == pid)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("PID {pid}"));

    // 3. Build the audio engine and its lock-free control queues
    let mut engine = AudioEngine::new(DEFAULT_SAMPLE_RATE);
    let engine_tx = engine.attach_engine_queue(ENGINE_QUEUE_CAPACITY);
    let cmd_producer = engine.attach_command_queue(MATRIX_QUEUE_CAPACITY);

    // Console VU meters read the bus meters directly. Grabbed before the engine is moved into
    // the real-time thread.
    let stream_meter = Arc::clone(&engine.matrix().bus(MixBusId::Stream).meter);
    let monitor_meter = Arc::clone(&engine.matrix().bus(MixBusId::Monitor).meter);

    // 4. Start the capture supervisor: it owns every capture and playback handle for life.
    let supervisor = CaptureSupervisor::spawn(engine_tx)
        .context("Failed to spawn the capture supervisor thread")?;

    // 5. Register the initial channels through the supervisor.
    // Channel 1 is ALWAYS the pinned, non-deletable "System" audio channel (PID 0 / default loopback).
    let sys_channel_id = 1u32;
    let mut initial_channels: Vec<(u32, String, &'static str, u32, f32)> = Vec::new();

    println!("\n\x1B[1;32m[+] Attaching System Audio Channel:\x1B[0m (PID: 0, Windows Audio Sink)");

    match supervisor
        .add_channel(ChannelSpec {
            channel_id: sys_channel_id,
            name: "System".to_string(),
            kind: CaptureKind::Process { pid: 0 },
        })
        .await
    {
        Ok(ready) => {
            println!(
                "\x1B[1;32m[+] System Audio Stream:\x1B[0m channel {} active at {} Hz",
                ready.channel_id, ready.sample_rate
            );
            initial_channels.push((sys_channel_id, "System".to_string(), "SoftwareProcess", 0, 1.0));
        }
        Err(err) => {
            println!(
                "\x1B[1;33m[!] System Audio stream notice:\x1B[0m {} (registered as default sink)",
                err
            );
            initial_channels.push((sys_channel_id, "System".to_string(), "SoftwareProcess", 0, 1.0));
        }
    }

    let mut next_channel_id = 2u32;
    let mic_channel_id = next_channel_id;
    next_channel_id += 1;

    // 6. Initialize Hardware Microphone Capture
    if !no_mic {
        match supervisor
            .add_channel(ChannelSpec {
                channel_id: mic_channel_id,
                name: "Hardware Mic".to_string(),
                kind: CaptureKind::Microphone,
            })
            .await
        {
            Ok(_) => {
                println!("\x1B[1;32m[+] Hardware Microphone:\x1B[0m Active (eCapture default endpoint)");
                initial_channels.push((mic_channel_id, "Hardware Mic".to_string(), "Microphone", 0, 1.0));
            }
            Err(err) => {
                println!(
                    "\x1B[1;33m[!] Hardware Microphone unavailable:\x1B[0m {} (continuing without mic)",
                    err
                );
            }
        }
    } else {
        println!("\x1B[1;33m[*] Microphone disabled by --no-mic\x1B[0m");
    }

    // 7. Bind the Monitor Mix output device BEFORE the control plane starts, so
    //    `owl.getEngineState` can report the endpoint that is actually bound.
    let mut bound_output_id = String::new();
    let mut bound_output_name = String::new();

    if !no_playback {
        match supervisor.set_output_device(output_endpoint.clone()).await {
            Ok(device) => {
                println!(
                    "\x1B[1;32m[+] Monitor Output (Playback):\x1B[0m {}",
                    device.name
                );
                if owl_core::devices::is_virtual_endpoint_name(&device.name) {
                    println!(
                        "\x1B[1;33m[!] WARNING: '{}' looks like a VIRTUAL audio cable.\x1B[0m\n    \
                         The monitor mix will not be audible on physical speakers/headphones.\n    \
                         Select a real output device (owl.setOutputDevice), e.g. your headphones.",
                        device.name
                    );
                }
                bound_output_id = device.id;
                bound_output_name = device.name;
            }
            Err(err) => {
                println!(
                    "\x1B[1;33m[!] WASAPI Playback unavailable:\x1B[0m {} (continuing in monitor-meter only mode)",
                    err
                );
            }
        }
    } else {
        println!("\x1B[1;33m[*] Playback output disabled by --no-playback\x1B[0m");
    }

    // 8. Initialize Windows Shared Memory Telemetry Publisher
    let telemetry_publisher = match TelemetryPublisher::new() {
        Ok(publ) => {
            println!("\x1B[1;32m[+] Windows Shared Memory Telemetry:\x1B[0m Active (Local\\OpenWaveLink_Telemetry_V1 @ 60-144 FPS)");
            Some(publ)
        }
        Err(err) => {
            println!("\x1B[1;33m[!] Telemetry mapping notice:\x1B[0m {} (continuing)", err);
            None
        }
    };

    // 9. Initialize OBS Studio Direct Shared Memory PCM Pipe
    let stream_pipe = match StreamPipePublisher::new() {
        Ok(pipe) => {
            println!("\x1B[1;32m[+] OBS Stream Mix Shared Memory Pipe:\x1B[0m Active (Local\\OpenWaveLink_StreamMix_PCM < 0.1 ms)");
            Some(pipe)
        }
        Err(err) => {
            println!("\x1B[1;33m[!] OBS Stream Pipe notice:\x1B[0m {} (continuing)", err);
            None
        }
    };

    let mic_fx_pipe = match StreamPipePublisher::new_mic_fx() {
        Ok(pipe) => {
            println!("\x1B[1;32m[+] MicFX Shared Memory Pipe:\x1B[0m Active (Local\\OpenWaveLink_MicFX_PCM < 0.1 ms)");
            Some(pipe)
        }
        Err(err) => {
            println!("\x1B[1;33m[!] MicFX Pipe notice:\x1B[0m {} (continuing)", err);
            None
        }
    };

    // 10. Start Localhost JSON-RPC 2.0 WebSocket Control Server
    let (mut server_state, broadcast_tx) = server::ServerState::new(cmd_producer);
    server_state.set_supervisor(Arc::clone(&supervisor));

    // Virtual cables (USB/IP). A missing helper is reported once here and the
    // cable RPCs then explain the reason instead of failing silently.
    match owl_core::CableManager::new() {
        Ok(manager) => {
            println!(
                "\x1B[1;32m[+] Virtual cables:\x1B[0m usbip={} server={}",
                manager.usbip_binary().display(),
                manager.server_binary().display()
            );
            server_state.set_cable_manager(Arc::new(manager));
        }
        Err(err) => {
            println!(
                "\x1B[1;33m[!] Virtual cables unavailable:\x1B[0m {} (continuing)",
                err
            );
        }
    }

    server_state.output_device_id = bound_output_id;
    server_state.output_device_name = bound_output_name;
    for (id, name, kind, ch_pid, gain) in &initial_channels {
        server_state.add_channel(*id, name.clone(), kind, *ch_pid, *gain, false);
    }

    let ws_server = match server::WsServer::bind(server::DEFAULT_WS_PORT, server_state, broadcast_tx).await {
        Ok(srv) => {
            println!("\x1B[1;32m[+] JSON-RPC 2.0 WebSocket Server:\x1B[0m ws://{}", srv.addr);
            println!("    - Config info written to: {}", srv.info_path.display());
            Some(srv)
        }
        Err(err) => {
            println!("\x1B[1;33m[!] WebSocket Server failed to bind:\x1B[0m {}", err);
            None
        }
    };

    let ws_task = ws_server.map(|srv| {
        tokio::spawn(async move {
            if let Err(e) = srv.run().await {
                log::error!("WebSocket server error: {:?}", e);
            }
        })
    });

    println!(
        "\x1B[1;32m[+] Routing Matrix Graph:\x1B[0m {} Channels x {} Buses (dynamic)",
        initial_channels.len(),
        NUM_MIX_BUSES
    );
    println!("    - \x1B[1;37mStream Mix:\x1B[0m  Apps 30% (-10.5 dB), Mic 100% [OpenClipguard 2.0 active]");
    println!("    - \x1B[1;37mMonitor Mix:\x1B[0m Apps 100% (0 dB Unity), Mic Muted (Headphone feed)");
    println!("\x1B[1;33m[!] Real-Time Audio Matrix running. Press Ctrl+C to stop.\x1B[0m\n");

    // 11. Spawn Real-Time Matrix Mixer Thread
    let is_running = Arc::new(AtomicBool::new(true));
    let is_running_worker = Arc::clone(&is_running);

    let mixer_thread = std::thread::Builder::new()
        .name("owl-matrix-mixer".to_string())
        .spawn(move || {
            run_matrix_summing_loop(
                engine,
                telemetry_publisher,
                stream_pipe,
                mic_fx_pipe,
                is_running_worker,
            );
        })
        .context("Failed to spawn matrix audio worker thread")?;

    // 12. Console Animated VU-Meters for both Stream and Monitor Mixes
    let mut interval = tokio::time::interval(Duration::from_millis(33)); // ~30 FPS

    let mut stream_peak_hold_l = 0.0f32;
    let mut stream_peak_hold_r = 0.0f32;
    let mut monitor_peak_hold_l = 0.0f32;
    let mut monitor_peak_hold_r = 0.0f32;
    let mut hold_decay = 0;

    print!("\x1B[?25l"); // Hide cursor
    let _ = io::stdout().flush();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\n\n\x1B[1;33m[!] Stopping audio matrix and daemon...\x1B[0m");
                break;
            }
            _ = interval.tick() => {
                let stream_read = stream_meter.read();
                let monitor_read = monitor_meter.read();

                if stream_read.peak_l > stream_peak_hold_l { stream_peak_hold_l = stream_read.peak_l; }
                if stream_read.peak_r > stream_peak_hold_r { stream_peak_hold_r = stream_read.peak_r; }
                if monitor_read.peak_l > monitor_peak_hold_l { monitor_peak_hold_l = monitor_read.peak_l; }
                if monitor_read.peak_r > monitor_peak_hold_r { monitor_peak_hold_r = monitor_read.peak_r; }

                hold_decay += 1;
                if hold_decay >= 15 {
                    hold_decay = 0;
                    stream_peak_hold_l = (stream_peak_hold_l * 0.92).max(0.0);
                    stream_peak_hold_r = (stream_peak_hold_r * 0.92).max(0.0);
                    monitor_peak_hold_l = (monitor_peak_hold_l * 0.92).max(0.0);
                    monitor_peak_hold_r = (monitor_peak_hold_r * 0.92).max(0.0);
                }

                render_dual_matrix_vu_meter(
                    &stream_read,
                    stream_peak_hold_l,
                    stream_peak_hold_r,
                    &monitor_read,
                    monitor_peak_hold_l,
                    monitor_peak_hold_r,
                );
            }
        }
    }

    print!("\x1B[?25h"); // Restore cursor
    let _ = io::stdout().flush();

    // Graceful shutdown, in dependency order: stop producing audio, then stop the control
    // plane, then let the supervisor stop every capture and the playback device on the thread
    // that created them.
    is_running.store(false, Ordering::Release);
    let _ = mixer_thread.join();

    if let Some(t) = ws_task {
        t.abort();
    }

    supervisor.shutdown();

    println!("\x1B[1;32m[✓] OpenWaveLink Matrix Engine stopped cleanly.\x1B[0m");
    Ok(())
}

/// Hard Real-Time Matrix Summing Loop
///
/// The engine owns the matrix, the capture rings and the playback producer. This loop only
/// drives it and republishes its results — it never allocates and never locks.
fn run_matrix_summing_loop(
    mut engine: AudioEngine,
    mut telemetry_publisher: Option<TelemetryPublisher>,
    mut stream_pipe: Option<StreamPipePublisher>,
    mut mic_fx_pipe: Option<StreamPipePublisher>,
    is_running: Arc<AtomicBool>,
) {
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

    let mut channel_meters = [owl_ipc::MeterSample::default(); MAX_CHANNELS];
    let mut mix_meters = [owl_ipc::MeterSample::default(); MAX_MIXES];

    while is_running.load(Ordering::Acquire) {
        // Drain control-plane parameter updates (gain / mute / sends / VST3). Zero-allocation,
        // and idempotent if `process_block` drains the queue itself.
        engine.matrix_mut().process_commands();

        // Drain engine events (attach/detach capture, add/remove channel, swap output), pull
        // every attached capture ring, run the matrix, and push the Monitor mix to playback.
        let frames = engine.process_block();
        if frames == 0 {
            // Nothing captured this cycle; sleep briefly (~1 ms) to prevent high CPU spin.
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }

        // Forward Stream Mix samples directly to OBS Studio Shared Memory Pipe (< 0.1 ms)
        if let Some(ref mut pipe) = stream_pipe {
            pipe.write_interleaved(engine.stream_mix());
        }

        // Forward MicFX samples to its Shared Memory Pipe
        if let Some(ref mut pipe) = mic_fx_pipe {
            pipe.write_interleaved(engine.mic_fx());
        }

        // Publish VU-meter telemetry to Windows Shared Memory (@ 60-144 FPS)
        if let Some(ref mut publ) = telemetry_publisher {
            engine.write_telemetry_samples(&mut channel_meters, &mut mix_meters);
            let ch_count = engine.matrix().channel_count().min(MAX_CHANNELS);
            let mix_count = NUM_MIX_BUSES.min(MAX_MIXES);
            publ.update(
                engine.matrix().sample_rate(),
                frames as u32,
                &channel_meters[..ch_count],
                &mix_meters[..mix_count],
            );
        }
    }

    if let Some(h) = mmcss_handle {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(h);
        }
    }
}

/// Render 4-channel animated ASCII VU Meter for both Stream Mix and Monitor Mix
fn render_dual_matrix_vu_meter(
    stream: &owl_core::MeterReading,
    stream_hold_l: f32,
    stream_hold_r: f32,
    monitor: &owl_core::MeterReading,
    monitor_hold_l: f32,
    monitor_hold_r: f32,
) {
    const WIDTH: usize = 30;

    let s_bar_l = make_meter_bar(stream.peak_l, stream.rms_l, stream_hold_l, WIDTH);
    let s_bar_r = make_meter_bar(stream.peak_r, stream.rms_r, stream_hold_r, WIDTH);
    let m_bar_l = make_meter_bar(monitor.peak_l, monitor.rms_l, monitor_hold_l, WIDTH);
    let m_bar_r = make_meter_bar(monitor.peak_r, monitor.rms_r, monitor_hold_r, WIDTH);

    let s_clip = if stream.is_clipping { "\x1B[1;41;37m CLIP! \x1B[0m" } else { "       " };
    let m_clip = if monitor.is_clipping { "\x1B[1;41;37m CLIP! \x1B[0m" } else { "       " };

    print!(
        "\r\x1B[2K\x1B[1;35m[STREAM MIX] \x1B[1;36mL:\x1B[0m [{}] {:>5.1} dB (RMS: {:>5.1} dB) {}\n\
         \r\x1B[2K\x1B[1;35m             \x1B[1;36mR:\x1B[0m [{}] {:>5.1} dB (RMS: {:>5.1} dB)\n\
         \r\x1B[2K\x1B[1;34m[MONITOR MIX]\x1B[1;36mL:\x1B[0m [{}] {:>5.1} dB (RMS: {:>5.1} dB) {}\n\
         \r\x1B[2K\x1B[1;34m             \x1B[1;36mR:\x1B[0m [{}] {:>5.1} dB (RMS: {:>5.1} dB)\x1B[3A",
        s_bar_l, stream.peak_l_db(), stream.rms_l_db(), s_clip,
        s_bar_r, stream.peak_r_db(), stream.rms_r_db(),
        m_bar_l, monitor.peak_l_db(), monitor.rms_l_db(), m_clip,
        m_bar_r, monitor.peak_r_db(), monitor.rms_r_db(),
    );
    let _ = io::stdout().flush();
}

fn make_meter_bar(peak: f32, rms: f32, peak_hold: f32, width: usize) -> String {
    let peak_pos = ((peak.clamp(0.0, 1.0) * (width as f32)).round() as usize).min(width);
    let rms_pos = ((rms.clamp(0.0, 1.0) * (width as f32)).round() as usize).min(width);
    let hold_pos = ((peak_hold.clamp(0.0, 1.0) * (width as f32)).round() as usize).min(width);

    let mut out = String::with_capacity(width * 8);

    for i in 1..=width {
        let is_yellow = i > (width * 65) / 100 && i <= (width * 88) / 100;
        let is_red = i > (width * 88) / 100;

        let color_code = if is_red {
            "\x1B[1;31m"
        } else if is_yellow {
            "\x1B[1;33m"
        } else {
            "\x1B[1;32m"
        };

        if i == hold_pos && hold_pos > peak_pos {
            out.push_str("\x1B[1;37m|\x1B[0m");
        } else if i <= rms_pos {
            out.push_str(color_code);
            out.push('█');
            out.push_str("\x1B[0m");
        } else if i <= peak_pos {
            out.push_str(color_code);
            out.push('▓');
            out.push_str("\x1B[0m");
        } else {
            out.push_str("\x1B[1;30m-\x1B[0m");
        }
    }

    out
}

fn print_help() {
    println!("OpenWaveLink Daemon - Windows 11 Audio Process Loopback & Matrix Engine");
    println!("Usage:");
    println!("  owl-daemon                  Interactive mode (lists sessions & asks for PID)");
    println!("  owl-daemon <PID>            Directly capture specified PID and mix with Mic");
    println!("  owl-daemon --pid <PID>      Directly capture specified PID and mix with Mic");
    println!("  owl-daemon --auto           Automatically select first active audio session");
    println!("  owl-daemon --list           List audio sessions and exit");
    println!("  owl-daemon --no-mic         Run matrix without microphone input");
    println!("  owl-daemon --no-playback    Run matrix without headphone output");
    println!("  owl-daemon --output <ID>    Bind Monitor Mix to a specific render endpoint id");
    println!("  owl-daemon --help           Show this help message");
}

fn print_sessions_table(sessions: &[AudioProcessInfo]) {
    println!("\x1B[1;37mActive Windows Audio Sessions:\x1B[0m");
    println!("+-------+----------------------------------+-----------+");
    println!("|  PID  | Process Executable               | Status    |");
    println!("+-------+----------------------------------+-----------+");

    if sessions.is_empty() {
        println!("|       | No active audio sessions found   |           |");
    } else {
        for s in sessions {
            let status = if s.is_active {
                "\x1B[1;32mActive \x1B[0m"
            } else {
                "\x1B[1;30mIdle   \x1B[0m"
            };
            println!("| {:>5} | {:<32} | {} |", s.pid, truncate_str(&s.name, 32), status);
        }
    }
    println!("+-------+----------------------------------+-----------+");
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len - 3])
    } else {
        s.to_string()
    }
}

fn prompt_for_pid(sessions: &[AudioProcessInfo]) -> Result<u32> {
    let default_pid = sessions
        .iter()
        .find(|s| s.is_active && s.pid != 0)
        .or_else(|| sessions.iter().find(|s| s.pid != 0))
        .map(|s| s.pid);

    match default_pid {
        Some(def) => print!("\x1B[1;33mEnter PID to capture [Default: {}]: \x1B[0m", def),
        None => print!("\x1B[1;33mEnter PID to capture: \x1B[0m"),
    }
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return default_pid.ok_or_else(|| anyhow::anyhow!("No process selected"));
    }

    trimmed
        .parse::<u32>()
        .context(format!("Invalid process PID: '{trimmed}'"))
}

fn atty_check() -> bool {
    true
}

fn enable_virtual_terminal_processing() {
    unsafe {
        if let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let mut mode = CONSOLE_MODE(0);
            if GetConsoleMode(handle, &mut mode).is_ok() {
                let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

