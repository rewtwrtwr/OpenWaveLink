import React, { useState, useEffect } from "react";
import { SlidersHorizontal, Mic, MicOff, Headphones, Radio, ChevronDown, Maximize2 } from "lucide-react";
import { StereoVuMeter } from "./StereoVuMeter";
import {
  ChannelMeterDto,
  TelemetrySnapshotDto,
  EngineStateDto,
  AudioDeviceDto,
  setMixGain,
  setMixMute,
  setChannelMute,
  getEngineState,
  getSystemAudioDevices,
  listenEvent,
} from "../ipc";

const EMPTY_METER: ChannelMeterDto = {
  peak_l: 0,
  peak_r: 0,
  rms_l: 0,
  rms_r: 0,
  clipping: false,
};

/**
 * Taskbar quick mixer.
 *
 * Every value comes from the engine — no hardcoded device names, no assumed
 * channel/mix indices. Mixes are addressed by the daemon's `mix_idx`, and the
 * mic toggle targets the real numeric channel id of the microphone channel.
 */
export const FlyoutView: React.FC = () => {
  const [engine, setEngine] = useState<EngineStateDto | null>(null);
  const [outputs, setOutputs] = useState<AudioDeviceDto[]>([]);
  const [micMuted, setMicMuted] = useState<boolean>(false);
  const [mixMeters, setMixMeters] = useState<ChannelMeterDto[]>([]);
  const [showDeviceMenu, setShowDeviceMenu] = useState(false);

  // Real engine topology (channels with numeric ids, mixes with mix_idx).
  useEffect(() => {
    getEngineState()
      .then((state) => {
        if (state) setEngine(state);
      })
      .catch(() => {});
    getSystemAudioDevices()
      .then((res) => setOutputs(res?.outputs || []))
      .catch(() => {});
  }, []);

  useEffect(() => {
    let unlistenVu: (() => void) | undefined;
    listenEvent<TelemetrySnapshotDto>("vu-meter-frame", (frame) => {
      setMixMeters(frame.mixes || []);
    }).then((un) => {
      unlistenVu = un;
    });
    return () => {
      unlistenVu?.();
    };
  }, []);

  const mixes = engine?.mixes || [];
  const micChannel = (engine?.channels || []).find(
    (c) => c.channel_type === "Microphone"
  );

  const handleMicToggle = () => {
    if (!micChannel) return;
    const next = !micMuted;
    setMicMuted(next);
    setChannelMute(micChannel.id, next).catch(console.error);
  };

  const handleMixGain = (mixIdx: number, value: number) => {
    setEngine((prev) =>
      prev
        ? {
            ...prev,
            mixes: prev.mixes.map((m) => (m.id === mixIdx ? { ...m, gain: value } : m)),
          }
        : prev
    );
    setMixGain(mixIdx, value).catch(console.error);
  };

  const handleMixMute = (mixIdx: number, muted: boolean) => {
    setEngine((prev) =>
      prev
        ? {
            ...prev,
            mixes: prev.mixes.map((m) => (m.id === mixIdx ? { ...m, is_muted: muted } : m)),
          }
        : prev
    );
    setMixMute(mixIdx, muted).catch(console.error);
  };

  const openMainWindow = async () => {
    if (typeof window !== "undefined" && window.__TAURI_INTERNALS__) {
      const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const current = getCurrentWebviewWindow();
      await current.hide();
    }
  };

  return (
    <div className="w-[360px] h-[480px] bg-[#16141a] border border-white/[0.08] text-[#e4e4e7] flex flex-col p-4 rounded-xl select-none font-sans overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between pb-3 border-b border-white/[0.06]">
        <div className="flex items-center gap-2">
          <div className="w-6 h-6 rounded-md bg-[#262130] border border-white/[0.06] flex items-center justify-center">
            <SlidersHorizontal className="w-3.5 h-3.5 text-[#9ca3af]" />
          </div>
          <div>
            <span className="text-xs font-semibold text-[#e4e4e7] leading-none block">
              OpenWaveLink
            </span>
            <span className="text-[10px] text-[#71717a] leading-none">Quick mixer</span>
          </div>
        </div>

        <button
          type="button"
          onClick={openMainWindow}
          className="p-1.5 rounded-md bg-[#211d27] hover:bg-[#272230] text-[#71717a] hover:text-[#e4e4e7] border border-white/[0.06] transition-colors"
          title="Open the full routing matrix"
        >
          <Maximize2 className="w-3.5 h-3.5" />
        </button>
      </div>

      {/* Mic mute */}
      <div className="my-3">
        <button
          type="button"
          onClick={handleMicToggle}
          disabled={!micChannel}
          className={`w-full py-2.5 px-3 rounded-md border flex items-center justify-between transition-colors disabled:opacity-50 ${
            micMuted
              ? "bg-[#3a1c24] border-[#ef4444]"
              : "bg-[#211d27] border-white/[0.06] hover:border-white/[0.12]"
          }`}
        >
          <div className="flex items-center gap-2.5 min-w-0">
            <div
              className={`p-1.5 rounded-md border ${
                micMuted
                  ? "bg-[#3a1c24] border-[#ef4444] text-[#ef4444]"
                  : "bg-[#191621] border-white/[0.06] text-[#9ca3af]"
              }`}
            >
              {micMuted ? <MicOff className="w-4 h-4" /> : <Mic className="w-4 h-4" />}
            </div>
            <div className="text-left min-w-0">
              <span className="text-xs font-semibold block text-[#e4e4e7] truncate">
                {micChannel ? micChannel.name : "No microphone channel"}
              </span>
              <span className="text-[10px] text-[#71717a] font-mono">
                {micMuted ? "Muted" : "Live"}
              </span>
            </div>
          </div>

          <span
            className={`text-[10px] font-mono px-2 py-0.5 rounded border shrink-0 ${
              micMuted
                ? "bg-[#3a1c24] border-[#ef4444] text-[#ef4444]"
                : "bg-[#191621] border-white/[0.06] text-[#71717a]"
            }`}
          >
            {micMuted ? "MUTED" : "ACTIVE"}
          </span>
        </button>
      </div>

      {/* Bound output endpoint */}
      <div className="mb-3">
        <label className="text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af] block mb-1">
          Monitor output
        </label>
        <button
          type="button"
          onClick={() => setShowDeviceMenu(!showDeviceMenu)}
          className="w-full flex items-center justify-between p-2 rounded-md bg-[#211d27] border border-white/[0.06] text-xs text-[#9ca3af] hover:text-[#e4e4e7] transition-colors"
          title="The endpoint the engine is actually bound to"
        >
          <div className="flex items-center gap-2 truncate">
            <Headphones className="w-3.5 h-3.5 text-[#71717a] shrink-0" />
            <span className="truncate">
              {engine?.output_device_name || "No endpoint bound"}
            </span>
          </div>
          <ChevronDown className="w-3.5 h-3.5 text-[#71717a] shrink-0" />
        </button>
        {showDeviceMenu && (
          <div className="mt-1 w-full bg-[#201c2b] border border-white/[0.08] rounded-lg py-1 text-xs max-h-40 overflow-y-auto">
            <div className="px-3 py-1 text-[10px] font-semibold uppercase tracking-wider text-[#71717a]">
              Detected outputs ({outputs.length})
            </div>
            {outputs.map((dev) => (
              <div
                key={dev.id}
                className="px-3 py-1.5 text-[#71717a] truncate"
                title={dev.id}
              >
                {dev.name}
              </div>
            ))}
            <div className="px-3 py-1 text-[10px] text-[#71717a] border-t border-white/[0.06] mt-1">
              Change the endpoint in the main window.
            </div>
          </div>
        )}
      </div>

      {/* Mixes */}
      <div className="space-y-3 flex-1 overflow-y-auto pr-1">
        {mixes.length === 0 && (
          <div className="p-3 text-center text-xs text-[#71717a] border border-dashed border-white/[0.08] rounded-md">
            No mixes reported by the engine.
          </div>
        )}
        {mixes.map((mix) => {
          const meter = mixMeters[mix.id] || EMPTY_METER;
          return (
            <div
              key={mix.id}
              className="p-2.5 rounded-md bg-[#211d27] border border-white/[0.06] space-y-1.5"
            >
              <div className="flex items-center justify-between text-xs">
                <div className="flex items-center gap-1.5 font-semibold text-[#9ca3af] min-w-0">
                  {mix.id === 0 ? (
                    <Headphones className="w-3.5 h-3.5 shrink-0" />
                  ) : (
                    <Radio className="w-3.5 h-3.5 shrink-0" />
                  )}
                  <span className="truncate">{mix.name}</span>
                </div>
                <span className="font-mono text-[#71717a] shrink-0">
                  {Math.round(mix.gain * 100)}%
                </span>
              </div>
              <input
                type="range"
                min="0"
                max="1.0"
                step="0.01"
                value={mix.gain}
                onChange={(e) => handleMixGain(mix.id, parseFloat(e.target.value))}
                className="wl-slider w-full cursor-pointer"
                title={`${mix.name} volume`}
              />
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => handleMixMute(mix.id, !mix.is_muted)}
                  className={`px-1.5 py-0.5 rounded border text-[10px] font-mono transition-colors ${
                    mix.is_muted
                      ? "bg-[#3a1c24] border-[#ef4444] text-[#ef4444]"
                      : "bg-[#191621] border-white/[0.06] text-[#71717a] hover:text-[#e4e4e7]"
                  }`}
                  title={mix.is_muted ? "Unmute mix" : "Mute mix"}
                >
                  {mix.is_muted ? "MUTED" : "MUTE"}
                </button>
                <div className="flex-1">
                  <StereoVuMeter
                    peakL={meter.peak_l}
                    peakR={meter.peak_r}
                    clipping={meter.clipping}
                    orientation="horizontal"
                  />
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
};
