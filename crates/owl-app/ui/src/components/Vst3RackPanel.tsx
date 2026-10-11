import React, { useState, useEffect } from "react";
import { Plus, Power, ExternalLink, Trash2, ShieldCheck, Sliders, ChevronDown } from "lucide-react";
import { Vst3PluginDto, Vst3SlotDto, getVst3Plugins, setVst3Slot, openVst3Editor } from "../ipc";

interface Vst3RackPanelProps {
  channelId: number;
  channelName: string;
  slots: Vst3SlotDto[];
  onSlotsUpdate: (newSlots: Vst3SlotDto[]) => void;
  onClose?: () => void;
}

/** VST3 insert rack — matte graphite surfaces, no neon, no coloured borders. */
export const Vst3RackPanel: React.FC<Vst3RackPanelProps> = ({
  channelId,
  channelName,
  slots,
  onSlotsUpdate,
  onClose,
}) => {
  const [availablePlugins, setAvailablePlugins] = useState<Vst3PluginDto[]>([]);
  const [showAddMenu, setShowAddMenu] = useState<boolean>(false);
  const [limitNotice, setLimitNotice] = useState<string | null>(null);

  useEffect(() => {
    getVst3Plugins()
      .then((plugins) => setAvailablePlugins(plugins))
      .catch(console.error);
  }, []);

  const handleAddPlugin = async (plugin: Vst3PluginDto) => {
    setShowAddMenu(false);
    setLimitNotice(null);

    let slotIdx = slots.findIndex((s) => !s.is_enabled);
    if (slotIdx === -1) {
      if (slots.length >= 8) {
        setLimitNotice("Slot limit reached (8 per channel).");
        return;
      }
      slotIdx = slots.length;
    }

    const newSlot: Vst3SlotDto = {
      slot_id: slotIdx,
      plugin_id: plugin.id,
      plugin_name: plugin.name,
      category: plugin.category,
      is_enabled: true,
      is_bypassed: false,
      dry_wet: 1.0,
    };

    const updated = [...slots];
    if (slotIdx < updated.length) {
      updated[slotIdx] = newSlot;
    } else {
      updated.push(newSlot);
    }

    onSlotsUpdate(updated);

    try {
      await setVst3Slot(channelId, slotIdx, {
        plugin_id: plugin.id,
        is_bypassed: false,
        dry_wet: 1.0,
      });
    } catch (e) {
      console.error("Failed to load VST3 plugin:", e);
    }
  };

  const handleToggleBypass = async (slotIdx: number) => {
    const slot = slots[slotIdx];
    if (!slot) return;

    const newBypassed = !slot.is_bypassed;
    const updated = slots.map((s, idx) =>
      idx === slotIdx ? { ...s, is_bypassed: newBypassed } : s
    );
    onSlotsUpdate(updated);

    try {
      await setVst3Slot(channelId, slotIdx, { is_bypassed: newBypassed });
    } catch (e) {
      console.error("Failed to toggle bypass:", e);
    }
  };

  const handleDryWetChange = async (slotIdx: number, dryWet: number) => {
    const updated = slots.map((s, idx) =>
      idx === slotIdx ? { ...s, dry_wet: dryWet } : s
    );
    onSlotsUpdate(updated);

    try {
      await setVst3Slot(channelId, slotIdx, { dry_wet: dryWet });
    } catch (e) {
      console.error("Failed to update dry/wet:", e);
    }
  };

  const handleRemoveSlot = async (slotIdx: number) => {
    const updated = slots.map((s, idx) =>
      idx === slotIdx
        ? {
            ...s,
            plugin_id: undefined,
            plugin_name: undefined,
            category: undefined,
            is_enabled: false,
            is_bypassed: false,
          }
        : s
    );
    onSlotsUpdate(updated);

    try {
      await setVst3Slot(channelId, slotIdx, { clear: true });
    } catch (e) {
      console.error("Failed to clear VST3 slot:", e);
    }
  };

  const handleOpenGui = async (slotIdx: number) => {
    try {
      await openVst3Editor(channelId, slotIdx);
    } catch (e) {
      console.error("Failed to open VST3 editor:", e);
    }
  };

  const activeSlots = slots.filter((s) => s.is_enabled);

  return (
    <div className="p-3 bg-[#211d27] border border-white/[0.06] rounded-lg text-[#e4e4e7]">
      {/* Rack header */}
      <div className="flex items-center justify-between pb-2.5 mb-2 border-b border-white/[0.06]">
        <div className="flex items-center gap-2 min-w-0">
          <Sliders className="w-4 h-4 text-[#9ca3af] shrink-0" />
          <span className="text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af] truncate">
            VST3 inserts — {channelName}
          </span>
          <span className="flex items-center gap-1 text-[10px] text-[#71717a] bg-white/[0.04] px-1.5 py-0.5 rounded border border-white/[0.06] font-mono shrink-0">
            <ShieldCheck className="w-3 h-3" />
            Crash guard
          </span>
        </div>

        <div className="flex items-center gap-2 relative shrink-0">
          {onClose && (
            <button
              type="button"
              onClick={onClose}
              className="px-2 py-1 text-xs text-[#71717a] hover:text-[#e4e4e7] rounded-md hover:bg-white/[0.06] transition-colors"
              title="Close the rack"
            >
              Close
            </button>
          )}

          <button
            type="button"
            onClick={() => setShowAddMenu(!showAddMenu)}
            className="flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium bg-[#332d3d] hover:bg-[#3d374a] text-[#e4e4e7] rounded-md transition-colors"
          >
            <Plus className="w-3.5 h-3.5" />
            Add VST3
            <ChevronDown className="w-3 h-3 text-[#71717a]" />
          </button>

          {showAddMenu && (
            <div className="absolute right-0 top-8 w-64 bg-[#201c2b] border border-white/[0.08] rounded-lg shadow-xl shadow-black/60 z-50 p-1">
              <div className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wider text-[#71717a]">
                Available VST3 plugins ({availablePlugins.length})
              </div>
              <div className="max-h-60 overflow-y-auto py-1 space-y-0.5">
                {availablePlugins.length === 0 && (
                  <div className="px-2.5 py-2 text-[11px] text-[#71717a]">
                    No VST3 plugins found on this system.
                  </div>
                )}
                {availablePlugins.map((plugin) => (
                  <button
                    key={plugin.id}
                    type="button"
                    onClick={() => handleAddPlugin(plugin)}
                    className="w-full text-left px-2.5 py-1.5 rounded-md hover:bg-white/[0.04] flex items-center justify-between transition-colors"
                  >
                    <div className="flex flex-col min-w-0 pr-2">
                      <span className="text-xs font-medium text-[#e4e4e7] truncate">
                        {plugin.name}
                      </span>
                      <span className="text-[10px] text-[#71717a]">{plugin.vendor}</span>
                    </div>
                    <span className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-[#191621] text-[#71717a] shrink-0 border border-white/[0.06]">
                      {plugin.category}
                    </span>
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>

      {limitNotice && (
        <div className="mb-2 px-2 py-1 rounded-md bg-[#3a1c24] border border-[#ef4444] text-[11px] text-[#ef4444]">
          {limitNotice}
        </div>
      )}

      {activeSlots.length === 0 ? (
        <div className="py-6 text-center text-xs text-[#71717a] border border-dashed border-white/[0.08] rounded-md">
          No VST3 effects on this channel yet.
        </div>
      ) : (
        <div className="space-y-2">
          {slots.map((slot, idx) => {
            if (!slot.is_enabled) return null;
            return (
              <div
                key={idx}
                className={`flex items-center justify-between p-2 rounded-md border transition-colors ${
                  slot.is_bypassed
                    ? "bg-[#191621] border-white/[0.06] opacity-70"
                    : "bg-[#272230] border-white/[0.06]"
                }`}
              >
                <div className="flex items-center gap-2.5 min-w-0 flex-1">
                  <div className="w-5 h-5 rounded bg-[#191621] border border-white/[0.06] flex items-center justify-center text-[10px] font-mono text-[#71717a] shrink-0">
                    {idx + 1}
                  </div>
                  <div className="min-w-0 pr-2">
                    <div className="flex items-center gap-1.5">
                      <span className="text-xs font-semibold text-[#e4e4e7] truncate">
                        {slot.plugin_name || "Unknown plugin"}
                      </span>
                      <span className="text-[9px] font-mono px-1 py-0.5 rounded bg-[#191621] text-[#71717a] border border-white/[0.06]">
                        {slot.category || "FX"}
                      </span>
                    </div>
                    <div className="text-[10px] text-[#71717a] flex items-center gap-2 mt-0.5 font-mono">
                      <span>PDC: +32 smp</span>
                      <span>•</span>
                      <span>{slot.is_bypassed ? "BYPASS" : "ACTIVE"}</span>
                    </div>
                  </div>
                </div>

                <div className="flex items-center gap-2 px-3 shrink-0">
                  <div className="flex flex-col items-center">
                    <span className="text-[9px] font-mono text-[#71717a]">Dry / Wet</span>
                    <input
                      type="range"
                      min="0"
                      max="1"
                      step="0.01"
                      value={slot.dry_wet}
                      disabled={slot.is_bypassed}
                      onChange={(e) => handleDryWetChange(idx, parseFloat(e.target.value))}
                      className="wl-slider w-20 cursor-pointer mt-0.5"
                    />
                  </div>
                  <span className="text-[10px] font-mono text-[#9ca3af] w-8 text-right">
                    {Math.round(slot.dry_wet * 100)}%
                  </span>
                </div>

                <div className="flex items-center gap-1.5 shrink-0">
                  <button
                    type="button"
                    onClick={() => handleToggleBypass(idx)}
                    className="px-1.5 py-1 rounded-md bg-[#191621] hover:bg-[#332d3d] text-[#9ca3af] hover:text-[#e4e4e7] border border-white/[0.06] transition-colors flex items-center gap-1 text-[10px] font-mono"
                    title={slot.is_bypassed ? "Enable processing" : "Bypass plugin"}
                  >
                    <Power className="w-3.5 h-3.5" />
                    <span>{slot.is_bypassed ? "Bypass" : "On"}</span>
                  </button>

                  <button
                    type="button"
                    onClick={() => handleOpenGui(idx)}
                    className="px-1.5 py-1 rounded-md bg-[#191621] hover:bg-[#332d3d] text-[#9ca3af] hover:text-[#e4e4e7] border border-white/[0.06] transition-colors flex items-center gap-1 text-[10px] font-medium"
                    title="Open the plugin's native GUI"
                  >
                    <ExternalLink className="w-3.5 h-3.5" />
                    <span>GUI</span>
                  </button>

                  <button
                    type="button"
                    onClick={() => handleRemoveSlot(idx)}
                    className="p-1.5 rounded-md bg-[#191621] hover:bg-[#332d3d] text-[#71717a] hover:text-[#ef4444] border border-white/[0.06] transition-colors"
                    title="Remove plugin from slot"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
