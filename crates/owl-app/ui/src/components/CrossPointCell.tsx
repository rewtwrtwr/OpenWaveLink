import React, { useState, useEffect, useRef } from "react";
import { Plus, Volume2, VolumeX, X } from "lucide-react";
import { CrossPointState } from "../types";

interface CrossPointCellProps {
  channelId: string;
  mixId: string;
  channelName: string;
  mixName: string;
  channelMasterMuted: boolean;
  meterLevel?: number; // 0.0 .. 1.0 (audio volume level for VU bar)
  state?: CrossPointState;
  onActivate: () => void;
  onDeactivate: () => void;
  onGainChange: (gain: number) => void;
  onMuteToggle: () => void;
}

/**
 * Matrix cross-point (Wave Link 3.0 style, 72px height).
 *
 * Empty  : `#191621`, clean matte dark card with (+) button on hover.
 * Active : `#272230` card with red-bordered Mute button, wide fader with green VU meter bar.
 * Muted  : `#3a1c24` with `#ef4444` border.
 */
export const CrossPointCell: React.FC<CrossPointCellProps> = ({
  channelName,
  mixName,
  channelMasterMuted,
  meterLevel = 0,
  state,
  onActivate,
  onDeactivate,
  onGainChange,
  onMuteToggle,
}) => {
  const isActive = Boolean(state?.active);
  const isMuted = Boolean(state?.isMuted);
  const gain = state?.gain ?? 1.0;

  // Smooth VU level
  const [smoothLevel, setSmoothLevel] = useState(0);
  const lastTimeRef = useRef(performance.now());

  useEffect(() => {
    if (!isActive || isMuted || channelMasterMuted) {
      setSmoothLevel(0);
      return;
    }
    const effective = meterLevel * gain;
    const now = performance.now();
    const dt = Math.min((now - lastTimeRef.current) / 1000, 0.1);
    lastTimeRef.current = now;

    setSmoothLevel((prev) => (effective >= prev ? effective : Math.max(0, prev - dt * 2.5)));
  }, [meterLevel, gain, isActive, isMuted, channelMasterMuted]);

  // 1. Inactive cross-point cell
  if (!isActive) {
    return (
      <div
        className="group relative h-[72px] w-full bg-[#151518] hover:bg-[#1a1a1d] border border-white/[0.06] rounded-xl flex items-center justify-center transition-colors cursor-pointer"
        onClick={onActivate}
        title={`Add ${channelName} to ${mixName}`}
      >
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onActivate();
          }}
          className="opacity-0 group-hover:opacity-100 transition-all duration-150 w-8 h-8 rounded-full bg-[#242429] hover:bg-[#303036] text-[#a1a1aa] hover:text-[#f4f4f5] flex items-center justify-center border border-white/[0.08]"
          title="Add channel to mix"
        >
          <Plus className="w-4 h-4" />
        </button>
      </div>
    );
  }

  const vuPct = Math.min(100, Math.max(0, smoothLevel * 100));

  // 2. Active cross-point cell
  return (
    <div
      className={`group relative h-[72px] w-full px-3 py-2 rounded-xl border transition-colors flex items-center justify-between gap-2.5 ${
        isMuted
          ? "bg-[#2e1417] border-[#ef4444]/60"
          : "bg-[#1f1f23] hover:bg-[#252529] border-white/[0.06]"
      }`}
      onContextMenu={(e) => {
        e.preventDefault();
        onDeactivate();
      }}
      title={`${channelName} → ${mixName} (right-click to remove)`}
    >
      {/* Quick remove cross in top-right corner on hover */}
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onDeactivate();
        }}
        className="absolute top-1.5 right-1.5 opacity-0 group-hover:opacity-100 transition-opacity w-4 h-4 rounded hover:bg-red-500/20 text-[#71717a] hover:text-[#ef4444] flex items-center justify-center z-10"
        title={`Remove from ${mixName}`}
      >
        <X className="w-3 h-3" />
      </button>

      {/* Per-mix mute button (Wave Link red box style) */}
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onMuteToggle();
        }}
        className={`w-8 h-8 rounded-lg flex items-center justify-center shrink-0 transition-colors border ${
          isMuted
            ? "bg-[#331418] border-[#ef4444] text-[#ef4444]"
            : "bg-[#18181b] hover:bg-[#222226] border-white/[0.08] text-[#a1a1aa] hover:text-[#f4f4f5]"
        }`}
        title={isMuted ? `Unmute in ${mixName}` : `Mute in ${mixName}`}
      >
        {isMuted ? <VolumeX className="w-4 h-4" /> : <Volume2 className="w-4 h-4" />}
      </button>

      {/* Send gain slider with embedded green VU bar */}
      <div className="flex-1 flex flex-col justify-center min-w-0 pr-1">
        <div className="flex items-center justify-between text-[10px] font-mono leading-none mb-1.5">
          <span className="text-[#a1a1aa] font-medium truncate max-w-[90px]" title={mixName}>
            {mixName}
          </span>
          <span className={isMuted ? "text-[#ef4444] font-semibold" : "text-[#d4d4d8]"}>
            {isMuted ? "MUTED" : `${Math.round(gain * 100)}%`}
          </span>
        </div>

        {/* Fader track container */}
        <div className="relative w-full h-4 flex items-center">
          {/* Background track */}
          <div className="absolute inset-0 bg-[#141416] rounded-full overflow-hidden border border-white/[0.06]">
            {/* Green volume meter fill */}
            <div
              className="h-full rounded-full transition-[width] duration-75 ease-out"
              style={{
                width: `${vuPct}%`,
                backgroundColor: vuPct >= 96 ? "#ef4444" : vuPct >= 80 ? "#eab308" : "#22c55e",
              }}
            />
          </div>

          {/* Interactive input slider overlaid transparently */}
          <input
            type="range"
            min="0"
            max="1.5"
            step="0.01"
            value={gain}
            disabled={channelMasterMuted || isMuted}
            onChange={(e) => onGainChange(parseFloat(e.target.value))}
            className="wl-slider w-full cursor-pointer relative z-10 !bg-transparent"
            title={`Send gain: ${Math.round(gain * 100)}%`}
          />
        </div>
      </div>
    </div>
  );
};
