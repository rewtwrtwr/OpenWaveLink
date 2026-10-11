import React, { useState, useEffect } from "react";
import { Mic, Volume2, VolumeX, ShieldAlert, AppWindow } from "lucide-react";
import { StereoVuMeter } from "./StereoVuMeter";
import { ChannelMeterDto, setChannelGain, setChannelMute, setSendGain, setSendMute, getAppIcon } from "../ipc";

export interface SendState {
  mixId: number;
  name: string;
  gain: number;
  isMuted: boolean;
}

export interface ChannelItem {
  id: number;
  name: string;
  channelType: "HardwareInput" | "ProcessLoopback" | "VirtualCable";
  pid: number;
  gain: number;
  isMuted: boolean;
  sends: SendState[];
  iconUrl?: string;
}

interface ChannelRowProps {
  channel: ChannelItem;
  meter?: ChannelMeterDto;
  onUpdate: (updated: ChannelItem) => void;
}

/**
 * Single input channel row — matte graphite Wave Link 3.0 style, no neon.
 */
export const ChannelRow: React.FC<ChannelRowProps> = ({ channel, meter, onUpdate }) => {
  const [appIcon, setAppIcon] = useState<string | null>(channel.iconUrl || null);

  useEffect(() => {
    if (!appIcon && channel.pid > 0) {
      getAppIcon(channel.pid, channel.name)
        .then((icon) => {
          if (icon) setAppIcon(icon);
        })
        .catch(() => {});
    }
  }, [channel.pid, channel.name, appIcon]);

  const getIcon = () => {
    if (appIcon) {
      return <img src={appIcon} className="w-5 h-5 object-contain rounded" alt="" />;
    }
    if (channel.channelType === "HardwareInput") return <Mic className="w-4 h-4 text-[#9ca3af]" />;
    if (channel.channelType === "ProcessLoopback") return <AppWindow className="w-4 h-4 text-[#9ca3af]" />;
    return <Volume2 className="w-4 h-4 text-[#9ca3af]" />;
  };

  const isClipping = meter?.clipping || false;

  const handleGainChange = (newGain: number) => {
    onUpdate({ ...channel, gain: newGain });
    setChannelGain(channel.id, newGain).catch(console.error);
  };

  const handleMuteToggle = () => {
    const newMuted = !channel.isMuted;
    onUpdate({ ...channel, isMuted: newMuted });
    setChannelMute(channel.id, newMuted).catch(console.error);
  };

  const handleSendGainChange = (mixIdx: number, newGain: number) => {
    const updatedSends = channel.sends.map((s, idx) =>
      idx === mixIdx ? { ...s, gain: newGain } : s
    );
    onUpdate({ ...channel, sends: updatedSends });
    setSendGain(channel.id, mixIdx, newGain).catch(console.error);
  };

  const handleSendMuteToggle = (mixIdx: number) => {
    const updatedSends = channel.sends.map((s, idx) =>
      idx === mixIdx ? { ...s, isMuted: !s.isMuted } : s
    );
    onUpdate({ ...channel, sends: updatedSends });
    const targetMute = updatedSends[mixIdx].isMuted;
    setSendMute(channel.id, mixIdx, targetMute).catch(console.error);
  };

  return (
    <div className="grid grid-cols-12 gap-2 p-2.5 bg-[#211d27] hover:bg-[#272230] rounded-xl border border-white/[0.06] items-center transition-colors">
      {/* Channel info & pre-gain */}
      <div className="col-span-4 flex items-center gap-3">
        <div className="w-9 h-9 rounded-lg bg-[#191621] border border-white/[0.06] flex items-center justify-center shrink-0 overflow-hidden">
          {getIcon()}
        </div>

        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-sm font-semibold text-[#f3f4f6] truncate">{channel.name}</span>
            {isClipping && (
              <span className="flex items-center gap-0.5 px-1 py-0.5 text-[10px] font-mono bg-[#3a1c24] text-[#ef4444] border border-[#ef4444] rounded">
                <ShieldAlert className="w-3 h-3" />
                CLIP
              </span>
            )}
          </div>
          <div className="text-[11px] text-[#71717a] flex items-center gap-2">
            <span>
              {channel.channelType === "HardwareInput" ? "Mic / device" : `PID: ${channel.pid}`}
            </span>
            <span>•</span>
            <span className="font-mono">{Math.round(channel.gain * 100)}%</span>
          </div>
        </div>

        <div className="flex items-center gap-2 shrink-0">
          <input
            type="range"
            min="0"
            max="1.5"
            step="0.01"
            value={channel.gain}
            onChange={(e) => handleGainChange(parseFloat(e.target.value))}
            className="wl-slider w-24 cursor-pointer"
            title={`Master gain: ${Math.round(channel.gain * 100)}%`}
          />

          <button
            type="button"
            onClick={handleMuteToggle}
            className={`w-8 h-8 rounded-lg border transition-colors flex items-center justify-center ${
              channel.isMuted
                ? "bg-[#3a1c24] border-[#ef4444] text-[#ef4444]"
                : "bg-[#191621] border-white/[0.06] text-[#9ca3af] hover:text-[#e4e4e7]"
            }`}
            title={channel.isMuted ? "Unmute channel" : "Mute channel"}
          >
            {channel.isMuted ? <VolumeX className="w-4 h-4" /> : <Volume2 className="w-4 h-4" />}
          </button>

          <div className="h-9 flex items-center">
            <StereoVuMeter
              peakL={channel.isMuted ? 0 : meter?.peak_l || 0}
              peakR={channel.isMuted ? 0 : meter?.peak_r || 0}
              clipping={isClipping}
              height={36}
              width={10}
            />
          </div>
        </div>
      </div>

      {/* Sends matrix */}
      <div className="col-span-8 grid grid-cols-5 gap-2 px-1">
        {channel.sends.map((send, mixIdx) => (
          <div
            key={send.mixId}
            className={`flex items-center justify-between p-2 rounded-lg bg-[#191621] border transition-colors ${
              send.isMuted ? "border-[#ef4444]" : "border-white/[0.06]"
            }`}
          >
            <div className="flex flex-col flex-1 min-w-0 pr-1.5">
              <span className="text-[10px] font-mono text-[#71717a] truncate">{send.name}</span>
              <input
                type="range"
                min="0"
                max="1.0"
                step="0.01"
                value={send.gain}
                disabled={channel.isMuted}
                onChange={(e) => handleSendGainChange(mixIdx, parseFloat(e.target.value))}
                className="wl-slider w-full cursor-pointer mt-0.5"
                title={`Send to ${send.name}: ${Math.round(send.gain * 100)}%`}
              />
            </div>

            <div className="flex flex-col items-end">
              <span className="text-[9px] font-mono text-[#71717a]">
                {Math.round(send.gain * 100)}%
              </span>
              <button
                type="button"
                onClick={() => handleSendMuteToggle(mixIdx)}
                className={`text-[9px] font-mono px-1 py-0.5 rounded border transition-colors mt-0.5 ${
                  send.isMuted
                    ? "bg-[#3a1c24] text-[#ef4444] border-[#ef4444]"
                    : "bg-[#191621] text-[#71717a] border-white/[0.06] hover:text-[#e4e4e7]"
                }`}
                title={send.isMuted ? `Unmute ${send.name}` : `Mute ${send.name}`}
              >
                M
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
};
