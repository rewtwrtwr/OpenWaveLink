//! End-to-end verification that audio really travels through a USB/IP virtual
//! cable.
//!
//! This test is the load-bearing proof for the whole virtual-cable feature: it
//! writes a known signal into the cable's *render* endpoint and reads it back
//! from the cable's *capture* endpoint. If samples come out the other side, the
//! cable is a working audio loopback; if they do not, no amount of RPC plumbing
//! would make the feature real.
//!
//! The test is `#[ignore]`d by default because it needs the USB/IP driver, an
//! installed `virtualcables.exe`, and elevation for the one-time attach. Run it
//! explicitly with:
//!
//! ```text
//! cargo test -p owl-core --test cable_audio -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use owl_core::cable::CableManager;
use owl_core::{MicrophoneCapture, WasapiPlayback};

/// Amplitude of the injected tone. Comfortably above the noise floor while
/// leaving headroom, so a passing test cannot be explained by dither.
const TONE_AMPLITUDE: f32 = 0.25;

/// Frequency of the injected tone.
const TONE_HZ: f32 = 440.0;

/// How long to feed audio before expecting to read something back.
const FEED_DURATION: Duration = Duration::from_millis(1500);

/// Frames written per push to the playback ring.
const CHUNK_FRAMES: usize = 480;

#[test]
#[ignore = "requires the usbip-win2 driver, virtualcables.exe, and elevation"]
fn test_audio_flows_through_virtual_cable() {
    const CABLE_NAME: &str = "OWL Loopback Test";

    let manager = CableManager::new().expect("cable manager");
    println!("server binary: {}", manager.server_binary().display());
    println!("usbip  binary: {}", manager.usbip_binary().display());

    // 1. Export exactly one cable under a known name.
    let cables = manager
        .set_cables(&[CABLE_NAME.to_string()])
        .expect("export cable");
    println!("exported cables: {cables:?}");

    // 2. Make sure the cable is attached. If it was attached before, the driver
    //    restores it automatically and this only re-stashes the record.
    if let Err(err) = manager.attach_all_persistent() {
        println!("attach_all_persistent reported: {err} (continuing: it may already be attached)");
    }

    // 3. Resolve both endpoints the cable exposes.
    let endpoints = manager
        .resolve_endpoints(CABLE_NAME, Duration::from_secs(20))
        .expect("cable endpoints must appear in Windows");
    println!("render endpoint: {}", endpoints.render);
    println!("capture endpoint: {}", endpoints.capture);

    // 4. Open playback into the cable's render side.
    let mut playback = WasapiPlayback::new_with_device(Some(&endpoints.render))
        .expect("open cable render endpoint");
    playback.start().expect("start playback");
    let mut producer = playback.take_producer().expect("playback producer");
    println!(
        "playback bound to {:?}",
        playback.device_name().unwrap_or("<unknown>")
    );

    // 5. Open capture from the cable's capture side.
    let mut capture =
        MicrophoneCapture::new_with_device(&endpoints.capture).expect("open cable capture endpoint");
    capture.start().expect("start capture");
    let mut consumer = capture.take_consumer().expect("capture consumer");
    println!(
        "capture bound to {:?}",
        capture.format.map(|f| f.sample_rate).unwrap_or(0)
    );

    // 6. Feed a tone into the cable while draining whatever comes back.
    let started = Instant::now();
    let mut phase = 0.0f32;
    let phase_step = std::f32::consts::TAU * TONE_HZ / 48_000.0;
    let mut peak_read = 0.0f32;
    let mut frames_read = 0usize;

    while started.elapsed() < FEED_DURATION {
        for _ in 0..CHUNK_FRAMES {
            let sample = TONE_AMPLITUDE * phase.sin();
            phase += phase_step;
            // Stereo: identical channels.
            let _ = producer.push(sample);
            let _ = producer.push(sample);
        }

        // Drain everything currently available from the capture side.
        while let Ok(sample) = consumer.pop() {
            let magnitude = sample.abs();
            if magnitude > peak_read {
                peak_read = magnitude;
            }
            frames_read += 1;
        }

        std::thread::sleep(Duration::from_millis(5));
    }

    // Final drain: the cable buffers audio, so the tail arrives late.
    let drain_deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < drain_deadline {
        while let Ok(sample) = consumer.pop() {
            let magnitude = sample.abs();
            if magnitude > peak_read {
                peak_read = magnitude;
            }
            frames_read += 1;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    capture.stop();
    playback.stop();

    println!("frames read back: {frames_read}");
    println!("peak amplitude read back: {peak_read}");

    // 7. The assertion that matters. A USB cable round-trip attenuates a little,
    //    so require a clear fraction of the injected amplitude rather than an
    //    exact match: silence or noise must fail.
    assert!(
        frames_read > 0,
        "no audio came back through the cable: the loopback is not working"
    );
    assert!(
        peak_read > TONE_AMPLITUDE * 0.2,
        "audio came back but far too quiet (peak {peak_read}, injected {TONE_AMPLITUDE}); \
         the cable transport is not carrying the signal"
    );
}
