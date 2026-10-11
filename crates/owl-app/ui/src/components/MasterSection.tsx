import React from "react";
import { Sliders, Volume2, VolumeX, ShieldCheck } from "lucide-react";
import { StereoVuMeter } from "./StereoVuMeter";
import { ChannelMeterDto, setMixGain, setMixMute } from "../ipc";

export interface MixBusItem {
  id: number;
  name: string;
  gain: number;
  isMuted: boolean;
}

interface MasterSectionProps {
  mixes: MixBusItem[];
  meters: ChannelMeterDto[];
  onUpdate: (updated: MixBusItem[]) => void;
}

const EMPTY_METER: ChannelMeterDto = { peak_l: 0, peak_r: 0, rms_l: 0, rms_r: 0, clipping: false };

/**
 * Master bus column strip — matte graphite token set, no neon.
 * `mixIdx` is the daemon-side `mix_idx`.
 */
export const MasterSection: React.FC<MasterSectionProps> = ({ mixes, meters, onUpdate }) => {
  const handleGainChange = (mixIdx: number, newGain: number) => {
    const updated = mixes.map((m, idx) => (idx === mixIdx ? { ...m, gain: newGain } : m));
    onUpdate(updated);
    setMixGain(mixIdx, newGain).catch(console.error);
  };

  const handleMuteToggle = (mixIdx: number) => {
    const updated = mixes.map((m, idx) => (idx === mixIdx ? { ...m, isMuted: !m.isMuted } : m));
    onUpdate(updated);
    setMixMute(mixIdx, updated[mixIdx].isMuted).catch(console.error);
  };

  return (
    <div className="bg-[#211d27] border border-white/[0.06] rounded-lg p-3 flex flex-col h-full">
      <div className="flex items-center justify-between pb-3 border-b border-white/[0.06] mb-3">
        <div className="flex items-center gap-2">
          <Sliders className="w-4 h-4 text-[#9ca3af]" />
          <span className="text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af]">
            Master buses ({mixes.length})
          </span>
        </div>
        <div className="flex items-center gap-1 text-[10px] text-[#71717a] font-mono bg-white/[0.04] border border-white/[0.06] px-1.5 py-0.5 rounded">
          <ShieldCheck className="w-3 h-3" />
          Limiter
        </div>
      </div>

      <div className="grid grid-cols-5 gap-2.5 flex-1">
        {mixes.map((mix, idx) => {
          const meter = meters[idx] || EMPTY_METER;
          const isMuted = mix.isMuted;

          return (
            <div
              key={mix.id}
              className={`flex flex-col items-center justify-between p-2 rounded-lg bg-[#191621] border transition-colors ${
                isMuted ? "border-[#ef4444]" : "border-white/[0.06]"
              }`}
            >
              <div className="flex flex-col items-center gap-1 text-center w-full">
                <div className="p-1.5 rounded-md bg-[#211d27] border border-white/[0.06]">
                  <Sliders className="w-4 h-4 text-[#9ca3af]" />
                </div>
                <span className="text-xs font-semibold text-[#e4e4e7] truncate w-full">
                  {mix.name}
                </span>
                <span className="text-[10px] font-mono text-[#71717a]">
                  {Math.round(mix.gain * 100)}%
                </span>
              </div>

              <div className="flex items-center justify-center gap-2 my-2 h-44">
                <input
                  type="range"
                  min="0"
                  max="1.0"
                  step="0.01"
                  value={mix.gain}
                  onChange={(e) => handleGainChange(idx, parseFloat(e.target.value))}
                  className="wl-slider wl-slider-vertical h-36"
                  title={`${mix.name} volume`}
                />

                <StereoVuMeter
                  peakL={isMuted ? 0 : meter.peak_l}
                  peakR={isMuted ? 0 : meter.peak_r}
                  clipping={meter.clipping}
                  height={150}
                  width={14}
                  showLabels={idx === 0}
                />
              </div>

              <button
                type="button"
                onClick={() => handleMuteToggle(idx)}
                className={`w-full py-1 rounded-md text-xs font-medium flex items-center justify-center gap-1 transition-colors border ${
                  isMuted
                    ? "bg-[#3a1c24] text-[#ef4444] border-[#ef4444]"
                    : "bg-[#211d27] text-[#71717a] border-white/[0.06] hover:text-[#e4e4e7]"
                }`}
              >
                {isMuted ? <VolumeX className="w-3.5 h-3.5" /> : <Volume2 className="w-3.5 h-3.5" />}
                <span>{isMuted ? "MUTED" : "MUTE"}</span>
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
};
