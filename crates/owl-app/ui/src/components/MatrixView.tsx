import React, { useState, useEffect, useRef } from "react";
import { Monitor, Mic, AppWindow, Radio, Sliders, Volume2, VolumeX, Plus, X, Headphones } from "lucide-react";
import { HeaderBar } from "./HeaderBar";
import { CrossPointCell } from "./CrossPointCell";
import { CreateChannelDropdown } from "./CreateChannelDropdown";
import { MixSettingsModal } from "./MixSettingsModal";
import { ChannelNode, MixBus, AppMatrixState, loadPersistedState, savePersistedState, DEFAULT_MATRIX_STATE } from "../types";
import {
  AudioDeviceDto,
  ChannelMeterDto,
  EngineChannelDto,
  TelemetrySnapshotDto,
  Vst3SlotDto,
  addChannel,
  removeChannel,
  getEngineState,
  getTelemetrySnapshot,
  listenEvent,
  setChannelGain,
  setChannelMute,
  setSendGain,
  setSendMute,
  setMixGain,
  setMixMute,
  getActiveAudioSessions,
  getSystemAudioDevices,
  createCables,
  bindAppAudio,
  getAppIcon,
  setDefaultPlaybackDevice,
  loadAppConfigFile,
} from "../ipc";

/** Maps our UI source type onto the frozen daemon `channel_type` enum. */
const toChannelType = (type: ChannelNode["type"]): string =>
  type === "hardware" ? "Microphone" : "SoftwareProcess";

/**
 * Names of the virtual cables the user has asked for.
 */
const desiredCableNames: string[] = [];

export const MatrixView: React.FC = () => {
  // 1. Dynamic matrix state
  const [matrixState, setMatrixState] = useState<AppMatrixState>(() => loadPersistedState());
  const [channelMeters, setChannelMeters] = useState<ChannelMeterDto[]>([]);
  const [mixMeters, setMixMeters] = useState<ChannelMeterDto[]>([]);
  const [fps, setFps] = useState<number>(0);
  const [daemonConnected, setDaemonConnected] = useState<boolean>(false);
  const [outputDevices, setOutputDevices] = useState<AudioDeviceDto[]>([]);
  const [appIcons, setAppIcons] = useState<Record<string, string>>({});
  const [draggingIndex, setDraggingIndex] = useState<number | null>(null);

  // UI state for modals / popovers
  const [isCreateChannelOpen, setIsCreateChannelOpen] = useState(false);
  const [selectedMixForModal, setSelectedMixForModal] = useState<MixBus | null>(null);
  const [editingChannelId, setEditingChannelId] = useState<string | null>(null);
  const [editingChannelName, setEditingChannelName] = useState("");

  // Inline new-mix popover
  const [isAddingMix, setIsAddingMix] = useState(false);
  const [newMixName, setNewMixName] = useState("");

  const frameCountRef = useRef(0);

  const { channels, mixes, routes, listeningMixId, monitorDevice } = matrixState;

  // Autosave
  useEffect(() => {
    savePersistedState(matrixState);
  }, [matrixState]);

  // Real telemetry frame rate (measured, not hardcoded)
  useEffect(() => {
    const id = setInterval(() => {
      setFps(frameCountRef.current);
      frameCountRef.current = 0;
    }, 1000);
    return () => clearInterval(id);
  }, []);

  // Load real application icons for channels
  useEffect(() => {
    channels.forEach((ch) => {
      if (ch.iconUrl && !appIcons[ch.id]) {
        setAppIcons((prev) => ({ ...prev, [ch.id]: ch.iconUrl! }));
      } else if (!appIcons[ch.id] && (ch.pid || ch.processName)) {
        getAppIcon(ch.pid, ch.processName)
          .then((icon) => {
            if (icon) {
              setAppIcons((prev) => ({ ...prev, [ch.id]: icon }));
            }
          })
          .catch(() => {});
      }
    });
  }, [channels, appIcons]);

  // Load config from Documents on startup, and auto-create all cables from config
  useEffect(() => {
    loadAppConfigFile()
      .then((content) => {
        let loadedState: AppMatrixState = matrixState;
        if (content) {
          try {
            const parsed = JSON.parse(content);
            if (parsed && Array.isArray(parsed.channels) && Array.isArray(parsed.mixes)) {
              loadedState = {
                ...matrixState,
                ...parsed,
              };
              setMatrixState(loadedState);
            }
          } catch (e) {
            console.warn("Could not parse Documents config:", e);
          }
        }

        // Auto-create all cables from the loaded config
        const cables: string[] = ["System:out"];
        (loadedState.channels || []).forEach((ch) => {
          if (ch.name.toLowerCase() !== "system") {
            cables.push(`${ch.name}:out`);
          }
        });
        (loadedState.mixes || []).forEach((m) => {
          cables.push(m.name);
        });

        cables.forEach((c) => {
          if (!desiredCableNames.includes(c)) desiredCableNames.push(c);
        });

        createCables([...desiredCableNames])
          .then((res) => {
            console.info("Configured cables ready:", res.map((c) => c.name).join(", "));
            const sysMatch = res.find((c) => c.name === "System:out" || c.name === "OWL System:out" || c.name === "System");
            if (sysMatch?.render) {
              setDefaultPlaybackDevice(sysMatch.render).catch(() => {});
            }
          })
          .catch((err) => console.warn("Initial cable creation failed:", err));
      })
      .catch(() => {
        registerCable("System", true, undefined, true);
      });
  }, []);

  // Real Windows 11 render endpoints, for the mix-settings modal.
  // NOTE: the initial *binding* is owned by HeaderBar's `on <device>` selector,
  // which calls `setOutputDevice(endpoint.id)` and reports the resolved name
  // back. We deliberately do not pre-fill `monitorDevice` here — doing so would
  // hide the fact that nothing is bound yet (contract §7).
  useEffect(() => {
    getSystemAudioDevices()
      .then((res) => {
        if (res?.outputs?.length) setOutputDevices(res.outputs);
      })
      .catch(() => {});
  }, []);

  // Adopt the authoritative engine topology (numeric channel ids + mixes).
  // Resolves `true` when the daemon answered with a usable topology, so callers can retry.
  const syncFromEngine = React.useCallback((): Promise<boolean> => {
    return getEngineState()
      .then((state) => {
        if (!state || !Array.isArray(state.channels)) return false;
        setDaemonConnected(true);
        setMatrixState((prev) => {
          const engineChannels: ChannelNode[] = state.channels.map((ec) => {
            const existing =
              prev.channels.find((c) => c.channelId === ec.id) ||
              prev.channels.find((c) => c.name === ec.name);
            return {
              id: existing?.id || `ch_engine_${ec.id}`,
              channelId: ec.id,
              name: ec.name,
              type: existing?.type || (ec.channel_type === "Microphone" ? "hardware" : "app"),
              pid: ec.pid || existing?.pid,
              processName: existing?.processName,
              deviceEndpointId: existing?.deviceEndpointId,
              gain: ec.gain,
              isMuted: ec.is_muted,
              vst3Slots: existing?.vst3Slots,
            };
          });

          // Ensure System channel is always present and pinned at index 0
          const sysIdx = engineChannels.findIndex(
            (c) => c.id === "ch_system" || c.name.toLowerCase() === "system"
          );
          if (sysIdx === -1) {
            const existingSys = prev.channels.find(
              (c) => c.id === "ch_system" || c.name.toLowerCase() === "system"
            );
            engineChannels.unshift(
              existingSys || {
                id: "ch_system",
                channelId: null,
                name: "System",
                type: "system",
                gain: 1.0,
                isMuted: false,
              }
            );
          } else if (sysIdx > 0) {
            const [sys] = engineChannels.splice(sysIdx, 1);
            engineChannels.unshift(sys);
          }

          // Preserve any channels that were added locally in UI
          prev.channels.forEach((pc) => {
            if (!engineChannels.some((ec) => ec.id === pc.id || ec.name.toLowerCase() === pc.name.toLowerCase())) {
              engineChannels.push(pc);
            }
          });

          const nextRoutes = { ...prev.routes };
          engineChannels.forEach((ch) => {
            if (!nextRoutes[ch.id]) nextRoutes[ch.id] = {};
          });

          // Adopt the daemon's authoritative send state, otherwise every cross-point renders as
          // inactive even though the engine is routing audio — the matrix would contradict the
          // meters. Engine `sends[].mix_id` is the same `mix_idx` the mix bus uses, and the UI
          // mix order matches it, so index lookup is exact.
          const engineByLocalId = new Map<string, EngineChannelDto>();
          state.channels.forEach((ec) => {
            const local =
              prev.channels.find((c) => c.channelId === ec.id) ||
              prev.channels.find((c) => c.name === ec.name);
            if (local) engineByLocalId.set(local.id, ec);
          });

          engineByLocalId.forEach((ec, localId) => {
            const sends = ec.sends;
            if (!Array.isArray(sends)) return;
            sends.forEach((send) => {
              const mix = prev.mixes[send.mix_id];
              if (!mix) return;
              nextRoutes[localId] = nextRoutes[localId] || {};
              nextRoutes[localId][mix.id] = {
                active: !send.is_muted && send.gain > 0,
                gain: send.gain,
                isMuted: send.is_muted,
              };
            });
          });

          return {
            ...prev,
            channels: engineChannels.length > 0 ? engineChannels : prev.channels,
            routes: nextRoutes,
            monitorDevice: state.output_device_name || prev.monitorDevice,
            monitorEndpointId: state.output_device_id || prev.monitorEndpointId,
          };
        });
        return true;
      })
      .catch((err) => {
        // Surface the reason instead of failing silently: a swallowed error here is what let
        // the matrix keep showing stale local state while the daemon was unreachable.
        console.warn("getEngineState failed; keeping local topology:", err);
        setDaemonConnected(false);
        return false;
      });
  }, []);

  // Sync on mount, then retry until the daemon answers.
  //
  // A single mount-time call races the daemon: the app spawns it and the UI mounts in the same
  // second, so the first `getEngineState` can fail. Without a retry the matrix would keep
  // showing the localStorage default ("System") instead of the real channels indefinitely.
  // The retry stops as soon as one sync succeeds, or after a bounded number of attempts so a
  // genuinely absent daemon does not spin forever.
  useEffect(() => {
    let attempts = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cancelled = false;

    const attempt = () => {
      if (cancelled) return;
      attempts += 1;
      syncFromEngine().then((ok) => {
        if (cancelled || ok || attempts >= 20) return;
        timer = setTimeout(attempt, 500);
      });
    };

    attempt();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [syncFromEngine]);

  // Re-sync when the daemon (re)connects — picks up a daemon restart.
  useEffect(() => {
    if (daemonConnected) syncFromEngine();
  }, [daemonConnected, syncFromEngine]);

  // Auto PID capture: attach a real process to an app channel by exe name
  useEffect(() => {
    const scanInterval = setInterval(() => {
      getActiveAudioSessions()
        .then((sessions) => {
          if (!sessions || sessions.length === 0) return;
          setMatrixState((prev) => {
            let changed = false;
            const updatedChannels = prev.channels.map((ch) => {
              if (ch.type !== "app" || !ch.processName || ch.channelId === null) return ch;
              const procLower = ch.processName.toLowerCase();
              const matched = sessions.find((s) => {
                if (!s.is_active) return false;
                const sName = s.name.toLowerCase();
                const sProc = s.process_name.toLowerCase();
                return sProc === procLower || sName === procLower;
              });
              if (matched && matched.pid !== ch.pid) {
                changed = true;
                // The daemon attaches the capture when the channel is (re)created.
                return { ...ch, pid: matched.pid };
              }
              return ch;
            });

            if (!changed) return prev;
            return { ...prev, channels: updatedChannels };
          });
        })
        .catch(() => {});
    }, 1500);

    return () => clearInterval(scanInterval);
  }, []);

  // Shared-memory telemetry (VU meters) + daemon status
  useEffect(() => {
    let unlistenVu: (() => void) | undefined;
    let unlistenStatus: (() => void) | undefined;

    listenEvent<TelemetrySnapshotDto>("vu-meter-frame", (frame) => {
      frameCountRef.current += 1;
      setChannelMeters(frame.channels || []);
      setMixMeters(frame.mixes || []);
    }).then((un) => {
      unlistenVu = un;
    });

    listenEvent<boolean>("daemon-connection-status", (status) => {
      setDaemonConnected(status);
    }).then((un) => {
      unlistenStatus = un;
    });

    // Browser-dev fallback polling
    const interval = setInterval(() => {
      getTelemetrySnapshot()
        .then((snapshot) => {
          if (snapshot?.channels?.length) {
            frameCountRef.current += 1;
            setChannelMeters(snapshot.channels);
            setMixMeters(snapshot.mixes);
          }
        })
        .catch(() => {});
    }, 16);

    return () => {
      unlistenVu?.();
      unlistenStatus?.();
      clearInterval(interval);
    };
  }, []);

  // ---------------------------------------------------------------------------
  // Channels
  // ---------------------------------------------------------------------------
  const handleAddChannel = (newCh: {
    name: string;
    type: ChannelNode["type"];
    pid?: number;
    processName?: string;
    deviceEndpointId?: string;
    iconUrl?: string;
  }) => {
    const localId = `ch_${Date.now()}_${Math.random().toString(36).substring(2, 6)}`;
    const created: ChannelNode = {
      id: localId,
      channelId: null,
      name: newCh.name,
      type: newCh.type,
      pid: newCh.pid,
      processName: newCh.processName,
      deviceEndpointId: newCh.deviceEndpointId,
      gain: 1.0,
      isMuted: false,
      iconUrl: newCh.iconUrl,
    };

    // A new channel starts routed into every existing mix at unity gain.
    const updatedRoutes = { ...routes };
    updatedRoutes[localId] = {};
    mixes.forEach((mix) => {
      updatedRoutes[localId][mix.id] = { active: true, gain: 1.0, isMuted: false };
    });

    setMatrixState((prev) => ({
      ...prev,
      channels: [...prev.channels, created],
      routes: updatedRoutes,
    }));

    // The daemon assigns the authoritative numeric channel id.
    addChannel(newCh.name, toChannelType(newCh.type), newCh.pid ?? 0)
      .then((channelId) => {
        if (typeof channelId !== "number") return;
        setMatrixState((prev) => ({
          ...prev,
          channels: prev.channels.map((c) =>
            c.id === localId ? { ...c, channelId } : c
          ),
        }));
      })
      .catch((err) => {
        console.warn("addChannel notice:", err);
      });

    // Inputs create cables ONLY in Outputs in Windows (:out suffix), and bind the app
    registerCable(newCh.name, true, newCh.pid);
  };

  /**
   * Register `name` as a virtual audio cable and sync the whole cable set.
   * isInput: true creates Playback/Outputs only in Windows.
   * isInput: false creates both Outputs and Inputs in Windows.
   */
  const registerCable = (
    name: string,
    isInput: boolean = false,
    targetPid?: number,
    isSystemDefault: boolean = false
  ) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    const cableName = isInput ? `${trimmed}:out` : trimmed;
    if (!desiredCableNames.includes(cableName)) {
      desiredCableNames.push(cableName);
    }

    createCables([...desiredCableNames])
      .then((cables) => {
        console.info(`virtual cables ready: ${cables.map((c) => c.name).join(", ")}`);
        // If an app channel was created with a PID, bind it immediately in Windows!
        if (targetPid && targetPid > 0) {
          const match = cables.find((c) => c.name === cableName || c.name === trimmed);
          if (match?.render) {
            bindAppAudio(targetPid, match.render)
              .then(() => console.info(`Bound PID ${targetPid} to Windows output device ${match.render}`))
              .catch((err) => console.warn(`bindAppAudio failed for PID ${targetPid}:`, err));
          }
        }
        // If System channel, set it as Windows default output device!
        if (isSystemDefault || trimmed.toLowerCase() === "system") {
          const match = cables.find((c) => c.name === cableName || c.name === trimmed);
          if (match?.render) {
            setDefaultPlaybackDevice(match.render)
              .then(() => console.info(`Set System virtual cable as default Windows audio output: ${match.render}`))
              .catch((err) => console.warn("setDefaultPlaybackDevice failed:", err));
          }
        }
      })
      .catch((err) => {
        console.error("createCables failed:", err);
      });
  };

  /** Drop a cable when its channel or mix is removed. */
  const unregisterCable = (name: string) => {
    const trimmed = name.trim();
    if (trimmed.toLowerCase() === "system") return; // System cable is never removed
    const clean = trimmed.replace(/:out$/, "");
    const toRemove = [trimmed, `${clean}:out`, clean];
    let changed = false;
    toRemove.forEach((item) => {
      const idx = desiredCableNames.indexOf(item);
      if (idx >= 0) {
        desiredCableNames.splice(idx, 1);
        changed = true;
      }
    });
    if (!changed) return;

    createCables([...desiredCableNames])
      .then(() => console.info(`virtual cable '${trimmed}' removed`))
      .catch((err) => console.error("createCables (remove) failed:", err));
  };

  const handleRemoveChannel = (chId: string) => {
    if (channels.length <= 1) return;
    const target = channels.find((c) => c.id === chId);
    if (!target) return;
    // System channel is locked and cannot be deleted
    if (target.id === "ch_system" || target.name.toLowerCase() === "system") {
      console.warn("System channel cannot be removed");
      return;
    }

    setMatrixState((prev) => {
      const nextRoutes = { ...prev.routes };
      delete nextRoutes[chId];
      return {
        ...prev,
        channels: prev.channels.filter((c) => c.id !== chId),
        routes: nextRoutes,
      };
    });

    if (target.channelId !== null && target.channelId !== undefined) {
      removeChannel(target.channelId).catch((err) =>
        console.error("removeChannel failed:", err)
      );
    }

    // The channel's cable goes with it
    unregisterCable(target.name);
  };

  const handleReorderChannels = (fromIdx: number, toIdx: number) => {
    if (fromIdx === toIdx) return;
    setMatrixState((prev) => {
      const nextChannels = [...prev.channels];
      const [moved] = nextChannels.splice(fromIdx, 1);
      nextChannels.splice(toIdx, 0, moved);

      // System channel must stay at index 0
      const sysIdx = nextChannels.findIndex(
        (c) => c.id === "ch_system" || c.name.toLowerCase() === "system"
      );
      if (sysIdx > 0) {
        const [sys] = nextChannels.splice(sysIdx, 1);
        nextChannels.unshift(sys);
      }

      return {
        ...prev,
        channels: nextChannels,
      };
    });
  };

  const handleChannelGainChange = (chId: string, newGain: number) => {
    setMatrixState((prev) => ({
      ...prev,
      channels: prev.channels.map((c) => (c.id === chId ? { ...c, gain: newGain } : c)),
    }));
    const target = channels.find((c) => c.id === chId);
    if (target?.channelId !== null && target?.channelId !== undefined) {
      setChannelGain(target.channelId, newGain).catch(console.error);
    }
  };

  const handleChannelMuteToggle = (chId: string) => {
    const target = channels.find((c) => c.id === chId);
    if (!target) return;
    const nextMuted = !target.isMuted;
    setMatrixState((prev) => ({
      ...prev,
      channels: prev.channels.map((c) => (c.id === chId ? { ...c, isMuted: nextMuted } : c)),
    }));
    if (target.channelId !== null && target.channelId !== undefined) {
      setChannelMute(target.channelId, nextMuted).catch(console.error);
    }
  };

  const handleChannelRename = (chId: string, newName: string) => {
    if (!newName.trim()) {
      setEditingChannelId(null);
      return;
    }
    setMatrixState((prev) => ({
      ...prev,
      channels: prev.channels.map((c) => (c.id === chId ? { ...c, name: newName.trim() } : c)),
    }));
    setEditingChannelId(null);
  };

  // ---------------------------------------------------------------------------
  // Mixes
  // ---------------------------------------------------------------------------
  const handleAddMix = (e: React.FormEvent) => {
    e.preventDefault();
    const name = newMixName.trim() || `Mix ${mixes.length + 1}`;
    const mixId = `mix_${Date.now()}_${Math.random().toString(36).substring(2, 6)}`;
    const created: MixBus = {
      id: mixId,
      index: mixes.length,
      name,
      iconType: "custom",
      volume: 1.0,
      isMuted: false,
      outputs: [],
    };

    const updatedRoutes = { ...routes };
    channels.forEach((ch) => {
      if (!updatedRoutes[ch.id]) updatedRoutes[ch.id] = {};
      updatedRoutes[ch.id][mixId] = { active: false, gain: 1.0, isMuted: false };
    });

    setMatrixState((prev) => ({
      ...prev,
      mixes: [...prev.mixes, created],
      routes: updatedRoutes,
    }));
    setNewMixName("");
    setIsAddingMix(false);

    // A mix becomes a real Windows cable too, so it can be selected as a
    // microphone in Discord/OBS exactly like an input channel.
    registerCable(name);
  };

  const handleDeleteMix = (mixId: string) => {
    if (mixes.length <= 1) return;
    const doomed = mixes.find((m) => m.id === mixId);
    setMatrixState((prev) => {
      const nextMixes = prev.mixes.filter((m) => m.id !== mixId);
      const nextRoutes = { ...prev.routes };
      Object.keys(nextRoutes).forEach((chId) => {
        delete nextRoutes[chId][mixId];
      });
      const nextListening =
        prev.listeningMixId === mixId ? nextMixes[0]?.id || "mix_personal" : prev.listeningMixId;
      return {
        ...prev,
        mixes: nextMixes,
        routes: nextRoutes,
        listeningMixId: nextListening,
      };
    });

    if (doomed) unregisterCable(doomed.name);
  };

  const handleUpdateMix = (updated: MixBus) => {
    setMatrixState((prev) => ({
      ...prev,
      mixes: prev.mixes.map((m) => (m.id === updated.id ? updated : m)),
    }));
    setMixGain(updated.index, updated.volume).catch(console.error);
    setMixMute(updated.index, updated.isMuted).catch(console.error);
  };

  // ---------------------------------------------------------------------------
  // Cross-points
  // ---------------------------------------------------------------------------
  const handleCrossPointActivate = (chId: string, mixId: string, mixIndex: number) => {
    const target = channels.find((c) => c.id === chId);
    setMatrixState((prev) => {
      const current = prev.routes[chId]?.[mixId] || { active: false, gain: 1.0, isMuted: false };
      return {
        ...prev,
        routes: {
          ...prev.routes,
          [chId]: { ...prev.routes[chId], [mixId]: { ...current, active: true } },
        },
      };
    });
    if (target?.channelId !== null && target?.channelId !== undefined) {
      setSendGain(target.channelId, mixIndex, 1.0).catch(console.error);
      setSendMute(target.channelId, mixIndex, false).catch(console.error);
    }
  };

  const handleCrossPointDeactivate = (chId: string, mixId: string, mixIndex: number) => {
    const target = channels.find((c) => c.id === chId);
    setMatrixState((prev) => {
      const current = prev.routes[chId]?.[mixId] || { active: true, gain: 1.0, isMuted: false };
      return {
        ...prev,
        routes: {
          ...prev.routes,
          [chId]: { ...prev.routes[chId], [mixId]: { ...current, active: false } },
        },
      };
    });
    if (target?.channelId !== null && target?.channelId !== undefined) {
      setSendGain(target.channelId, mixIndex, 0.0).catch(console.error);
    }
  };

  const handleCrossPointGain = (chId: string, mixId: string, mixIndex: number, gain: number) => {
    const target = channels.find((c) => c.id === chId);
    setMatrixState((prev) => ({
      ...prev,
      routes: {
        ...prev.routes,
        [chId]: {
          ...prev.routes[chId],
          [mixId]: {
            ...(prev.routes[chId]?.[mixId] || { active: true, isMuted: false }),
            gain,
          },
        },
      },
    }));
    if (target?.channelId !== null && target?.channelId !== undefined) {
      setSendGain(target.channelId, mixIndex, gain).catch(console.error);
    }
  };

  const handleCrossPointMuteToggle = (chId: string, mixId: string, mixIndex: number) => {
    const target = channels.find((c) => c.id === chId);
    const current = routes[chId]?.[mixId]?.isMuted || false;
    const nextMuted = !current;
    setMatrixState((prev) => ({
      ...prev,
      routes: {
        ...prev.routes,
        [chId]: {
          ...prev.routes[chId],
          [mixId]: {
            ...(prev.routes[chId]?.[mixId] || { active: true, gain: 1.0 }),
            isMuted: nextMuted,
          },
        },
      },
    }));
    if (target?.channelId !== null && target?.channelId !== undefined) {
      setSendMute(target.channelId, mixIndex, nextMuted).catch(console.error);
    }
  };

  // VST3 rack
  const handleVstSlotsUpdate = (chId: string, slots: Vst3SlotDto[]) => {
    setMatrixState((prev) => ({
      ...prev,
      channels: prev.channels.map((c) => (c.id === chId ? { ...c, vst3Slots: slots } : c)),
    }));
  };

  const getChannelIcon = (ch: ChannelNode) => {
    if (appIcons[ch.id]) {
      return (
        <img
          src={appIcons[ch.id]}
          alt=""
          className="w-7 h-7 object-contain rounded-md"
        />
      );
    }
    if (ch.type === "system") return <Monitor className="w-5 h-5 text-[#9ca3af]" />;
    if (ch.type === "hardware") return <Mic className="w-5 h-5 text-[#9ca3af]" />;
    return <AppWindow className="w-5 h-5 text-[#9ca3af]" />;
  };

  const getMixIcon = (mix: MixBus) => {
    const cls = "w-4 h-4 shrink-0 text-[#9ca3af]";
    switch (mix.iconType) {
      case "stream":
        return <Radio className={cls} />;
      case "record":
        return <Mic className={cls} />;
      case "aux":
        return <Sliders className={cls} />;
      default:
        return <Headphones className={cls} />;
    }
  };

  return (
    <div className="flex flex-col h-screen w-screen bg-[#111113] text-[#f4f4f5] overflow-hidden select-none font-sans">
      <HeaderBar
        mixes={mixes}
        listeningMixId={listeningMixId}
        onSelectListeningMix={(id) => setMatrixState((prev) => ({ ...prev, listeningMixId: id }))}
        monitorDevice={monitorDevice}
        onMonitorDeviceResolved={(endpointId, resolvedName) =>
          setMatrixState((prev) => ({
            ...prev,
            monitorEndpointId: endpointId,
            monitorDevice: resolvedName,
          }))
        }
        daemonConnected={daemonConnected}
        fps={fps}
        onResetDefaults={() => setMatrixState(DEFAULT_MATRIX_STATE)}
      />

      <main className="flex-1 overflow-auto p-4 flex flex-col">
        <div className="inline-block min-w-full">
          {/* Header row: inputs label + mix headers + add-mix */}
          <div className="flex items-end gap-3 pb-3 border-b border-white/[0.08]">
            <div className="w-[410px] shrink-0 px-3 pb-1">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-[#a1a1aa]">
                Inputs ({channels.length})
              </span>
            </div>

            <div className="flex items-center gap-3">
              {mixes.map((mix) => {
                const isListening = mix.id === listeningMixId;
                const mixMeter = mixMeters[mix.index];
                const mixPeak = mix.isMuted ? 0 : Math.max(mixMeter?.peak_l || 0, mixMeter?.peak_r || 0);
                const mixPct = Math.min(100, Math.max(0, mixPeak * 100));

                return (
                  <div
                    key={mix.id}
                    onClick={() => setSelectedMixForModal(mix)}
                    className="group relative overflow-hidden w-56 h-[72px] bg-[#1c1c20] hover:bg-[#222226] border border-white/[0.08] rounded-xl p-3 flex flex-col justify-between cursor-pointer transition-colors"
                    title="Configure mix outputs and volume"
                  >
                    {/* The ONLY listening indicator: a 3px #ffffff strip on the top edge. */}
                    {isListening && (
                      <div className="absolute top-0 left-0 right-0 h-[3px] bg-[#ffffff]" />
                    )}

                    {/* Delete mix cross button in top-right corner */}
                    {mixes.length > 1 && (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          handleDeleteMix(mix.id);
                        }}
                        className="absolute top-2 right-2 w-5 h-5 rounded hover:bg-red-500/20 text-[#71717a] hover:text-[#ef4444] flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity z-10"
                        title="Delete mix bus"
                      >
                        <X className="w-3.5 h-3.5" />
                      </button>
                    )}

                    <div className="flex items-center justify-between pr-4">
                      <div className="flex items-center gap-2 min-w-0">
                        {getMixIcon(mix)}
                        <span className="text-sm font-semibold text-[#f4f4f5] truncate max-w-[130px]">
                          {mix.name}
                        </span>
                      </div>
                      <span className="text-[11px] font-mono text-[#a1a1aa]">
                        {Math.round(mix.volume * 100)}%
                      </span>
                    </div>

                    {/* Green VU level line running across under mix header (Wave Link style) */}
                    <div className="w-full h-1 bg-[#141416] rounded-full overflow-hidden mt-1">
                      <div
                        className="h-full rounded-full transition-[width] duration-75 ease-out"
                        style={{
                          width: `${mixPct}%`,
                          backgroundColor: mixMeter?.clipping ? "#ef4444" : "#22c55e",
                        }}
                      />
                    </div>

                    <div className="flex items-center justify-between text-[10px] text-[#71717a] font-mono mt-0.5">
                      <span>
                        {mix.outputs.length} {mix.outputs.length === 1 ? "output" : "outputs"}
                      </span>
                    </div>
                  </div>
                );
              })}

              {/* `+` new mix */}
              <div className="relative">
                {isAddingMix ? (
                  <form
                    onSubmit={handleAddMix}
                    className="w-56 h-[72px] bg-[#1c1c20] border border-white/[0.08] rounded-xl p-2.5 flex items-center gap-1.5"
                  >
                    <input
                      type="text"
                      autoFocus
                      value={newMixName}
                      onChange={(e) => setNewMixName(e.target.value)}
                      placeholder="Mix name..."
                      className="w-full bg-[#141416] border border-[#2a2a2f] text-xs text-[#f4f4f5] px-2.5 py-1.5 rounded-lg focus:outline-none focus:border-[#44444c]"
                    />
                    <button
                      type="submit"
                      className="px-2.5 py-1.5 bg-[#28282e] hover:bg-[#323238] text-[#f4f4f5] rounded-lg text-xs font-medium shrink-0 transition-colors"
                    >
                      Add
                    </button>
                    <button
                      type="button"
                      onClick={() => setIsAddingMix(false)}
                      className="p-1 text-[#71717a] hover:text-[#f4f4f5] shrink-0"
                      title="Cancel"
                    >
                      <X className="w-3.5 h-3.5" />
                    </button>
                  </form>
                ) : (
                  <button
                    type="button"
                    onClick={() => {
                      setNewMixName("");
                      setIsAddingMix(true);
                    }}
                    className="w-14 h-[72px] rounded-xl bg-[#18181b] hover:bg-[#222226] border border-white/[0.08] hover:border-white/[0.14] flex items-center justify-center text-[#71717a] hover:text-[#f4f4f5] transition-colors"
                    title="Add new mix bus"
                  >
                    <Plus className="w-5 h-5" />
                  </button>
                )}
              </div>
            </div>
          </div>

          {/* Matrix rows */}
          <div className="space-y-3 pt-3">
            {channels.map((channel, chIndex) => {
              const isEditing = editingChannelId === channel.id;
              const meter = channelMeters[chIndex];

              return (
                <div
                  key={channel.id}
                  className={`flex items-center gap-3 transition-opacity ${
                    draggingIndex === chIndex ? "opacity-40" : ""
                  }`}
                  draggable={channel.id !== "ch_system"}
                  onDragStart={(e) => {
                    setDraggingIndex(chIndex);
                    e.dataTransfer.setData("text/plain", chIndex.toString());
                  }}
                  onDragOver={(e) => e.preventDefault()}
                  onDrop={(e) => {
                    e.preventDefault();
                    const from = parseInt(e.dataTransfer.getData("text/plain"), 10);
                    if (!isNaN(from) && from !== chIndex) {
                      handleReorderChannels(from, chIndex);
                    }
                    setDraggingIndex(null);
                  }}
                  onDragEnd={() => setDraggingIndex(null)}
                >
                  {/* Input row card (72px, 410px wide) */}
                  <div
                    className={`relative w-[410px] h-[72px] border rounded-xl px-3 py-2 flex items-center justify-between gap-2.5 transition-colors shrink-0 ${
                      channel.id !== "ch_system" ? "cursor-grab active:cursor-grabbing" : ""
                    } ${
                      channel.isMuted
                        ? "bg-[#2e1417] border-[#ef4444]"
                        : "bg-[#18181b] hover:bg-[#1f1f23] border-white/[0.08]"
                    }`}
                  >
                    {/* Delete channel cross button (System is permanent & protected) */}
                    {channel.id !== "ch_system" && channel.name.toLowerCase() !== "system" && channels.length > 1 && (
                      <button
                        type="button"
                        onClick={() => handleRemoveChannel(channel.id)}
                        className="absolute top-2 right-2 w-5 h-5 rounded hover:bg-red-500/20 text-[#71717a] hover:text-[#ef4444] flex items-center justify-center transition-colors z-10"
                        title="Delete channel"
                      >
                        <X className="w-3.5 h-3.5" />
                      </button>
                    )}

                    {/* App icon */}
                    <div className="w-10 h-10 rounded-xl bg-[#141416] border border-white/[0.08] flex items-center justify-center shrink-0 overflow-hidden">
                      {getChannelIcon(channel)}
                    </div>

                    {/* Channel name & process info */}
                    <div className="flex-1 min-w-0 pr-1">
                      {channel.id === "ch_system" || channel.name.toLowerCase() === "system" ? (
                        <div className="flex items-center gap-1.5">
                          <span className="text-sm font-semibold text-[#f4f4f5] truncate leading-tight">
                            System
                          </span>
                          <span className="text-[9px] font-mono px-1 py-0.2 rounded bg-white/[0.08] text-[#a1a1aa] uppercase">
                            Default
                          </span>
                        </div>
                      ) : isEditing ? (
                        <input
                          type="text"
                          autoFocus
                          value={editingChannelName}
                          onChange={(e) => setEditingChannelName(e.target.value)}
                          onBlur={() => handleChannelRename(channel.id, editingChannelName)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") handleChannelRename(channel.id, editingChannelName);
                            if (e.key === "Escape") setEditingChannelId(null);
                          }}
                          className="w-full bg-[#141416] text-xs font-semibold text-[#f4f4f5] px-1.5 py-0.5 rounded-md border border-[#2a2a2f] focus:outline-none focus:border-[#44444c]"
                        />
                      ) : (
                        <div
                          onDoubleClick={() => {
                            setEditingChannelId(channel.id);
                            setEditingChannelName(channel.name);
                          }}
                          className="text-sm font-semibold text-[#f4f4f5] truncate cursor-pointer leading-tight"
                          title="Double-click to rename"
                        >
                          {channel.name}
                        </div>
                      )}
                      <div className="text-[11px] text-[#a1a1aa] font-mono truncate mt-0.5">
                        {channel.type === "hardware"
                          ? `Input "${channel.name}" | OpenWaveLink`
                          : `Output "${channel.name}" | OpenWaveLink`}
                      </div>
                    </div>

                    {/* Master gain fader with green VU bar */}
                    <div className="w-28 shrink-0 flex flex-col justify-center">
                      <div className="text-[10px] font-mono text-[#a1a1aa] text-right leading-none mb-1.5">
                        {Math.round(channel.gain * 100)}%
                      </div>
                      <div className="relative w-full h-4 flex items-center">
                        <div className="absolute inset-0 bg-[#141416] rounded-full overflow-hidden border border-white/[0.06]">
                          <div
                            className="h-full rounded-full transition-[width] duration-75 ease-out"
                            style={{
                              width: `${channel.isMuted ? 0 : Math.min(100, Math.max(0, (meter?.peak_l || 0) * 100))}%`,
                              backgroundColor: meter?.clipping ? "#ef4444" : "#22c55e",
                            }}
                          />
                        </div>
                        <input
                          type="range"
                          min="0"
                          max="1.5"
                          step="0.01"
                          value={channel.gain}
                          disabled={channel.isMuted}
                          onChange={(e) => handleChannelGainChange(channel.id, parseFloat(e.target.value))}
                          className="wl-slider w-full cursor-pointer relative z-10 !bg-transparent"
                          title={`Master gain: ${Math.round(channel.gain * 100)}%`}
                        />
                      </div>
                    </div>

                    {/* Master mute button */}
                    <button
                      type="button"
                      onClick={() => handleChannelMuteToggle(channel.id)}
                      className={`w-8 h-8 rounded-lg flex items-center justify-center shrink-0 transition-colors border ${
                        channel.isMuted
                          ? "bg-[#331418] border-[#ef4444] text-[#ef4444]"
                          : "bg-[#141416] hover:bg-[#202024] border-white/[0.08] text-[#a1a1aa] hover:text-[#f4f4f5]"
                      }`}
                      title={channel.isMuted ? "Unmute channel" : "Mute channel"}
                    >
                      {channel.isMuted ? (
                        <VolumeX className="w-4 h-4" />
                      ) : (
                        <Volume2 className="w-4 h-4" />
                      )}
                    </button>
                  </div>

                  {/* Cross-points for this row */}
                  <div className="flex items-center gap-3">
                    {mixes.map((mix) => {
                      const state = routes[channel.id]?.[mix.id];
                      return (
                        <div key={mix.id} className="w-56 shrink-0">
                          <CrossPointCell
                            channelId={channel.id}
                            mixId={mix.id}
                            channelName={channel.name}
                            mixName={mix.name}
                            channelMasterMuted={channel.isMuted}
                            meterLevel={meter?.peak_l || 0}
                            state={state}
                            onActivate={() => handleCrossPointActivate(channel.id, mix.id, mix.index)}
                            onDeactivate={() =>
                              handleCrossPointDeactivate(channel.id, mix.id, mix.index)
                            }
                            onGainChange={(g) =>
                              handleCrossPointGain(channel.id, mix.id, mix.index, g)
                            }
                            onMuteToggle={() =>
                              handleCrossPointMuteToggle(channel.id, mix.id, mix.index)
                            }
                          />
                        </div>
                      );
                    })}
                  </div>
                </div>
              );
            })}

            {/* `+ Create channel` — strictly opens downward */}
            <div className="relative pt-2">
              <button
                type="button"
                onClick={() => setIsCreateChannelOpen(!isCreateChannelOpen)}
                className="w-[410px] h-11 rounded-xl bg-[#18181b] hover:bg-[#202024] border border-white/[0.08] hover:border-white/[0.14] flex items-center justify-center gap-2 text-xs font-semibold text-[#f4f4f5] transition-colors"
              >
                <Plus className="w-4 h-4 text-[#a1a1aa]" />
                <span>Create channel</span>
              </button>

              <CreateChannelDropdown
                isOpen={isCreateChannelOpen}
                onClose={() => setIsCreateChannelOpen(false)}
                onAddChannel={handleAddChannel}
              />
            </div>
          </div>
        </div>
      </main>

      <MixSettingsModal
        isOpen={Boolean(selectedMixForModal)}
        mix={selectedMixForModal}
        canDelete={mixes.length > 1}
        availableOutputs={outputDevices}
        onClose={() => setSelectedMixForModal(null)}
        onUpdateMix={handleUpdateMix}
        onDeleteMix={handleDeleteMix}
      />
    </div>
  );
};
