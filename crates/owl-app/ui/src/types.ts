import { Vst3SlotDto, saveAppConfigFile } from "./ipc";

export type ChannelSourceType = "system" | "hardware" | "app" | "empty";

/**
 * An input channel row.
 *
 * `channelId` is the AUTHORITATIVE numeric id assigned by the daemon through
 * `addChannel()` (INTERNAL_ENGINE_CONTRACT.md §4/§5). Every later control call
 * — `setChannelGain`, `setChannelMute`, `setSendGain`, `setSendMute` — must
 * send THIS number back, never the array index.
 *
 * `id` stays a React-only key (stable across re-renders, usable before the
 * daemon answers), and `channelId` is null until the backend has assigned one.
 */
export interface ChannelNode {
  id: string;
  channelId: number | null;
  name: string;
  type: ChannelSourceType;
  pid?: number;
  processName?: string;
  deviceEndpointId?: string;
  gain: number; // 0.0 .. 1.5 (1.0 = 0 dB)
  isMuted: boolean;
  vst3Slots?: Vst3SlotDto[];
  iconUrl?: string;
}

/**
 * An output mix bus.
 *
 * `index` is the daemon-side `mix_idx` used by `setMixGain` / `setMixMute`.
 */
export interface MixBus {
  id: string;
  index: number;
  name: string;
  iconType?: "personal" | "stream" | "record" | "aux" | "custom";
  volume: number; // 0.0 .. 1.0
  isMuted: boolean;
  outputs: string[];
}

export interface CrossPointState {
  active: boolean; // направлен ли канал в данный микс
  gain: number;    // Send Gain [0.0 .. 1.5]
  isMuted: boolean; // Mute конкретно в этом миксе
}

export type MatrixRoutingState = Record<string, Record<string, CrossPointState>>;

export interface AppMatrixState {
  channels: ChannelNode[];
  mixes: MixBus[];
  routes: MatrixRoutingState;
  listeningMixId: string;
  monitorDevice: string;
  /** Real IMMDeviceEnumerator endpoint id currently bound to the monitor output. */
  monitorEndpointId: string;
}

/**
 * First-run topology from the spec (§2): exactly ONE input channel (`System`)
 * and ONE output mix (`Personal Mix`). No mock app entries — every other
 * channel/mix is created by the user from real Windows 11 data.
 */
export const DEFAULT_MATRIX_STATE: AppMatrixState = {
  channels: [
    {
      id: "ch_system",
      channelId: null,
      name: "System",
      type: "system",
      gain: 1.0,
      isMuted: false,
    },
  ],
  mixes: [
    {
      id: "mix_personal",
      index: 0,
      name: "Personal Mix",
      iconType: "personal",
      volume: 1.0,
      isMuted: false,
      outputs: [],
    },
  ],
  routes: {
    ch_system: {
      mix_personal: {
        active: true,
        gain: 1.0,
        isMuted: false,
      },
    },
  },
  listeningMixId: "mix_personal",
  monitorDevice: "",
  monitorEndpointId: "",
};

const STORAGE_KEY = "openwavelink_matrix_v4";

export function loadPersistedState(): AppMatrixState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_MATRIX_STATE;
    const parsed = JSON.parse(raw) as Partial<AppMatrixState>;
    if (!parsed.channels || !parsed.mixes || parsed.channels.length === 0 || parsed.mixes.length === 0) {
      return DEFAULT_MATRIX_STATE;
    }
    const restoredChannels = parsed.channels.map((c, i) => ({
      ...c,
      channelId: typeof c.channelId === "number" ? c.channelId : null,
      id: c.id || `ch_restored_${i}`,
    }));

    // Ensure System channel is always present and pinned at index 0
    const sysIdx = restoredChannels.findIndex(
      (c) => c.id === "ch_system" || c.name.toLowerCase() === "system"
    );
    let channels: ChannelNode[];
    if (sysIdx === -1) {
      channels = [
        {
          id: "ch_system",
          channelId: null,
          name: "System",
          type: "system",
          gain: 1.0,
          isMuted: false,
        },
        ...restoredChannels,
      ];
    } else {
      const [sys] = restoredChannels.splice(sysIdx, 1);
      channels = [
        { ...sys, id: "ch_system", name: "System", type: "system" },
        ...restoredChannels,
      ];
    }

    return {
      channels,
      mixes: parsed.mixes.map((m, i) => ({
        ...m,
        index: typeof m.index === "number" ? m.index : i,
        id: m.id || `mix_restored_${i}`,
      })),
      routes: parsed.routes || {},
      listeningMixId: parsed.listeningMixId || parsed.mixes[0]?.id || "mix_personal",
      monitorDevice: parsed.monitorDevice || "",
      monitorEndpointId: parsed.monitorEndpointId || "",
    };
  } catch (err) {
    console.warn("Failed to load persisted state, using default:", err);
    return DEFAULT_MATRIX_STATE;
  }
}

export function savePersistedState(state: AppMatrixState): void {
  try {
    const json = JSON.stringify(state, null, 2);
    localStorage.setItem(STORAGE_KEY, json);
    saveAppConfigFile(json).catch(() => {});
  } catch (err) {
    console.warn("Failed to persist state:", err);
  }
}
