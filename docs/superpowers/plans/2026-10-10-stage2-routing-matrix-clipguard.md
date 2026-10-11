# OpenWaveLink Stage 2 Implementation Plan: 32-bit Float Routing Matrix & OpenClipguard 2.0

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans or subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement 32-bit Float Routing Matrix ($N \times M$ buses), OpenClipguard 2.0 DSP limiter/saturator, zipper noise smoother, hardware microphone capture, and low-latency WASAPI playback in `crates/owl-core` and demonstrate in `crates/owl-daemon`.

**Architecture:** A lock-free, zero-allocation hard real-time audio pipeline running in MMCSS Pro Audio threads. Raw mic and process loopback streams are fed into input channels, passed through input pre-gain, OpenClipguard 2.0 (for mic), and sent via smoothed cross-point nodes into 5 independent 32-bit float mix buses (Stream, Monitor, VOD, Discord, Aux) with Master Brickwall Limiters. Monitor mix is rendered to default audio endpoint via WASAPI Playback.

**Tech Stack:** Rust (2024 edition), `windows` 0.58 crate (WASAPI, COM, MMCSS), `rtrb` (lock-free SPSC ringbuffers), 32-bit Float Audio (IEEE 754).

**Spec:** `docs/MASTER_ARCHITECTURE_GUIDE.md` (Section 4), `docs/AI_DEVELOPER_PROMPT.md` (Section 5).

## Global Constraints
- Target platform: Strictly Windows 11 x64 (no Linux/macOS, no Stream Deck dependencies).
- Hard Real-Time Audio: zero heap allocations (`malloc`/`vec![]`/`format![]`), zero mutex locks, zero file/network I/O in the DSP and audio callback loops.
- All internal processing is 32-bit IEEE 754 Float stereo at 48 kHz.
- Output peak of OpenClipguard must strictly never exceed 0 dBFS (1.0) with smooth hyperbolic tangent ($\tanh$) saturation curve.

---

### Task 1: Zipper Noise Reduction & Fader Smoother (`crates/owl-core/src/dsp/smoother.rs`)

**Files:**
- Create: `crates/owl-core/src/dsp/mod.rs`
- Create: `crates/owl-core/src/dsp/smoother.rs`
- Modify: `crates/owl-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct OnePoleSmoother` with `new(sample_rate: f32, tau_seconds: f32) -> Self`, `set_target(&mut self, target: f32)`, `next_sample(&mut self) -> f32`, `current(&self) -> f32`, `reset(&mut self, value: f32)`.
  - `pub fn slider_to_gain(x: f32) -> f32` converting $[0.0, 1.0] \to [0.0, 3.98107]$ ($-\infty \dots +12\text{ dB}$).
  - `pub fn slider_to_db(x: f32) -> f32`.

- [ ] **Step 1: Write failing unit tests for `OnePoleSmoother` and `slider_to_gain`**
- [ ] **Step 2: Run tests to verify failure**
- [ ] **Step 3: Implement `OnePoleSmoother` and slider curve conversions**
- [ ] **Step 4: Run tests and verify they pass**

---

### Task 2: OpenClipguard 2.0 DSP Limiter & Soft Saturation (`crates/owl-core/src/dsp/clipguard.rs`)

**Files:**
- Create: `crates/owl-core/src/dsp/clipguard.rs`
- Modify: `crates/owl-core/src/dsp/mod.rs`
- Modify: `crates/owl-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct OpenClipguard` with `new(sample_rate: u32) -> Self`, `process_stereo(&mut self, left: &mut [f32], right: &mut [f32])`, `process_interleaved(&mut self, samples: &mut [f32])`, `process_sample_pair(&mut self, l: f32, r: f32) -> (f32, f32)`.
  - Features 1.5 ms circular lookahead buffer (72 frames at 48 kHz), threshold -0.5 dBFS (~0.944), attack 0.05 ms, release 60 ms, and smooth $\tanh$ soft saturation. Zero allocations during processing.

- [ ] **Step 1: Write failing unit test for `OpenClipguard` (+6 dBFS sine overload peak <= 1.0, smooth saturation, delay verification)**
- [ ] **Step 2: Run test to verify failure**
- [ ] **Step 3: Implement `OpenClipguard` zero-allocation DSP engine**
- [ ] **Step 4: Run test and verify it passes**

---

### Task 3: Master Brickwall Limiter (`crates/owl-core/src/dsp/limiter.rs`)

**Files:**
- Create: `crates/owl-core/src/dsp/limiter.rs`
- Modify: `crates/owl-core/src/dsp/mod.rs`

**Interfaces:**
- Produces:
  - `pub struct MasterBrickwallLimiter` with `new(sample_rate: u32) -> Self`, `process_frame(&mut self, left: &mut f32, right: &mut f32)`.

- [ ] **Step 1: Write unit test for `MasterBrickwallLimiter`**
- [ ] **Step 2: Implement limiter with soft knee and ceiling at 1.0 (0 dBFS)**
- [ ] **Step 3: Verify tests pass**

---

### Task 4: 32-bit Float Matrix Routing Engine (`crates/owl-core/src/matrix/mod.rs`)

**Files:**
- Create: `crates/owl-core/src/matrix/mod.rs`
- Create: `crates/owl-core/src/matrix/channel.rs`
- Create: `crates/owl-core/src/matrix/bus.rs`
- Modify: `crates/owl-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub enum MixBusId { Stream = 0, Monitor = 1, Vod = 2, Discord = 3, Aux = 4 }`
  - `pub struct MatrixSendNode` (gain, smoother, mute, pre_fader)
  - `pub struct InputChannel` (id, name, pre_gain, clipguard, sends)
  - `pub struct RoutingMatrix` with `new()`, `add_channel(...)`, `set_send_gain(...)`, `set_send_mute(...)`, `set_master_gain(...)`, `process_block(...)`
  - Independent 5 output mix buffers, true-peak limiting on each bus, and channel isolation.

- [ ] **Step 1: Write unit test for matrix isolation and routing**
- [ ] **Step 2: Implement `MatrixSendNode`, `InputChannel`, `MixBus`, `RoutingMatrix`**
- [ ] **Step 3: Verify matrix routing tests pass**

---

### Task 5: Hardware WASAPI Microphone Capture (`crates/owl-core/src/capture/mic.rs`)

**Files:**
- Create: `crates/owl-core/src/capture/mic.rs`
- Modify: `crates/owl-core/src/capture/mod.rs`
- Modify: `crates/owl-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct MicrophoneCapture` with `new() -> Result<Self>`, `start() -> Result<()>`, `stop()`, `take_consumer() -> Option<rtrb::Consumer<f32>>`, `meter() -> Arc<AudioMeterState>`.

- [ ] **Step 1: Implement WASAPI default capture device (`eCapture`, `eConsole`) initialization**
- [ ] **Step 2: Real-time worker thread with MMCSS Audio priority pushing into SPSC ring buffer**
- [ ] **Step 3: Integration test and compile check**

---

### Task 6: WASAPI Playback for Monitor Mix (`crates/owl-core/src/playback.rs`)

**Files:**
- Create: `crates/owl-core/src/playback.rs`
- Modify: `crates/owl-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct WasapiPlayback` with `new() -> Result<Self>`, `start(producer_or_source) -> Result<()>`, `stop()`.
  - Event-driven `AUDCLNT_STREAMFLAGS_EVENTCALLBACK`, low-latency shared mode render client on default `eRender` device.

- [ ] **Step 1: Implement `WasapiPlayback` device initialization and render worker**
- [ ] **Step 2: Write unit/integration tests with silent/mock render**
- [ ] **Step 3: Verify compilation and tests**

---

### Task 7: Update `crates/owl-daemon/src/main.rs` & Multi-Mix Live Console Demo

**Files:**
- Modify: `crates/owl-daemon/src/main.rs`

**Interfaces:**
- Connects:
  - Process Loopback capture + Microphone capture -> `RoutingMatrix` -> `Stream Mix` (Music 30%, Mic 100%) & `Monitor Mix` (Music 100%, Mic mute/balance).
  - Feeds `Monitor Mix` directly to `WasapiPlayback` in real time.
  - Live animated console ASCII VU meters for both `Stream Mix` and `Monitor Mix` concurrently.

- [ ] **Step 1: Add `--matrix` mode and dual-mix pipeline in `owl-daemon`**
- [ ] **Step 2: Run `cargo test --all` and verify all tests pass**
- [ ] **Step 3: Smoke test `cargo run -p owl-daemon -- --help` and verify build**
