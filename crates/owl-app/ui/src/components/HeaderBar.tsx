import React, { useState, useEffect, useRef } from "react";
import { Menu, ChevronDown, Headphones, Speaker, RotateCcw } from "lucide-react";
import { MixBus } from "../types";
import { AudioDeviceDto, getSystemAudioDevices, setOutputDevice } from "../ipc";
import { isVirtualDevice, pickPreferredOutput } from "./deviceUtils";

interface HeaderBarProps {
  mixes: MixBus[];
  listeningMixId: string;
  onSelectListeningMix: (mixId: string) => void;
  /** Resolved, human-readable name of the endpoint the backend actually bound. */
  monitorDevice: string;
  /** Fired after the backend confirmed the switch, with the name it resolved. */
  onMonitorDeviceResolved: (endpointId: string, resolvedName: string) => void;
  daemonConnected: boolean;
  fps: number;
  onResetDefaults?: () => void;
}

/**
 * Top control bar: burger menu, brand + `Mixes` heading, and the monitoring
 * panel (`Listening to` mix selector, `on <device>` endpoint selector).
 *
 * The `on <device>` selector performs a REAL backend call —
 * `setOutputDevice(outputs[].id)` with the IMMDeviceEnumerator endpoint id —
 * and then displays the name the backend resolved. It is never local state.
 *
 * Per contract §7 the system default here is a virtual cable, so the initial
 * pick prefers real hardware and virtual endpoints are labelled `virtual`.
 */
export const HeaderBar: React.FC<HeaderBarProps> = ({
  mixes,
  listeningMixId,
  onSelectListeningMix,
  monitorDevice,
  onMonitorDeviceResolved,
  daemonConnected,
  fps,
  onResetDefaults,
}) => {
  const [showMixMenu, setShowMixMenu] = useState(false);
  const [showDeviceMenu, setShowDeviceMenu] = useState(false);
  const [showAppMenu, setShowAppMenu] = useState(false);
  const [outputs, setOutputs] = useState<AudioDeviceDto[]>([]);
  const [isSwitchingDevice, setIsSwitchingDevice] = useState(false);
  const [deviceError, setDeviceError] = useState<string | null>(null);
  // Persisted in localStorage so a reload does not re-bind a device the user
  // already chose, and so the auto-bind runs at most once per browser profile.
  const userPickedRef = useRef<boolean>(
    typeof window !== "undefined" &&
      window.localStorage.getItem("openwavelink_device_picked") === "1"
  );
  const autoBindAttemptedRef = useRef(false);
  const containerRef = useRef<HTMLElement>(null);

  // Real IMMDeviceEnumerator(eRender) endpoints.
  //
  // Retried on failure: this races the daemon at startup, and an empty `outputs` would silently
  // disable the auto-bind below — leaving the backend on the virtual-cable default forever.
  useEffect(() => {
    let attempts = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cancelled = false;

    const load = () => {
      if (cancelled) return;
      getSystemAudioDevices()
        .then((res) => {
          if (cancelled) return;
          if (res?.outputs?.length) {
            setOutputs(res.outputs);
            return;
          }
          if (attempts < 20) {
            attempts += 1;
            timer = setTimeout(load, 500);
          }
        })
        .catch(() => {
          if (cancelled || attempts >= 20) return;
          attempts += 1;
          timer = setTimeout(load, 500);
        });
    };

    load();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, []);

  // Close every popover on outside click.
  useEffect(() => {
    const handleOutsideClick = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setShowMixMenu(false);
        setShowDeviceMenu(false);
        setShowAppMenu(false);
      }
    };
    document.addEventListener("mousedown", handleOutsideClick);
    return () => document.removeEventListener("mousedown", handleOutsideClick);
  }, []);

  const activeMix = mixes.find((m) => m.id === listeningMixId) || mixes[0];

  // Bind the backend to a real endpoint on first run (never the virtual default
  // when real hardware exists). Skipped once a device is already bound.
  const preferred = pickPreferredOutput(outputs);
  const preferredId = preferred?.id;
  const preferredIsPhysical = preferred ? !isVirtualDevice(preferred.name) : false;

  useEffect(() => {
    if (!preferredId || userPickedRef.current) return;
    // Nothing bound yet, or the backend is sitting on a virtual cable while
    // real hardware exists (contract §7: the default here IS a cable).
    const needsBind = !monitorDevice || (preferredIsPhysical && isVirtualDevice(monitorDevice));
    if (!needsBind) return;
    // One automatic attempt only — never fight the backend in a loop.
    if (autoBindAttemptedRef.current) return;
    autoBindAttemptedRef.current = true;

    // Deliberately not cancelled on cleanup: this reports to the parent, and
    // under StrictMode's double-mount the second run is already skipped above.
    setOutputDevice(preferredId)
      .then((resolvedName) => {
        onMonitorDeviceResolved(preferredId, resolvedName || preferred?.name || "");
      })
      .catch((err) => {
        console.error("Initial setOutputDevice failed:", err);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preferredId, preferredIsPhysical, monitorDevice]);

  const handleSelectDevice = async (device: AudioDeviceDto) => {
    setShowDeviceMenu(false);
    setDeviceError(null);
    userPickedRef.current = true;
    try {
      window.localStorage.setItem("openwavelink_device_picked", "1");
    } catch {
      /* storage unavailable — the in-memory flag still guards this session */
    }
    setIsSwitchingDevice(true);
    try {
      const resolvedName = await setOutputDevice(device.id);
      // The name the daemon actually bound wins over anything we had locally.
      onMonitorDeviceResolved(device.id, resolvedName || device.name);
    } catch (err) {
      console.error("Failed to switch output device:", err);
      setDeviceError("Switch failed");
    } finally {
      setIsSwitchingDevice(false);
    }
  };

  return (
    <header
      ref={containerRef}
      className="flex items-center justify-between px-3 py-2.5 border-b border-white/[0.06] bg-[#1a1720] shrink-0 select-none text-[#e4e4e7]"
    >
      {/* 1. Left: burger menu, brand, "Mixes" heading */}
      <div className="flex items-center gap-4">
        <div className="relative">
          <button
            type="button"
            onClick={() => {
              setShowAppMenu(!showAppMenu);
              setShowMixMenu(false);
              setShowDeviceMenu(false);
            }}
            className="w-8 h-8 rounded-md bg-[#211d27] hover:bg-[#272230] border border-white/[0.06] flex items-center justify-center text-[#9ca3af] hover:text-[#e4e4e7] transition-colors"
            title="App menu"
          >
            <Menu className="w-4 h-4" />
          </button>

          {showAppMenu && (
            <div className="absolute top-10 left-0 z-50 w-56 bg-[#201c2b] border border-white/[0.08] rounded-lg shadow-xl shadow-black/60 py-1.5 text-xs text-[#9ca3af]">
              <div className="px-3 py-1.5 text-[10px] font-semibold uppercase tracking-wider text-[#71717a] border-b border-white/[0.06] mb-1">
                OpenWaveLink • Windows 11
              </div>
              <div className="px-3 py-1.5 flex items-center justify-between">
                <span>Core daemon</span>
                <span className="text-[#e4e4e7] font-mono">
                  {daemonConnected ? "Connected" : "Offline"}
                </span>
              </div>
              <div className="px-3 py-1.5 flex items-center justify-between">
                <span>Telemetry</span>
                <span className="text-[#e4e4e7] font-mono">{fps} FPS</span>
              </div>
              {onResetDefaults && (
                <>
                  <div className="h-px bg-white/[0.06] my-1" />
                  <button
                    type="button"
                    onClick={() => {
                      setShowAppMenu(false);
                      onResetDefaults();
                    }}
                    className="w-full text-left px-3 py-2 hover:bg-white/[0.04] flex items-center gap-2 text-[#9ca3af] hover:text-[#e4e4e7] transition-colors"
                    title="Reset the matrix to System + Personal Mix"
                  >
                    <RotateCcw className="w-3.5 h-3.5" />
                    <span>Reset to default topology</span>
                  </button>
                </>
              )}
            </div>
          )}
        </div>

        <div className="flex items-center gap-3">
          <div className="w-7 h-7 rounded-md bg-[#1c1c20] border border-white/[0.08] flex items-center justify-center text-[#ffffff]">
            <span className="font-black text-xs tracking-tighter">|||</span>
          </div>

          <div className="flex items-baseline gap-2">
            <span className="text-xs font-medium text-[#71717a] hidden sm:inline">
              OpenWaveLink
            </span>
            <span className="text-sm font-semibold text-[#f4f4f5] tracking-tight">Mixes</span>
          </div>
        </div>
      </div>

      {/* 2. Right: monitoring controls */}
      <div className="flex items-center gap-3">
        {/* Listening to */}
        <div className="relative flex items-center gap-1.5 text-xs">
          <span className="text-[#71717a] font-medium hidden md:inline">Listening to</span>
          <button
            type="button"
            onClick={() => {
              setShowMixMenu(!showMixMenu);
              setShowDeviceMenu(false);
              setShowAppMenu(false);
            }}
            className="flex items-center gap-2 px-2.5 py-1 rounded-md bg-[#18181b] hover:bg-[#222226] border border-white/[0.08] text-[#f4f4f5] font-medium transition-colors"
          >
            <Headphones className="w-3.5 h-3.5 text-[#a1a1aa]" />
            <span className="truncate max-w-[120px]">{activeMix?.name || "Personal Mix"}</span>
            <ChevronDown className="w-3 h-3 text-[#71717a]" />
          </button>

          {showMixMenu && (
            <div className="absolute right-0 top-9 z-50 w-52 bg-[#1c1c20] border border-white/[0.08] rounded-lg shadow-xl shadow-black/60 py-1 text-xs">
              <div className="px-3 py-1 text-[10px] font-semibold uppercase tracking-wider text-[#71717a]">
                Select monitor mix
              </div>
              {mixes.map((m) => (
                <button
                  key={m.id}
                  type="button"
                  onClick={() => {
                    onSelectListeningMix(m.id);
                    setShowMixMenu(false);
                  }}
                  className={`w-full text-left px-3 py-1.5 hover:bg-white/[0.04] flex items-center justify-between transition-colors ${
                    m.id === listeningMixId ? "text-[#f4f4f5] bg-white/[0.04]" : "text-[#a1a1aa]"
                  }`}
                >
                  <span className="truncate">{m.name}</span>
                  {m.id === listeningMixId && (
                    <span className="w-1.5 h-1.5 rounded-full bg-[#38bdf8]" />
                  )}
                </button>
              ))}
            </div>
          )}
        </div>

        {/* on <device> — real endpoint switch */}
        <div className="relative flex items-center gap-1.5 text-xs">
          <span className="text-[#71717a] font-medium hidden lg:inline">on</span>
          <button
            type="button"
            onClick={() => {
              setShowDeviceMenu(!showDeviceMenu);
              setShowMixMenu(false);
              setShowAppMenu(false);
            }}
            disabled={isSwitchingDevice}
            className="flex items-center gap-2 px-2.5 py-1 rounded-md bg-[#18181b] hover:bg-[#222226] border border-white/[0.08] text-[#a1a1aa] hover:text-[#f4f4f5] font-medium transition-colors disabled:opacity-60"
            title={
              monitorDevice
                ? `Bound endpoint: ${monitorDevice}`
                : "Windows 11 output endpoint"
            }
          >
            <Speaker className="w-3.5 h-3.5 text-[#71717a]" />
            <span className="truncate max-w-[140px]">
              {monitorDevice || "Select output device"}
            </span>
            <ChevronDown className="w-3 h-3 text-[#71717a]" />
          </button>

          {showDeviceMenu && (
            <div className="absolute right-0 top-9 z-50 w-80 bg-[#1c1c20] border border-white/[0.08] rounded-lg shadow-xl shadow-black/60 py-1 text-xs">
              <div className="px-3 py-1 text-[10px] font-semibold uppercase tracking-wider text-[#71717a]">
                Windows 11 output device
              </div>
              {deviceError && (
                <div className="px-3 py-1.5 text-[#ef4444]">{deviceError}</div>
              )}
              {outputs.length === 0 && (
                <div className="px-3 py-2 text-[#71717a]">No output endpoints detected.</div>
              )}
              {outputs.map((dev) => {
                const virtual = isVirtualDevice(dev.name);
                const bound = dev.name === monitorDevice;
                return (
                  <button
                    key={dev.id}
                    type="button"
                    onClick={() => handleSelectDevice(dev)}
                    className={`w-full text-left px-3 py-1.5 hover:bg-white/[0.04] flex items-center justify-between gap-2 transition-colors ${
                      bound ? "text-[#e4e4e7] bg-white/[0.03]" : "text-[#9ca3af]"
                    }`}
                    title={dev.id}
                  >
                    <span className="truncate">{dev.name}</span>
                    <span className="flex items-center gap-2 shrink-0">
                      {virtual && (
                        <span className="text-[10px] font-mono text-[#71717a]">virtual</span>
                      )}
                      {bound && <span className="w-1.5 h-1.5 rounded-full bg-[#c084fc]" />}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </header>
  );
};
