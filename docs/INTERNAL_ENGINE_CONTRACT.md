# OpenWaveLink — FROZEN Internal Contract (2026-10-10)

**Status: FROZEN.** This file is the single source of truth for the interfaces between the
audio engine (`owl-core`), the daemon (`owl-daemon`), and the UI (`owl-app`).
Do not change a signature here without telling the Lead first.

---

## 0. Verified root causes being fixed

| # | Defect | Evidence (file:line) |
|---|---|---|
| 1 | Only ONE `ProcessAudioCapture` is created, at daemon startup, for one PID. `owl.assignProcess` only rewrites a text field and `ch.channel_type`; **no capture stream is ever started for a newly added app**. | `crates/owl-daemon/src/main.rs:119`; `crates/owl-core/src/matrix/mod.rs:118-122`; `crates/owl-daemon/src/server.rs:536-573` |
| 2 | `WasapiPlayback` binds `GetDefaultAudioEndpoint(eRender, eConsole)` once at start. The `on <device>` selector in the header **makes no backend call at all** — it only sets local React state. | `crates/owl-core/src/playback.rs:76-81`; `crates/owl-app/ui/src/components/HeaderBar.tsx:184-187`; `crates/owl-app/ui/src/components/MatrixView.tsx:398-399` |
| 3 | The mixer loop hardcodes exactly two inputs (channel ids 1 and 2) and reads only two ring consumers. | `crates/owl-daemon/src/main.rs:459-474` |

---

## 1. `owl-core::matrix` — additions (do NOT break `process_interleaved`)

Existing `process_interleaved(&mut self, inputs: &mut [(u32, &mut [f32], &mut [f32])], outputs: &mut [&mut [f32]; NUM_MIX_BUSES])`
must keep working unchanged. **Exactly 3 call sites depend on it** (corrected — an earlier
revision of this file wrongly said 4):

- `crates/owl-core/src/matrix/mod.rs:434` — inside `test_matrix_isolation`
- `crates/owl-core/src/matrix/mod.rs:541` — inside `test_matrix_commands_queue`
- `crates/owl-core/tests/test_shared_memory.rs:283` — inside `test_lockfree_matrix_commands_realtime_application`

(`process_interleaved` also exists on `OpenClipguard` at `dsp/clipguard.rs:163` and on
`MasterBrickwallLimiter` at `dsp/limiter.rs:76` — those are unrelated types, not call sites
of the matrix method.)

Refactor it into two new public methods and re-implement `process_interleaved` in terms of them:

```rust
/// Process ONE input channel into the mix buses (pre-fader, sends, metering).
/// `frames` actually used = min(in_l.len(), in_r.len(), outputs[0].len()/2).
pub fn process_channel_into(
    &mut self,
    channel_id: u32,
    in_l: &mut [f32],
    in_r: &mut [f32],
    outputs_interleaved: &mut [&mut [f32]; NUM_MIX_BUSES],
);

/// Master section for all buses: master fader -> brickwall limiter -> bus meter.
pub fn finalize_buses(
    &mut self,
    frame_count: usize,
    outputs_interleaved: &mut [&mut [f32]; NUM_MIX_BUSES],
);
```

`process_channel_into` MUST apply, in this order: Clipguard (if present) -> VST3 rack ->
per-sample channel fader + send gain -> sum into buses -> channel meter update.
It MUST NOT clear the output buffers.

`finalize_buses` MUST apply master fader (or 0 when muted) -> limiter -> bus meter.

Also change channel lookup to be **ID-first, index-fallback** (removes the index/ID ambiguity
once channels can be deleted):

```rust
fn find_channel_mut(&mut self, key: usize) -> Option<&mut MatrixInputChannel> {
    if let Some(pos) = self.channels.iter().position(|c| c.id == key as u32) {
        self.channels.get_mut(pos)
    } else {
        self.channels.get_mut(key)   // legacy index fallback
    }
}
```

Add:

```rust
pub fn remove_channel(&mut self, channel_id: u32) -> bool;   // true if removed
pub fn channel_ids(&self) -> Vec<u32>;
```

---

## 2. `owl-core::engine` — NEW module (`crates/owl-core/src/engine.rs`)

The real-time engine owns the matrix, the dynamic capture slots, the output buffers and the
playback producer. **Zero heap allocation inside `process_block`.**

```rust
pub const BLOCK_FRAMES: usize = 256;

pub enum EngineEvent {
    AddChannel { channel: MatrixInputChannel },
    RemoveChannel { channel_id: u32 },
    AttachCapture { channel_id: u32, pid: u32, consumer: rtrb::Consumer<f32> },
    DetachCapture { channel_id: u32 },
    SwapOutput { producer: rtrb::Producer<f32> },
}

pub struct AudioEngine { /* private */ }

impl AudioEngine {
    pub fn new(sample_rate: u32) -> Self;

    /// Create the lock-free control queue; the returned Producer goes to the Control Plane.
    pub fn attach_engine_queue(&mut self, capacity: usize) -> rtrb::Producer<EngineEvent>;

    /// Create and attach the matrix's parameter queue (channel/send/mix gain & mute).
    /// Delegates to `RoutingMatrix::attach_command_queue`. The returned Producer is
    /// what `ServerState.cmd_producer` feeds, so `owl.setChannel` / `owl.setSend` /
    /// `owl.setMix` keep working exactly as before.
    pub fn attach_command_queue(&mut self, capacity: usize) -> rtrb::Producer<MatrixCommand>;

    /// Register an input channel (the matrix owns it from here on).
    /// Callable before the engine is moved into the real-time thread.
    pub fn add_channel(&mut self, channel: MatrixInputChannel) -> u32;

    /// Attach a live capture ring to an existing channel. Returns false if no such channel.
    pub fn attach_capture(&mut self, channel_id: u32, pid: u32, consumer: rtrb::Consumer<f32>) -> bool;

    /// Detach and drop the capture ring of a channel. Returns false if none was attached.
    pub fn detach_capture(&mut self, channel_id: u32) -> bool;

    /// Remove a channel from the matrix entirely (detaches its capture first).
    pub fn remove_channel(&mut self, channel_id: u32) -> bool;

    /// Drain engine events, pull every attached capture ring, run the matrix,
    /// store the results, and push the Monitor mix into the playback producer.
    /// Returns the number of frames processed this block (0 when nothing was available).
    pub fn process_block(&mut self) -> usize;

    /// Frames produced by the most recent `process_block`.
    pub fn frames(&self) -> usize;

    /// Interleaved Stream Mix for the current block, `frames()*2` samples.
    pub fn stream_mix(&self) -> &[f32];
    /// Interleaved Monitor Mix for the current block, `frames()*2` samples.
    pub fn monitor_mix(&self) -> &[f32];

    pub fn matrix(&self) -> &RoutingMatrix;
    pub fn matrix_mut(&mut self) -> &mut RoutingMatrix;

    /// True when at least one capture ring is attached.
    pub fn has_active_captures(&self) -> bool;
    /// Channel ids that currently have a live capture ring.
    pub fn active_capture_ids(&self) -> Vec<u32>;

    /// Publish VU meters into pre-allocated slices (zero-allocation).
    pub fn write_telemetry_samples(
        &self,
        out_channels: &mut [owl_ipc::MeterSample],
        out_mixes: &mut [owl_ipc::MeterSample],
    );
}
```

> **Revision note (2026-10-10):** `EngineEvent::AddChannel` / `EngineEvent::RemoveChannel`
> and `AudioEngine::attach_command_queue` were added after `daemon-dev` correctly pointed out
> that the original §2 made the runtime RPCs unimplementable: `add_channel(&mut self)` is only
> callable before the engine moves into the real-time thread, and nothing exposed the
> `MatrixCommand` queue that `ServerState.cmd_producer` must feed. The additions are purely
> additive — no previously frozen signature changed.


Behaviour requirements:
- `process_block` MUST be allocation-free. Use fixed `[f32; BLOCK_FRAMES]` scratch per slot.
- Slots are indexed by matrix channel id; support at least `owl_ipc::MAX_CHANNELS` (32).
- A slot whose ring yields 0 frames contributes silence and MUST NOT be skipped in a way that
  corrupts other slots' offsets.
- `finalize_buses` runs over `max(frames across slots)`.
- If `playback_producer` is present, push the Monitor mix interleaved samples into it
  (drop on full — never block).

---

## 3. `owl-core::WasapiPlayback` — device selection

Keep `new()` working (default endpoint). Add:

```rust
/// Bind to a specific CoreAudio render endpoint id (`IMMDeviceEnumerator::GetDevice`).
/// `None` means the default eRender/eConsole endpoint (identical to `new()`).
pub fn new_with_device(endpoint_id: Option<&str>) -> Result<Self, CaptureError>;

/// Human-readable name of the endpoint currently bound (after `start()`).
pub fn device_name(&self) -> Option<&str>;
```

Add to `AudioFormatInfo` usage: the playback worker must keep writing via `write_stereo_frame`.

---

## 4. `owl-daemon` RPC contract (JSON-RPC 2.0 over ws://127.0.0.1:1824)

Existing methods keep working. NEW methods:

| Method | Params | Result |
|---|---|---|
| `owl.addChannel` | `{ name: string, channel_type: "SoftwareProcess"\|"Microphone", pid: u32 }` | `{ success: bool, channel_id: u32, capture_started: bool, message: string\|null }` |
| `owl.removeChannel` | `{ channel_id: u32 }` | `{ success: bool, channel_id: u32 }` |
| `owl.setOutputDevice` | `{ endpoint_id: string }` | `{ success: bool, endpoint_id: string, device_name: string, message: string\|null }` |
| `owl.getEngineState` | `{}` | `{ channels: [ChannelState], mixes: [MixState], output_device_id: string, output_device_name: string }` |

### Honest-failure guarantees (added 2026-10-10 after adversarial review)

These are load-bearing. The whole point of this rework was to stop the daemon reporting success
for channels that can never produce audio.

1. **`success` mirrors `capture_started`.** `owl.addChannel` returns `success: false` whenever no
   capture stream actually started. A client checking only `success` must never be told a dead
   channel is live.
2. **A PID that does not exist is rejected.** `ProcessAudioCapture::start` calls
   `owl_core::capture::process_is_alive(pid)` first and returns
   `CaptureError::ProcessNotFound(pid)`. `ActivateAudioInterfaceAsync` happily succeeds for a dead
   PID and returns a client that never delivers a sample, so without this check
   `capture_started: true` was a lie. Verified live: `addChannel{pid:999999}` →
   `{"capture_started":false,"success":false,"message":"...Target process PID 999999 does not exist..."}`.
   `process_is_alive` distinguishes `ERROR_ACCESS_DENIED` (process exists but is protected →
   report alive) from any other failure (→ report gone).
3. **No ghost channels.** If the capture fails to start *or* no supervisor is attached, the
   channel is rolled back out of engine state and `channelsListChanged` is re-broadcast, so a UI
   row created optimistically is corrected rather than left claiming a working channel.
4. **`output_device_name` is the endpoint actually bound**, not an echo of the request.
   An unknown `endpoint_id` fails with `success: false` — never a silent fallback to the default.

`channel_id` is the authoritative id assigned by the daemon (1, 2, 3, …) and is what the UI
must store and send back. `owl.setChannel` / `owl.setSend` / `owl.setMix` accept `channel_id`
and `mix_idx`.

Notifications broadcast to clients: `channelChanged`, `mixChanged`, and NEW
`channelsListChanged` (payload `{ channels: [...] }`) after add/remove.

---

## 5. Tauri commands (`owl-app`) — UI-facing

| Command | Args | Returns |
|---|---|---|
| `add_channel` | `name: String, channel_type: String, pid: u32` | `u32` (channel_id) |
| `remove_channel` | `channel_id: u32` | `bool` |
| `set_output_device` | `endpoint_id: String` | `String` (resolved device name) |
| `get_engine_state` | — | `EngineStateDto` |

TS wrappers in `crates/owl-app/ui/src/ipc.ts`:

```ts
export const addChannel = (name: string, channel_type: string, pid: number) =>
  invokeCommand<number>("add_channel", { name, channelType: channel_type, pid });
export const removeChannel = (channel_id: number) =>
  invokeCommand<boolean>("remove_channel", { channelId: channel_id });
export const setOutputDevice = (endpoint_id: string) =>
  invokeCommand<string>("set_output_device", { endpointId: endpoint_id });
export const getEngineState = () => invokeCommand<EngineStateDto>("get_engine_state");
```

`EngineStateDto` = `{ channels: {id,name,channel_type,pid,gain,is_muted}[]; mixes: {id,name,gain,is_muted}[]; output_device_id: string; output_device_name: string }`.

**The `on <device>` selector MUST call `setOutputDevice(endpoint.id)` with the real
`IMMDeviceEnumerator` endpoint id — never just local state.**

---

## 6. UI design tokens — from `WAVE_LINK_3_UI_SPEC.md`

| Token | Value | Use |
|---|---|---|
| window bg | `#16141a` | app canvas |
| input row card | `#211d27` | left channel row |
| cross-point card | `#272230` | active matrix cell |
| empty cell | `#191621` | inactive cell |
| mix header card | `#262130` | mix column header |
| border | `1px solid rgba(255,255,255,0.06)` | all cards |
| monitor indicator | `#c084fc`, height `3px` | top edge of the mix selected in `Listening to` |
| slider thumb | `#d88be2` | round, **no glow** |
| slider track | `#332d3d` | inactive track |
| slider progress | `#a855f7` | active track, no glow |
| MUTE bg | `#3a1c24` | matte dark maroon |
| MUTE border | `#ef4444` | thin red |
| category text | `#9ca3af` | 11px uppercase font-semibold tracking-wider |
| primary text | `#e4e4e7` | |
| secondary text | `#71717a` | |
| dropdown bg | `#201c2b` | `+ Create channel` panel, border `rgba(255,255,255,0.08)` |
| dropdown search | bg `#16131f`, border `#3d374a` | |

**Hard prohibitions:** no neon, no glows, no gradients, no coloured borders, no all-caps
`LISTENING` label, no dead gear/arrow/empty-icon buttons, no mock/fake app lists.
`+ Create channel` panel opens **strictly downward**: `top-full mt-2 left-0 z-50`,
`max-h-[480px]`, one single `+` in the button.

---

## 7. VERIFIED machine state — why there is no sound (measured 2026-10-10)

Enumerated live via `IMMDeviceEnumerator` (`cargo test -p owl-core devices::tests::test_enumerate_devices -- --nocapture`).

**THE DEFAULT RENDER ENDPOINT IS A VIRTUAL CABLE, NOT HEADPHONES:**

```
DEFAULT RENDER = "Spotify (Elgato Virtual Audio)"   {0.0.0.00000000}.{5f1f5057-85bd-4c50-97b1-467d17a665f6}
```

`WasapiPlayback` binds `GetDefaultAudioEndpoint(eRender, eConsole)` **once** at startup
(`crates/owl-core/src/playback.rs:76-81`). So the monitor mix is rendered into the Elgato
"Spotify" virtual cable — the user hears nothing. This is the mechanical reason for
"звука нет вообще", independent of the missing capture pool.

Real render endpoints (7):

| Name | Endpoint id |
|---|---|
| Spotify (Elgato Virtual Audio) — **default** | `{0.0.0.00000000}.{5f1f5057-85bd-4c50-97b1-467d17a665f6}` |
| **Speakers (Sound Blaster Play! 3) — the user's real headphones** | `{0.0.0.00000000}.{cca3f6ac-59d9-4b55-ad5b-0f6c645e7f2a}` |
| System (Elgato Virtual Audio) | `{0.0.0.00000000}.{c31b4406-a2a4-4ef0-bfb6-d9a484f5ff3f}` |
| Discord (Elgato Virtual Audio) | `{0.0.0.00000000}.{171ac91f-ca3c-4cbe-9697-95f9b631182f}` |
| Counter-Strike 2 (Elgato Virtual Audio) | `{0.0.0.00000000}.{7ac46718-51cf-47c1-ae20-00884aa47776}` |
| CABLE Input (VB-Audio Virtual Cable) | `{0.0.0.00000000}.{2d7292e2-89ca-4a98-bcfd-8d090ae6ee57}` |
| CABLE Output (VB-Audio Virtual Cable) (Elgato Virtual Audio) | `{0.0.0.00000000}.{e968cd56-3c59-4e9f-9df7-7d8bef1f8228}` |

Real capture endpoints (5): `Microphone (fifine AM8 Pro)` (default),
`Mic (Elgato Virtual Audio)`, `Mic++ (Elgato Virtual Audio)`,
`Personal Mix (Elgato Virtual Audio)`,
`CABLE Output (VB-Audio Virtual Cable)` `{0.0.1.00000000}.{f7162a82-c52d-40b5-8b1d-bd1b30aff21e}`.

### Consequence for `setOutputDevice`

`owl.setOutputDevice` **must default the UI to the user's real headphones**, and the UI must
never silently keep the virtual-cable default. When no explicit device has been chosen yet,
the header selector must show a real endpoint, not "Default Audio Output".

---

## 8. Virtual endpoints — HONEST status (§2 of the user's request)

**Verified: the in-repo kernel driver CANNOT be built or loaded on this machine.**

| Fact | Evidence |
|---|---|
| `driver/package/OpenWaveLinkAudio.sys` is **51 bytes of ASCII**, not a PE binary | `Format-Hex`: first bytes `4F 70 65 6E` ("Open"), no `MZ` header |
| That fake file is produced by the build script itself, on MSBuild failure | `driver/build_driver.bat:58-61` — `echo OpenWaveLink ... > OpenWaveLinkAudio.sys` |
| The WDK VS toolset is not installed | MSBuild: `error MSB8020: The build tools for WindowsKernelModeDriver10.0 ... cannot be found` |
| `C:\Program Files (x86)\Windows Kits\10\Vsix` is empty; no `*WDK*` under VS `Common7\IDE\Extensions` | directory listings |
| No `.cat` catalog exists anywhere, yet the INF requires one | `driver/OpenWaveLinkAudio.inf:13` — `CatalogFile = OpenWaveLinkAudio.cat` |
| Test signing is OFF and cannot be enabled under Secure Boot | `bcdedit /enum {current}` has no `testsigning`; `Confirm-SecureBootUEFI` = `True` |
| HVCI is ON — an unsigned binary will not load even in test mode | `HypervisorEnforcedCodeIntegrity\Enabled` = `1`, `SecurityServicesRunning` = `{2}` |
| No driver-signing certificate exists | `Cert:\LocalMachine\My` empty; `Cert:\CurrentUser\PrivateCertStore` empty |
| The driver sources do not compile even with a toolset | ≥7 errors: include-order breaks `portcls.h`; `GetHardwareLatency` vs `GetHWLatency`; `IPortWaveRT::Init` arity; `PcDispatchProperty` does not exist in any WDK header; `KSRTAUDIO_HWLATENCY` has `ChipsetDelay`/`CodecDelay`, not `ChipWriteDelay`/`ChipReadDelay` |
| Even if it built, the render path is inert | `COwlSharedBridge::ReadRenderSamples` has **zero call sites**; `m_pNotificationEvents` are never signalled on `KSSTATE_RUN`; `UpdatePositionRegister()` is never called; no user-mode code calls `DeviceIoControl` anywhere in `crates/` |

**What IS achievable today, with zero driver work:** the machine already has
**signed, working virtual audio cables** installed and running:

- `VBAudioVACMME` — "VB-Audio Virtual Cable (WDM)", state **Running**
- `fifineVirtualAudioDriver` — "fifine Driver", state **Running**

The VB-Audio pair gives a real, Windows-visible loopback:

```
render into  CABLE Input  {0.0.0.00000000}.{2d7292e2-89ca-4a98-bcfd-8d090ae6ee57}
capture from CABLE Output {0.0.1.00000000}.{f7162a82-c52d-40b5-8b1d-bd1b30aff21e}
```

That is the honest path to "a mix visible in Windows as a microphone for Discord/OBS"
**without** building, signing, or loading a kernel driver. Wiring this up is a follow-up
task; it is NOT part of the three defects being fixed now.

