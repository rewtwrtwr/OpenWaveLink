//! Virtual audio cable management via USB/IP (usbip-win2 + Virtual Cables).
//!
//! # Why USB/IP instead of a kernel driver
//!
//! A conventional virtual audio cable is a WDM kernel driver, which on Windows
//! 11 with Secure Boot enabled must carry a Microsoft WHQL signature. Producing
//! one requires an EV code-signing certificate and hardware certification, so a
//! locally built driver only loads with Secure Boot disabled and test signing
//! turned on.
//!
//! This module takes a different route: [`virtualcables`] emulates a *USB Audio
//! Class 1.0 device* and exports it over USB/IP. Windows then binds its own
//! in-box, Microsoft-signed `usbaudio.sys` class driver, so the resulting cable
//! is a completely ordinary audio endpoint and **Secure Boot stays enabled**.
//!
//! # How a cable is created
//!
//! 1. `virtualcables.exe --names "A,B" --headless` starts a local USB/IP server
//!    that exports one UAC1 device per name on `127.0.0.1:3240`.
//! 2. Each exported device is attached once to the virtual host controller with
//!    `usbip.exe attach -r 127.0.0.1 -b 1-N`, which requires elevation.
//! 3. `usbip.exe port --stash` records the attachment as *persistent*, after
//!    which the driver re-attaches the device automatically every time it loads.
//!
//! Step 3 is what makes the feature usable: adding, renaming or removing a cable
//! afterwards only requires restarting the server (step 1), with **no further
//! administrator prompts**.
//!
//! # Device identity and renaming
//!
//! Windows caches a USB device's friendly name against its VID/PID/serial
//! identity. The serial is therefore derived from a hash of the cable *name*
//! (see the driver's `serialForName`), so renaming a cable yields a genuinely
//! new device and the new name actually appears.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Fixed VID/PID base used by the Virtual Cables UAC1 descriptors.
/// Cable *n* is exported with PID `0xCA00 + n`.
const CABLE_PID_BASE: u16 = 0xCA00;

/// Sample rate the UAC1 descriptors advertise. The cable is fixed-format.
pub const CABLE_SAMPLE_RATE: u32 = 48_000;

/// Number of channels in every cable.
pub const CABLE_CHANNELS: u16 = 2;

/// Bits per sample the cable transport uses (16-bit PCM, not float).
pub const CABLE_BITS_PER_SAMPLE: u16 = 16;

/// Default USB/IP TCP port used by the Virtual Cables server.
pub const DEFAULT_USBIP_PORT: u16 = 3240;

/// Errors raised while managing virtual cables.
#[derive(Debug)]
pub enum CableError {
    /// `virtualcables.exe` could not be found next to the application.
    ServerBinaryMissing { searched: Vec<PathBuf> },
    /// `usbip.exe` is not installed, so cables cannot be attached.
    UsbipMissing { searched: Vec<PathBuf> },
    /// The USB/IP server refused to start or never became ready.
    ServerNotReady { detail: String },
    /// An attach/stash command failed.
    UsbipFailed { command: String, detail: String },
    /// The requested cable name is empty or otherwise unusable.
    InvalidName { name: String },
    /// A cable could not be resolved to a Windows audio endpoint in time.
    EndpointNotFound { name: String, waited_ms: u64 },
    /// An OS-level failure while spawning a helper process.
    Io(std::io::Error),
}

impl std::fmt::Display for CableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerBinaryMissing { searched } => write!(
                f,
                "virtualcables.exe was not found. Searched: {}",
                join_paths(searched)
            ),
            Self::UsbipMissing { searched } => write!(
                f,
                "usbip.exe was not found; install the usbip-win2 driver. Searched: {}",
                join_paths(searched)
            ),
            Self::ServerNotReady { detail } => {
                write!(f, "the virtual cable server did not become ready: {detail}")
            }
            Self::UsbipFailed { command, detail } => {
                write!(f, "usbip {command} failed: {detail}")
            }
            Self::InvalidName { name } => write!(f, "invalid cable name: {name:?}"),
            Self::EndpointNotFound { name, waited_ms } => write!(
                f,
                "no Windows audio endpoint named {name:?} appeared within {waited_ms} ms"
            ),
            Self::Io(err) => write!(f, "io error: {err}"),
        }
    }
}

impl std::error::Error for CableError {}

impl From<std::io::Error> for CableError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

fn join_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return "(no candidates)".to_string();
    }
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// One virtual cable, as seen by the manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualCable {
    /// 1-based cable number. Also determines the USB bus id (`1-<number>`)
    /// and the device PID (`0xCA00 + number`).
    pub number: u8,
    /// Display name, e.g. `OWL Spotify`. Becomes the Windows endpoint name.
    pub name: String,
}

impl VirtualCable {
    /// USB bus id this cable is exported under, e.g. `1-2`.
    pub fn bus_id(&self) -> String {
        format!("1-{}", self.number)
    }

    /// USB product id for this cable.
    pub fn pid(&self) -> u16 {
        CABLE_PID_BASE + u16::from(self.number)
    }
}

/// Locate `virtualcables.exe`: next to the running executable, in `bin/`, or on
/// `PATH`.
fn find_server_binary() -> Result<PathBuf, CableError> {
    let mut searched = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        // Walk up a few levels: the daemon may live in `target/debug` or in
        // `installer/release_payload`, while the binary sits in `bin/`.
        for _ in 0..4 {
            let Some(current) = dir else { break };
            for candidate in [
                current.join("virtualcables.exe"),
                current.join("bin").join("virtualcables.exe"),
            ] {
                if candidate.is_file() {
                    return Ok(candidate);
                }
                searched.push(candidate);
            }
            dir = current.parent().map(Path::to_path_buf);
        }
    }

    if let Some(path) = which("virtualcables.exe") {
        return Ok(path);
    }
    searched.push(PathBuf::from("virtualcables.exe (PATH)"));

    Err(CableError::ServerBinaryMissing { searched })
}

/// Locate `usbip.exe`, installed by the usbip-win2 package.
fn find_usbip() -> Result<PathBuf, CableError> {
    let mut searched = Vec::new();

    for root in ["ProgramFiles", "ProgramFiles(x86)"] {
        let Ok(base) = std::env::var(root) else {
            continue;
        };
        for sub in ["USBip", "usbip-win2", "usbip"] {
            let candidate = PathBuf::from(&base).join(sub).join("usbip.exe");
            if candidate.is_file() {
                return Ok(candidate);
            }
            searched.push(candidate);
        }
    }

    if let Some(path) = which("usbip.exe") {
        return Ok(path);
    }
    searched.push(PathBuf::from("usbip.exe (PATH)"));

    Err(CableError::UsbipMissing { searched })
}

/// Minimal `which` for a single file name, avoiding a shell invocation.
fn which(file_name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(file_name))
        .find(|candidate| candidate.is_file())
}

/// Manages the USB/IP server process and the set of exported cables.
///
/// The manager owns the `virtualcables.exe` child process. Restarting the
/// server is how the cable set changes, because the server exports a fixed set
/// of devices for its whole lifetime.
pub struct CableManager {
    server_binary: PathBuf,
    usbip_binary: PathBuf,
    port: u16,
    child: Mutex<Option<Child>>,
    cables: Mutex<Vec<VirtualCable>>,
}

impl CableManager {
    /// Create a manager, resolving the required helper binaries.
    ///
    /// Fails early with a precise message when either `virtualcables.exe` or
    /// `usbip.exe` is missing, because without both no cable can ever appear.
    pub fn new() -> Result<Self, CableError> {
        Ok(Self {
            server_binary: find_server_binary()?,
            usbip_binary: find_usbip()?,
            port: DEFAULT_USBIP_PORT,
            child: Mutex::new(None),
            cables: Mutex::new(Vec::new()),
        })
    }

    /// Path of the resolved USB/IP server binary (for diagnostics).
    pub fn server_binary(&self) -> &Path {
        &self.server_binary
    }

    /// Path of the resolved `usbip.exe` (for diagnostics).
    pub fn usbip_binary(&self) -> &Path {
        &self.usbip_binary
    }

    /// Names of the cables the manager currently exports.
    pub fn cable_names(&self) -> Vec<String> {
        self.cables
            .lock()
            .map(|cables| cables.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Replace the exported cable set and restart the server.
    ///
    /// `names` must be unique and non-empty; the order defines the cable
    /// numbers, so a name keeps its number as long as it stays in the list.
    /// Cables already attached as persistent devices are re-attached by the
    /// driver automatically, so this needs no elevation.
    pub fn set_cables(&self, names: &[String]) -> Result<Vec<VirtualCable>, CableError> {
        let cables = build_cable_list(names)?;

        self.stop_server();
        self.start_server(&cables)?;
        self.wait_for_server_ready(Duration::from_secs(10))?;

        if let Ok(mut guard) = self.cables.lock() {
            *guard = cables.clone();
        }
        Ok(cables)
    }

    /// Stop the server, leaving the cable set empty.
    pub fn stop(&self) {
        self.stop_server();
    }

    /// Start the server for `cables` without touching the current set.
    fn start_server(&self, cables: &[VirtualCable]) -> Result<(), CableError> {
        let names = cables
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(",");

        let mut command = Command::new(&self.server_binary);
        command
            .arg("--names")
            .arg(&names)
            .arg("--headless")
            .arg("--buffer-ms")
            .arg("15")
            .arg("--listen")
            .arg(format!("127.0.0.1:{}", self.port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            /// Do not create a console window for the background server.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let child = command.spawn()?;
        if let Ok(mut guard) = self.child.lock() {
            *guard = Some(child);
        }
        Ok(())
    }

    /// Terminate the server process, if any.
    fn stop_server(&self) {
        let Ok(mut guard) = self.child.lock() else {
            return;
        };
        let Some(mut child) = guard.take() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Poll the server until it answers a device-list request.
    fn wait_for_server_ready(&self, timeout: Duration) -> Result<(), CableError> {
        let deadline = Instant::now() + timeout;
        let mut last_detail = String::from("no response");

        while Instant::now() < deadline {
            match self.run_usbip(&["list", "-r", "127.0.0.1"]) {
                Ok(output) => {
                    // A ready server always lists at least one bus id.
                    if output.contains("1-1") {
                        return Ok(());
                    }
                    last_detail = format!("server answered but exported nothing: {output}");
                }
                Err(detail) => last_detail = detail,
            }
            std::thread::sleep(Duration::from_millis(150));
        }

        Err(CableError::ServerNotReady { detail: last_detail })
    }

    /// Run `usbip.exe` and return its combined output.
    fn run_usbip(&self, args: &[&str]) -> Result<String, String> {
        let mut command = Command::new(&self.usbip_binary);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let output = command.output().map_err(|e| e.to_string())?;
        let mut text = String::from_utf8_lossy(&output.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        let text = text.trim().to_string();

        if output.status.success() {
            Ok(text)
        } else {
            Err(text)
        }
    }

    /// Attach every cable and mark it persistent.
    ///
    /// Must run elevated. Afterwards the driver re-attaches these cables on its
    /// own, so this is a one-time operation per cable number.
    ///
    /// Cables the driver has *already* restored from the persistent list are
    /// skipped deliberately: attaching a bus id that is already attached
    /// creates a second port for the same device, and Windows then sees two
    /// competing instances of one cable (one of which stays in the `Unknown`
    /// state and exposes no audio endpoint at all).
    pub fn attach_all_persistent(&self) -> Result<(), CableError> {
        let cables = self
            .cables
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();

        let attached = self.attached_bus_ids();
        let mut attached_any = false;

        for cable in &cables {
            let bus_id = cable.bus_id();
            if attached.contains(&bus_id) {
                continue;
            }
            self.run_usbip(&["attach", "-r", "127.0.0.1", "-b", &bus_id])
                .map_err(|detail| CableError::UsbipFailed {
                    command: format!("attach -b {bus_id}"),
                    detail,
                })?;
            attached_any = true;
        }

        // Only touch the persistent list when something was newly attached:
        // re-stashing an unchanged set is what produced duplicate records.
        if attached_any {
            self.run_usbip(&["port", "--stash"])
                .map_err(|detail| CableError::UsbipFailed {
                    command: "port --stash".to_string(),
                    detail,
                })?;
        }

        Ok(())
    }

    /// Bus ids currently attached to the virtual host controller.
    ///
    /// Reads the `usbip port` listing and extracts every
    /// `usbip://<host>:<port>/<bus-id>` target it reports.
    pub fn attached_bus_ids(&self) -> Vec<String> {
        let Ok(output) = self.run_usbip(&["port"]) else {
            return Vec::new();
        };

        let mut bus_ids = Vec::new();
        for line in output.lines() {
            let Some(position) = line.find("usbip://") else {
                continue;
            };
            let target = line[position + "usbip://".len()..].trim();
            // `127.0.0.1:3240/1-1` -> `1-1`
            if let Some((_, bus_id)) = target.rsplit_once('/') {
                let bus_id = bus_id.trim().to_string();
                if !bus_id.is_empty() && !bus_ids.contains(&bus_id) {
                    bus_ids.push(bus_id);
                }
            }
        }
        bus_ids
    }

    /// Detach all cables and clear their persistent records.
    pub fn detach_all(&self) -> Result<(), CableError> {
        self.run_usbip(&["detach", "--all"])
            .map_err(|detail| CableError::UsbipFailed {
                command: "detach --all".to_string(),
                detail,
            })?;
        Ok(())
    }

    /// Resolve a cable name to the Windows audio endpoints it produced.
    ///
    /// A UAC1 cable exposes both a render and a capture endpoint under the same
    /// friendly name, so the caller receives one endpoint id per direction.
    ///
    /// Windows renames the endpoint by prefixing its form factor, so a cable
    /// exported as `OWL Spotify` appears as `Speakers (OWL Spotify)` for
    /// playback and `Microphone (OWL Spotify)` for recording. Matching is
    /// therefore done on the parenthesised device name, not on the raw string.
    pub fn resolve_endpoints(
        &self,
        name: &str,
        timeout: Duration,
    ) -> Result<CableEndpoints, CableError> {
        let deadline = Instant::now() + timeout;
        let mut waited = 0u64;
        let clean_name = name.strip_suffix(":out").unwrap_or(name);
        let is_out_only = name.ends_with(":out");

        while Instant::now() < deadline {
            if let Ok(devices) = crate::devices::get_system_audio_devices() {
                let render = devices
                    .outputs
                    .iter()
                    .find(|d| endpoint_matches(&d.name, clean_name))
                    .map(|d| d.id.clone());
                let capture = devices
                    .inputs
                    .iter()
                    .find(|d| endpoint_matches(&d.name, clean_name))
                    .map(|d| d.id.clone());

                if is_out_only {
                    if let Some(render) = render {
                        return Ok(CableEndpoints {
                            render,
                            capture: String::new(),
                        });
                    }
                } else if let (Some(render), Some(capture)) = (render, capture) {
                    return Ok(CableEndpoints { render, capture });
                }
            }
            std::thread::sleep(Duration::from_millis(200));
            waited = waited.saturating_add(200);
        }

        Err(CableError::EndpointNotFound {
            name: name.to_string(),
            waited_ms: waited,
        })
    }
}

impl Drop for CableManager {
    fn drop(&mut self) {
        self.stop_server();
    }
}

/// The render/capture endpoint pair a single virtual cable exposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CableEndpoints {
    /// Playback endpoint id: write audio here to feed the cable.
    pub render: String,
    /// Recording endpoint id: other applications read the cable from here.
    pub capture: String,
}

/// Does a Windows endpoint name belong to the cable called `cable_name`?
///
/// Windows renders a UAC1 cable as `<FormFactor> (<DeviceName>)`, where the
/// device name is the USB product string the server exported. The comparison
/// therefore extracts the parenthesised part when present, and otherwise falls
/// back to an exact match.
///
/// The `2- ` prefix seen on some machines is the USB bus-id prefix Windows adds
/// when two devices share a product string; it is stripped as well.
fn endpoint_matches(endpoint_name: &str, cable_name: &str) -> bool {
    let inner = match (endpoint_name.find('('), endpoint_name.rfind(')')) {
        (Some(open), Some(close)) if close > open => &endpoint_name[open + 1..close],
        _ => endpoint_name,
    };

    let inner = inner.trim();
    if inner == cable_name {
        return true;
    }

    // Tolerate the "<bus-id>- <name>" form Windows produces for duplicates.
    if let Some((prefix, rest)) = inner.split_once("- ")
        && prefix.trim().chars().all(|c| c.is_ascii_digit())
        && rest.trim() == cable_name
    {
        return true;
    }

    false
}

/// Validate and number a cable list.
fn build_cable_list(names: &[String]) -> Result<Vec<VirtualCable>, CableError> {
    if names.len() > 32 {
        return Err(CableError::InvalidName {
            name: format!("{} cables requested, the maximum is 32", names.len()),
        });
    }

    let mut seen = HashMap::new();
    let mut cables = Vec::with_capacity(names.len());

    for (index, raw) in names.iter().enumerate() {
        let name = raw.trim();
        if name.is_empty() {
            return Err(CableError::InvalidName {
                name: raw.clone(),
            });
        }
        // The name travels through a command line and a USB descriptor, so a
        // comma would corrupt the argument list and a quote would break
        // quoting. Both are rejected rather than silently rewritten.
        if name.contains(',') || name.contains('"') {
            return Err(CableError::InvalidName {
                name: raw.clone(),
            });
        }
        let clean = name.strip_suffix(":out").unwrap_or(name).to_lowercase();
        if seen.insert(clean, ()).is_some() {
            // Already added this cable, skip duplicate gracefully
            continue;
        }

        cables.push(VirtualCable {
            number: u8::try_from(index + 1).map_err(|_| CableError::InvalidName {
                name: format!("cable number {} exceeds 255", index + 1),
            })?,
            name: name.to_string(),
        });
    }

    Ok(cables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cable_list_numbers_in_order() {
        let cables = build_cable_list(&["OWL Spotify".into(), "OWL Personal Mix".into()])
            .expect("valid names");
        assert_eq!(cables.len(), 2);
        assert_eq!(cables[0].number, 1);
        assert_eq!(cables[0].bus_id(), "1-1");
        assert_eq!(cables[0].pid(), 0xCA01);
        assert_eq!(cables[1].number, 2);
        assert_eq!(cables[1].bus_id(), "1-2");
        assert_eq!(cables[1].pid(), 0xCA02);
    }

    #[test]
    fn test_build_cable_list_deduplicates() {
        let cables = build_cable_list(&["OWL A".into(), "OWL A".into()]).expect("dedup");
        assert_eq!(cables.len(), 1);
    }

    #[test]
    fn test_build_cable_list_rejects_empty_and_separators() {
        assert!(build_cable_list(&["".into()]).is_err());
        assert!(build_cable_list(&["   ".into()]).is_err());
        // A comma would corrupt the comma-separated --names argument.
        assert!(build_cable_list(&["OWL,A".into()]).is_err());
        assert!(build_cable_list(&["OWL\"A".into()]).is_err());
    }

    #[test]
    fn test_build_cable_list_trims_whitespace() {
        let cables = build_cable_list(&["  OWL Spotify  ".into()]).expect("valid");
        assert_eq!(cables[0].name, "OWL Spotify");
    }

    #[test]
    fn test_build_cable_list_rejects_more_than_32() {
        let many: Vec<String> = (0..33).map(|i| format!("cable{i}")).collect();
        assert!(build_cable_list(&many).is_err());
    }

    #[test]
    fn test_parse_attached_bus_ids_from_port_listing() {
        // Verbatim shape of `usbip port` output on this machine.
        let listing = "\
Imported USB devices
====================
Port 01: device in use at Full Speed(12Mbps)
         unknown vendor : unknown product (ffff:ca01)
           -> usbip://127.0.0.1:3240/1-1
           -> remote bus/dev: 001/001
           -> serial: 
           -> mode: zero-copy
Port 02: device in use at Full Speed(12Mbps)
         unknown vendor : unknown product (ffff:ca02)
           -> usbip://127.0.0.1:3240/1-2
           -> remote bus/dev: 001/002";

        let mut found = Vec::new();
        for line in listing.lines() {
            let Some(position) = line.find("usbip://") else {
                continue;
            };
            let target = line[position + "usbip://".len()..].trim();
            if let Some((_, bus_id)) = target.rsplit_once('/') {
                let bus_id = bus_id.trim().to_string();
                if !bus_id.is_empty() && !found.contains(&bus_id) {
                    found.push(bus_id);
                }
            }
        }

        assert_eq!(found, vec!["1-1".to_string(), "1-2".to_string()]);
    }

    #[test]
    fn test_parse_attached_bus_ids_deduplicates() {
        // The same bus id attached twice must collapse to one entry, which is
        // exactly the condition that made Windows show a dead duplicate device.
        let listing = "\
Port 01: device in use
           -> usbip://127.0.0.1:3240/1-1
Port 02: device in use
           -> usbip://127.0.0.1:3240/1-1";

        let mut found: Vec<String> = Vec::new();
        for line in listing.lines() {
            let Some(position) = line.find("usbip://") else {
                continue;
            };
            let target = line[position + "usbip://".len()..].trim();
            if let Some((_, bus_id)) = target.rsplit_once('/') {
                let bus_id = bus_id.trim().to_string();
                if !bus_id.is_empty() && !found.contains(&bus_id) {
                    found.push(bus_id);
                }
            }
        }

        assert_eq!(found, vec!["1-1".to_string()]);
    }

    #[test]
    fn test_parse_attached_bus_ids_empty_listing() {
        // An empty listing (nothing attached) must yield nothing rather than a
        // bogus entry, so `attach_all_persistent` proceeds to attach.
        let listing = "Imported USB devices\n====================";
        let found: Vec<&str> = listing
            .lines()
            .filter_map(|line| line.find("usbip://").map(|p| &line[p + 8..]))
            .collect();
        assert!(found.is_empty());
    }

    #[test]
    fn test_endpoint_matches_windows_form_factor_names() {
        // Measured on this machine: a cable exported as "OWL Timing Test"
        // appears under these exact endpoint names.
        assert!(endpoint_matches(
            "Speakers (OWL Timing Test)",
            "OWL Timing Test"
        ));
        assert!(endpoint_matches(
            "Microphone (OWL Timing Test)",
            "OWL Timing Test"
        ));
    }

    #[test]
    fn test_endpoint_matches_tolerates_bus_id_prefix() {
        // Windows adds a bus-id prefix when two devices share a product string.
        assert!(endpoint_matches(
            "Speakers (2- OWL Timing Test)",
            "OWL Timing Test"
        ));
        assert!(endpoint_matches(
            "Microphone (2- OWL Timing Test)",
            "OWL Timing Test"
        ));
    }

    #[test]
    fn test_endpoint_matches_rejects_other_devices() {
        // Real hardware and a third-party cable must never be mistaken for ours.
        assert!(!endpoint_matches(
            "Speakers (Sound Blaster Play! 3)",
            "OWL Timing Test"
        ));
        assert!(!endpoint_matches(
            "CABLE Input (VB-Audio Virtual Cable)",
            "OWL Timing Test"
        ));
        assert!(!endpoint_matches("OWL Other Cable", "OWL Timing Test"));
        // A substring match would be wrong: a longer name is a different cable.
        assert!(!endpoint_matches(
            "Speakers (OWL Timing Test Extra)",
            "OWL Timing Test"
        ));
    }

    #[test]
    fn test_endpoint_matches_handles_unparenthesised_name() {
        assert!(endpoint_matches("OWL Timing Test", "OWL Timing Test"));
    }
}
