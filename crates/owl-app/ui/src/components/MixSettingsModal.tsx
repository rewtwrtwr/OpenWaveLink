import React, { useState, useEffect } from "react";
import { X, Headphones, Radio, Mic, Sliders, Trash2, Plus, Volume2, VolumeX, Speaker } from "lucide-react";
import { MixBus } from "../types";
import { AudioDeviceDto, getSystemAudioDevices } from "../ipc";

interface MixSettingsModalProps {
  isOpen: boolean;
  mix: MixBus | null;
  canDelete: boolean;
  availableOutputs?: AudioDeviceDto[];
  onClose: () => void;
  onUpdateMix: (updated: MixBus) => void;
  onDeleteMix: (mixId: string) => void;
}

/**
 * Mix settings modal (spec §3.4) — restyled to the matte graphite token set.
 * Outputs are real Windows 11 render endpoints, never a hardcoded list.
 */
export const MixSettingsModal: React.FC<MixSettingsModalProps> = ({
  isOpen,
  mix,
  canDelete,
  availableOutputs,
  onClose,
  onUpdateMix,
  onDeleteMix,
}) => {
  const [name, setName] = useState("");
  const [volume, setVolume] = useState(1.0);
  const [isMuted, setIsMuted] = useState(false);
  const [outputs, setOutputs] = useState<string[]>([]);
  const [iconType, setIconType] = useState<MixBus["iconType"]>("personal");
  const [activeTab, setActiveTab] = useState<"outputs" | "settings">("outputs");
  const [showAddOutputSelect, setShowAddOutputSelect] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [systemOutputs, setSystemOutputs] = useState<AudioDeviceDto[]>([]);

  useEffect(() => {
    if (availableOutputs && availableOutputs.length > 0) {
      setSystemOutputs(availableOutputs);
    } else {
      getSystemAudioDevices()
        .then((res) => {
          if (res?.outputs?.length) setSystemOutputs(res.outputs);
        })
        .catch(() => {});
    }
  }, [availableOutputs, isOpen]);

  useEffect(() => {
    if (mix) {
      setName(mix.name);
      setVolume(mix.volume);
      setIsMuted(mix.isMuted);
      setOutputs(mix.outputs || []);
      setIconType(mix.iconType || "personal");
      setShowAddOutputSelect(false);
      setConfirmDelete(false);
    }
  }, [mix]);

  if (!isOpen || !mix) return null;

  const handleSaveName = (newName: string) => {
    setName(newName);
    onUpdateMix({ ...mix, name: newName });
  };

  const handleVolumeChange = (newVol: number) => {
    setVolume(newVol);
    onUpdateMix({ ...mix, volume: newVol });
  };

  const handleMuteToggle = () => {
    const next = !isMuted;
    setIsMuted(next);
    onUpdateMix({ ...mix, isMuted: next });
  };

  const handleAddOutput = (deviceName: string) => {
    if (!outputs.includes(deviceName)) {
      const updated = [...outputs, deviceName];
      setOutputs(updated);
      onUpdateMix({ ...mix, outputs: updated });
    }
    setShowAddOutputSelect(false);
  };

  const handleRemoveOutput = (deviceName: string) => {
    const updated = outputs.filter((d) => d !== deviceName);
    setOutputs(updated);
    onUpdateMix({ ...mix, outputs: updated });
  };

  const handleIconChange = (type: MixBus["iconType"]) => {
    setIconType(type);
    onUpdateMix({ ...mix, iconType: type });
  };

  const getMixIcon = () => {
    switch (iconType) {
      case "stream":
        return <Radio className="w-8 h-8 text-[#9ca3af]" />;
      case "record":
        return <Mic className="w-8 h-8 text-[#9ca3af]" />;
      case "aux":
        return <Sliders className="w-8 h-8 text-[#9ca3af]" />;
      default:
        return <Headphones className="w-8 h-8 text-[#9ca3af]" />;
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
      <div className="w-full max-w-xl bg-[#211d27] border border-white/[0.08] rounded-xl shadow-xl shadow-black/60 flex flex-col overflow-hidden text-[#e4e4e7]">
        {/* Header */}
        <div className="px-5 py-4 border-b border-white/[0.06] flex items-center justify-between bg-[#262130]">
          <div className="flex items-center gap-3 flex-1 mr-4">
            <span className="text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af]">
              Mix settings
            </span>
            <input
              type="text"
              value={name}
              onChange={(e) => handleSaveName(e.target.value)}
              className="bg-[#16131f] border border-[#3d374a] hover:border-[#5b5470] focus:border-[#5b5470] rounded-md px-3 py-1 text-sm font-semibold text-[#e4e4e7] focus:outline-none transition-colors flex-1 max-w-xs"
              placeholder="Mix name..."
            />
          </div>

          <button
            type="button"
            onClick={onClose}
            className="w-7 h-7 rounded-md hover:bg-white/[0.06] text-[#71717a] hover:text-[#e4e4e7] flex items-center justify-center transition-colors"
            title="Close"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Body */}
        <div className="grid grid-cols-12 gap-5 p-5">
          {/* Left: mix tile + master volume */}
          <div className="col-span-5 flex flex-col items-center bg-[#191621] border border-white/[0.06] rounded-lg p-4">
            <div className="w-20 h-20 rounded-lg bg-[#272230] border border-white/[0.06] flex items-center justify-center mb-3">
              {getMixIcon()}
            </div>

            <span className="text-xs font-semibold text-[#e4e4e7] text-center truncate max-w-full mb-1">
              {name || "Untitled Mix"}
            </span>
            <span className="text-[11px] font-mono text-[#71717a] mb-4">
              {outputs.length} {outputs.length === 1 ? "output" : "outputs"}
            </span>

            <div className="w-full space-y-1 mb-4">
              <div className="flex items-center justify-between text-xs font-mono">
                <span className="text-[#71717a]">Mix volume</span>
                <span className={isMuted ? "text-[#ef4444]" : "text-[#9ca3af]"}>
                  {isMuted ? "MUTED" : `${Math.round(volume * 100)}%`}
                </span>
              </div>
              <input
                type="range"
                min="0"
                max="1.0"
                step="0.01"
                value={volume}
                onChange={(e) => handleVolumeChange(parseFloat(e.target.value))}
                className="wl-slider w-full cursor-pointer"
              />
            </div>

            <button
              type="button"
              onClick={handleMuteToggle}
              className={`w-full py-2 px-3 rounded-md border text-xs font-medium flex items-center justify-center gap-2 transition-colors mb-4 ${
                isMuted
                  ? "bg-[#3a1c24] border-[#ef4444] text-[#ef4444]"
                  : "bg-[#272230] border-white/[0.06] text-[#9ca3af] hover:text-[#e4e4e7]"
              }`}
            >
              {isMuted ? <VolumeX className="w-4 h-4" /> : <Volume2 className="w-4 h-4" />}
              <span>{isMuted ? "Unmute mix" : "Mute mix"}</span>
            </button>

            <div className="mt-auto w-full pt-2 border-t border-white/[0.06]">
              {confirmDelete ? (
                <div className="space-y-1.5 text-center">
                  <div className="text-[10px] text-[#ef4444] font-medium">Delete this mix bus?</div>
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => setConfirmDelete(false)}
                      className="flex-1 py-1 text-xs rounded-md bg-[#272230] text-[#9ca3af] hover:text-[#e4e4e7] transition-colors"
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        onDeleteMix(mix.id);
                        onClose();
                      }}
                      className="flex-1 py-1 text-xs rounded-md bg-[#3a1c24] border border-[#ef4444] text-[#ef4444] font-medium transition-colors"
                    >
                      Confirm
                    </button>
                  </div>
                </div>
              ) : (
                <button
                  type="button"
                  disabled={!canDelete}
                  onClick={() => setConfirmDelete(true)}
                  className={`w-full py-1.5 px-2 rounded-md text-xs flex items-center justify-center gap-1.5 border border-transparent transition-colors ${
                    canDelete ? "text-[#9ca3af] hover:text-[#ef4444] hover:bg-white/[0.04]" : "text-[#3f3f46] cursor-not-allowed"
                  }`}
                  title={canDelete ? "Delete this mix" : "Cannot delete the only remaining mix"}
                >
                  <Trash2 className="w-3.5 h-3.5" />
                  <span>Delete mix</span>
                </button>
              )}
            </div>
          </div>

          {/* Right: outputs & settings */}
          <div className="col-span-7 flex flex-col">
            <div className="flex items-center gap-3 border-b border-white/[0.06] pb-2 mb-3">
              <button
                type="button"
                onClick={() => setActiveTab("outputs")}
                className={`text-xs font-semibold pb-1 border-b-2 transition-colors ${
                  activeTab === "outputs"
                    ? "border-[#c084fc] text-[#e4e4e7]"
                    : "border-transparent text-[#71717a] hover:text-[#9ca3af]"
                }`}
              >
                Audio output ({outputs.length})
              </button>
              <button
                type="button"
                onClick={() => setActiveTab("settings")}
                className={`text-xs font-semibold pb-1 border-b-2 transition-colors ${
                  activeTab === "settings"
                    ? "border-[#c084fc] text-[#e4e4e7]"
                    : "border-transparent text-[#71717a] hover:text-[#9ca3af]"
                }`}
              >
                Mix settings
              </button>
            </div>

            {activeTab === "outputs" && (
              <div className="flex-1 flex flex-col">
                <p className="text-[11px] text-[#71717a] mb-3 leading-relaxed">
                  Assign Windows 11 hardware outputs to duplicate this mix (e.g. headphones and speakers at once).
                </p>

                <div className="space-y-1.5 flex-1 max-h-48 overflow-y-auto pr-1">
                  {outputs.map((dev) => (
                    <div
                      key={dev}
                      className="flex items-center justify-between p-2 rounded-md bg-[#191621] border border-white/[0.06]"
                    >
                      <div className="flex items-center gap-2 min-w-0">
                        <Speaker className="w-4 h-4 text-[#9ca3af] shrink-0" />
                        <span className="text-xs text-[#e4e4e7] truncate font-medium">{dev}</span>
                      </div>
                      <button
                        type="button"
                        onClick={() => handleRemoveOutput(dev)}
                        className="text-[#71717a] hover:text-[#e4e4e7] p-1 rounded hover:bg-white/[0.06] transition-colors"
                        title="Remove output"
                      >
                        <X className="w-3.5 h-3.5" />
                      </button>
                    </div>
                  ))}

                  {outputs.length === 0 && (
                    <div className="p-3 text-center text-xs text-[#71717a] border border-dashed border-white/[0.08] rounded-md">
                      No output devices assigned to this mix.
                    </div>
                  )}
                </div>

                <div className="mt-3 pt-3 border-t border-white/[0.06]">
                  {showAddOutputSelect ? (
                    <div className="space-y-2">
                      <div className="text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af]">
                        Select device to add
                      </div>
                      <div className="space-y-1 max-h-32 overflow-y-auto">
                        {systemOutputs
                          .filter((dev) => !outputs.includes(dev.name))
                          .map((dev) => (
                            <button
                              key={dev.id}
                              type="button"
                              onClick={() => handleAddOutput(dev.name)}
                              className="w-full text-left p-1.5 rounded-md hover:bg-white/[0.04] text-xs text-[#9ca3af] hover:text-[#e4e4e7] flex items-center justify-between transition-colors"
                            >
                              <span className="truncate">{dev.name}</span>
                              <Plus className="w-3.5 h-3.5 text-[#71717a] shrink-0" />
                            </button>
                          ))}
                        {systemOutputs.filter((dev) => !outputs.includes(dev.name)).length === 0 && (
                          <div className="p-2 text-[11px] text-[#71717a]">
                            Every detected output is already assigned.
                          </div>
                        )}
                      </div>
                      <button
                        type="button"
                        onClick={() => setShowAddOutputSelect(false)}
                        className="text-[11px] text-[#71717a] hover:text-[#e4e4e7] transition-colors"
                      >
                        Cancel
                      </button>
                    </div>
                  ) : (
                    <button
                      type="button"
                      onClick={() => setShowAddOutputSelect(true)}
                      className="w-full py-2 px-3 rounded-md bg-[#272230] hover:bg-[#332d3d] border border-white/[0.06] text-xs font-medium text-[#9ca3af] hover:text-[#e4e4e7] flex items-center justify-center gap-2 transition-colors"
                    >
                      <Plus className="w-4 h-4 text-[#71717a]" />
                      <span>Add output</span>
                    </button>
                  )}
                </div>
              </div>
            )}

            {activeTab === "settings" && (
              <div className="space-y-4 text-xs">
                <div>
                  <div className="text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af] mb-2">
                    Mix icon
                  </div>
                  <div className="flex items-center gap-2">
                    {(["personal", "stream", "record", "aux"] as const).map((t) => (
                      <button
                        key={t}
                        type="button"
                        onClick={() => handleIconChange(t)}
                        className={`p-2.5 rounded-md border capitalize flex items-center gap-1.5 transition-colors ${
                          iconType === t
                            ? "bg-[#272230] border-white/[0.12] text-[#e4e4e7]"
                            : "bg-[#191621] border-white/[0.06] text-[#71717a] hover:text-[#9ca3af]"
                        }`}
                      >
                        {t === "personal" && <Headphones className="w-3.5 h-3.5" />}
                        {t === "stream" && <Radio className="w-3.5 h-3.5" />}
                        {t === "record" && <Mic className="w-3.5 h-3.5" />}
                        {t === "aux" && <Sliders className="w-3.5 h-3.5" />}
                        <span>{t}</span>
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Footer */}
        <div className="px-5 py-3 border-t border-white/[0.06] bg-[#262130] flex items-center justify-end">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-1.5 rounded-md bg-[#332d3d] hover:bg-[#3d374a] text-[#e4e4e7] font-medium text-xs transition-colors"
          >
            Close window
          </button>
        </div>
      </div>
    </div>
  );
};
