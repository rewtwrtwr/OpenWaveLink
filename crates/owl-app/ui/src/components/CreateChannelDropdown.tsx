import React, { useState, useEffect, useRef, useMemo } from "react";
import { Search, Mic, AppWindow, Plus, ChevronRight, X, Volume2, Loader2 } from "lucide-react";
import { ChannelSourceType } from "../types";
import {
  getActiveAudioSessions,
  getSystemAudioDevices,
  getInstalledApplications,
  AudioProcessDto,
  AudioDeviceDto,
  InstalledAppDto,
} from "../ipc";

interface CreateChannelDropdownProps {
  isOpen: boolean;
  onClose: () => void;
  onAddChannel: (channel: {
    name: string;
    type: ChannelSourceType;
    pid?: number;
    processName?: string;
    deviceEndpointId?: string;
    iconUrl?: string;
  }) => void;
}

/**
 * `+ Create channel` panel.
 *
 * Opens STRICTLY DOWNWARD (`absolute top-full mt-2 left-0 z-50`) — it never
 * flips up and never covers its own button (spec §3.2 / contract §6).
 *
 * Contents are 100% real Windows 11 data:
 *   • Input devices  — IMMDeviceEnumerator(eCapture) via getSystemAudioDevices()
 *   • Active apps    — IAudioSessionEnumerator via getActiveAudioSessions(), is_active only
 *   • Installed apps — registered Windows 11 programs via getInstalledApplications()
 * There is deliberately NO hardcoded/mock list anywhere in this file.
 */
export const CreateChannelDropdown: React.FC<CreateChannelDropdownProps> = ({
  isOpen,
  onClose,
  onAddChannel,
}) => {
  const [search, setSearch] = useState("");
  const [isAddingEmpty, setIsAddingEmpty] = useState(false);
  const [emptyChannelName, setEmptyChannelName] = useState("");

  const [inputDevices, setInputDevices] = useState<AudioDeviceDto[]>([]);
  const [activeSessions, setActiveSessions] = useState<AudioProcessDto[]>([]);
  const [installedApps, setInstalledApps] = useState<InstalledAppDto[]>([]);
  const [isLoading, setIsLoading] = useState(false);

  const containerRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);

  // Load real Windows 11 system data whenever the panel opens.
  useEffect(() => {
    if (isOpen) {
      setSearch("");
      setIsAddingEmpty(false);
      setEmptyChannelName("");
      setIsLoading(true);

      const focusTimer = setTimeout(() => searchInputRef.current?.focus(), 50);

      const loadData = () => {
        Promise.all([
          getSystemAudioDevices().catch(() => ({ inputs: [], outputs: [] })),
          getActiveAudioSessions().catch(() => []),
          getInstalledApplications().catch(() => []),
        ])
          .then(([devices, sessions, apps]) => {
            setInputDevices(devices.inputs || []);
            setActiveSessions(sessions || []);
            setInstalledApps(apps || []);
          })
          .finally(() => {
            setIsLoading(false);
          });
      };

      loadData();

      // Auto-refresh active audio sessions every 1.5 seconds while open
      const pollTimer = setInterval(() => {
        getActiveAudioSessions()
          .then((sessions) => {
            setActiveSessions(sessions || []);
          })
          .catch(() => {});
      }, 1500);

      return () => {
        clearTimeout(focusTimer);
        clearInterval(pollTimer);
      };
    }
    return undefined;
  }, [isOpen]);

  // Close on outside click / Escape.
  useEffect(() => {
    if (!isOpen) return undefined;

    const handleOutsideClick = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };

    document.addEventListener("mousedown", handleOutsideClick);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("mousedown", handleOutsideClick);
      document.removeEventListener("keydown", handleKey);
    };
  }, [isOpen, onClose]);

  const query = search.trim().toLowerCase();

  const filteredInputs = useMemo(() => {
    if (!query) return inputDevices;
    return inputDevices.filter((dev) => dev.name.toLowerCase().includes(query));
  }, [inputDevices, query]);

  const filteredActive = useMemo(() => {
    if (!query) return activeSessions;
    return activeSessions.filter(
      (s) =>
        s.name.toLowerCase().includes(query) ||
        s.process_name.toLowerCase().includes(query)
    );
  }, [activeSessions, query]);

  const filteredInstalled = useMemo(() => {
    if (!query) return installedApps.slice(0, 30);
    return installedApps.filter(
      (app) =>
        app.name.toLowerCase().includes(query) ||
        app.exe_name.toLowerCase().includes(query)
    );
  }, [installedApps, query]);

  if (!isOpen) return null;

  const hasAnyMatches =
    filteredInputs.length > 0 ||
    filteredActive.length > 0 ||
    filteredInstalled.length > 0;

  const handleSelectInput = (device: AudioDeviceDto) => {
    onAddChannel({
      name: device.name,
      type: "hardware",
      deviceEndpointId: device.id,
    });
    onClose();
  };

  const handleSelectActiveApp = (session: AudioProcessDto) => {
    onAddChannel({
      name: session.name,
      type: "app",
      pid: session.pid,
      processName: session.process_name,
      iconUrl: session.icon,
    });
    onClose();
  };

  const handleSelectInstalledApp = (app: InstalledAppDto) => {
    onAddChannel({
      name: app.name,
      type: "app",
      processName: app.exe_name,
    });
    onClose();
  };

  const handleCreateEmpty = (e: React.FormEvent) => {
    e.preventDefault();
    const finalName = emptyChannelName.trim() || search.trim() || "Virtual Channel";
    onAddChannel({
      name: finalName,
      type: "empty",
    });
    onClose();
  };

  return (
    <div
      ref={containerRef}
      className="absolute top-full mt-2 left-0 z-50 w-[410px] max-h-[480px] bg-[#1c1c20] border border-white/[0.08] rounded-xl shadow-2xl shadow-black/80 flex flex-col overflow-hidden text-[#f4f4f5]"
    >
      {/* Header & Search */}
      <div className="p-3 border-b border-white/[0.06] shrink-0">
        <div className="flex items-center justify-between mb-2">
          <span className="text-[11px] font-semibold uppercase tracking-wider text-[#a1a1aa]">
            Create Channel
          </span>
          <button
            type="button"
            onClick={onClose}
            className="text-[#a1a1aa] hover:text-[#f4f4f5] p-0.5 rounded hover:bg-white/5 transition-colors"
            title="Close"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>

        <div className="relative">
          <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-[#71717a]" />
          <input
            ref={searchInputRef}
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search audio sources & apps..."
            className="w-full bg-[#141416] border border-[#2a2a2f] rounded-lg pl-8 pr-7 py-1.5 text-xs text-[#f4f4f5] placeholder-[#71717a] focus:outline-none focus:border-[#44444c] transition-colors"
          />
          {search && (
            <button
              type="button"
              onClick={() => setSearch("")}
              className="absolute right-2.5 top-1/2 -translate-y-1/2 text-[#71717a] hover:text-[#f4f4f5]"
              title="Clear search"
            >
              <X className="w-3 h-3" />
            </button>
          )}
        </div>
      </div>

      {/* Scrollable list — neutral dark scrollbar */}
      <div className="flex-1 min-h-0 overflow-y-auto overscroll-contain p-2 space-y-3 text-xs">
        {isLoading ? (
          <div className="py-8 flex flex-col items-center justify-center gap-2 text-[#71717a]">
            <Loader2 className="w-5 h-5 animate-spin" />
            <span className="text-[11px]">Scanning Windows 11 audio endpoints...</span>
          </div>
        ) : isAddingEmpty ? (
          <form onSubmit={handleCreateEmpty} className="p-2 space-y-2.5">
            <div className="text-[11px] text-[#a1a1aa]">Enter custom channel name:</div>
            <input
              type="text"
              autoFocus
              value={emptyChannelName}
              onChange={(e) => setEmptyChannelName(e.target.value)}
              placeholder="e.g. Soundboard, Music, Aux"
              className="w-full bg-[#141416] border border-[#2a2a2f] focus:border-[#44444c] rounded-lg px-2.5 py-1.5 text-xs text-[#f4f4f5] focus:outline-none transition-colors"
            />
            <div className="flex items-center justify-end gap-2 pt-1">
              <button
                type="button"
                onClick={() => setIsAddingEmpty(false)}
                className="px-2.5 py-1 rounded-md bg-[#242429] text-[#a1a1aa] hover:text-[#f4f4f5] text-[11px] transition-colors"
              >
                Back
              </button>
              <button
                type="submit"
                className="px-3 py-1 rounded-md bg-[#2e2e34] hover:bg-[#383840] text-[#f4f4f5] font-medium text-[11px] transition-colors"
              >
                Add Channel
              </button>
            </div>
          </form>
        ) : (
          <>
            {/* 1. ACTIVE APPS — processes rendering sound right now */}
            {filteredActive.length > 0 && (
              <div>
                <div className="px-2 py-1 flex items-center justify-between text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af]">
                  <span>Active Apps ({filteredActive.length})</span>
                  <span className="flex items-center gap-1.5 text-[10px] text-[#22c55e] font-medium">
                    <span className="w-1.5 h-1.5 rounded-full bg-[#22c55e] animate-pulse" />
                    live audio
                  </span>
                </div>
                <div className="space-y-0.5 mt-0.5">
                  {filteredActive.map((session) => (
                    <button
                      key={`${session.pid}_${session.name}`}
                      type="button"
                      onClick={() => handleSelectActiveApp(session)}
                      className="w-full flex items-center gap-2.5 px-2.5 py-2 rounded-lg hover:bg-white/[0.05] text-left transition-colors group"
                    >
                      <div className="w-8 h-8 rounded-lg bg-[#16131f] border border-white/[0.08] flex items-center justify-center shrink-0 overflow-hidden">
                        {session.icon ? (
                          <img src={session.icon} className="w-6 h-6 object-contain" alt="" />
                        ) : (
                          <Volume2 className="w-4 h-4 text-[#22c55e]" />
                        )}
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="font-medium text-[#e4e4e7] truncate text-xs">
                          {session.name}
                        </div>
                        <div className="text-[10px] text-[#71717a] truncate font-mono">
                          {session.process_name} • PID {session.pid}
                        </div>
                      </div>
                      <Plus className="w-3.5 h-3.5 text-[#71717a] group-hover:text-[#e4e4e7] shrink-0 transition-colors" />
                    </button>
                  ))}
                </div>
              </div>
            )}

            {/* 2. INPUT DEVICES — real capture endpoints */}
            {filteredInputs.length > 0 && (
              <div>
                <div className="px-2 py-1 text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af]">
                  Input Devices
                </div>
                <div className="space-y-0.5 mt-0.5">
                  {filteredInputs.map((dev) => (
                    <button
                      key={dev.id}
                      type="button"
                      onClick={() => handleSelectInput(dev)}
                      className="w-full flex items-center gap-2.5 px-2.5 py-2 rounded-lg hover:bg-white/[0.05] text-left transition-colors group"
                    >
                      <div className="w-8 h-8 rounded-lg bg-[#16131f] border border-white/[0.08] flex items-center justify-center shrink-0">
                        <Mic className="w-4 h-4 text-[#9ca3af]" />
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="font-medium text-[#e4e4e7] truncate text-xs">
                          {dev.name}
                        </div>
                        <div className="text-[10px] text-[#71717a] truncate">
                          {dev.is_default ? "Default Recording Device" : "Audio Capture Endpoint"}
                        </div>
                      </div>
                      <Plus className="w-3.5 h-3.5 text-[#71717a] group-hover:text-[#e4e4e7] shrink-0 transition-colors" />
                    </button>
                  ))}
                </div>
              </div>
            )}

            {/* 3. INSTALLED APPS — registered Windows 11 programs */}
            {filteredInstalled.length > 0 && (
              <div>
                <div className="px-2 py-1 text-[11px] font-semibold uppercase tracking-wider text-[#9ca3af]">
                  Installed Apps
                </div>
                <div className="space-y-0.5 mt-0.5">
                  {filteredInstalled.map((app) => (
                    <button
                      key={`${app.exe_name}_${app.path}`}
                      type="button"
                      onClick={() => handleSelectInstalledApp(app)}
                      className="w-full flex items-center gap-2.5 px-2 py-1.5 rounded-md hover:bg-white/[0.04] text-left transition-colors group"
                    >
                      <div className="w-7 h-7 rounded-md bg-[#16131f] border border-white/[0.06] flex items-center justify-center shrink-0">
                        <AppWindow className="w-4 h-4 text-[#9ca3af]" />
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="font-medium text-[#e4e4e7] truncate">
                          {app.name}
                        </div>
                        <div className="text-[10px] text-[#71717a] truncate font-mono">
                          {app.exe_name}
                        </div>
                      </div>
                      <Plus className="w-3.5 h-3.5 text-[#71717a] group-hover:text-[#e4e4e7] shrink-0 transition-colors" />
                    </button>
                  ))}
                </div>
              </div>
            )}

            {/* Nothing scanned at all / no search match */}
            {!hasAnyMatches && (
              <div className="py-6 px-4 text-center text-[#71717a]">
                <p className="mb-2 text-xs">
                  {query
                    ? "No matching audio sources or apps found"
                    : "No audio sources detected on this system yet."}
                </p>
                {query && (
                  <button
                    type="button"
                    onClick={() => {
                      onAddChannel({ name: search.trim(), type: "empty" });
                      onClose();
                    }}
                    className="px-3 py-1.5 rounded-md bg-[#332d3d] hover:bg-[#3d374a] text-[#e4e4e7] text-[11px] font-medium transition-colors"
                  >
                    Create &quot;{search.trim()}&quot; as empty channel
                  </button>
                )}
              </div>
            )}
          </>
        )}
      </div>

      {/* Footer: Add empty channel */}
      {!isAddingEmpty && (
        <div className="p-2 border-t border-white/[0.06] shrink-0">
          <button
            type="button"
            onClick={() => setIsAddingEmpty(true)}
            className="w-full flex items-center justify-between px-2.5 py-1.5 rounded-md hover:bg-white/[0.04] text-[#9ca3af] hover:text-[#e4e4e7] transition-colors text-xs group"
          >
            <div className="flex items-center gap-2">
              <Plus className="w-3.5 h-3.5 text-[#71717a]" />
              <span className="font-medium">Add empty channel &gt;</span>
            </div>
            <ChevronRight className="w-3.5 h-3.5 text-[#71717a]" />
          </button>
        </div>
      )}
    </div>
  );
};
