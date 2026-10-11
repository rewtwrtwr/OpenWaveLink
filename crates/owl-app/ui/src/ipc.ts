// Tauri 2.x IPC Bridge with browser-dev fallback

export interface ChannelMeterDto {
  peak_l: number;
  peak_r: number;
  rms_l: number;
  rms_r: number;
  clipping: boolean;
}

export interface TelemetrySnapshotDto {
  seq: number;
  sample_rate: number;
  channels: ChannelMeterDto[];
  mixes: ChannelMeterDto[];
}

export interface HudNotificationDto {
  title: string;
  detail: string;
  volume: number;
  is_muted: boolean;
  icon: string;
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

const isTauri = () => typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);

export async function invokeCommand<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  // Browser fallback stub
  console.debug(`[Tauri Mock Invoke] ${cmd}:`, args);
  return {} as T;
}

export async function listenEvent<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<T>(event, (ev) => handler(ev.payload));
    return unlisten;
  }
  return () => {};
}

export const setChannelGain = (channel_idx: number, gain: number) =>
  invokeCommand("set_channel_gain", { channelIdx: channel_idx, gain });

export const setChannelMute = (channel_idx: number, is_muted: boolean) =>
  invokeCommand("set_channel_mute", { channelIdx: channel_idx, isMuted: is_muted });

export const setSendGain = (channel_idx: number, mix_idx: number, gain: number) =>
  invokeCommand("set_send_gain", { channelIdx: channel_idx, mixIdx: mix_idx, gain });

export const setSendMute = (channel_idx: number, mix_idx: number, is_muted: boolean) =>
  invokeCommand("set_send_mute", { channelIdx: channel_idx, mixIdx: mix_idx, isMuted: is_muted });

export const setMixGain = (mix_idx: number, gain: number) =>
  invokeCommand("set_mix_gain", { mixIdx: mix_idx, gain });

export const setMixMute = (mix_idx: number, is_muted: boolean) =>
  invokeCommand("set_mix_mute", { mixIdx: mix_idx, isMuted: is_muted });

export const assignProcess = (channel_idx: number, pid: number, name: string) =>
  invokeCommand("assign_process", { channelIdx: channel_idx, pid, name });

// ---------------------------------------------------------------------------
// Dynamic channel lifecycle (frozen contract §5).
// `channel_id` is the authoritative numeric id assigned by the daemon and is
// what every subsequent setChannelGain/setSendGain call must send back.
// ---------------------------------------------------------------------------

export interface EngineSendDto {
  mix_id: number;
  name: string;
  gain: number;
  is_muted: boolean;
}

export interface EngineChannelDto {
  id: number;
  name: string;
  channel_type: string;
  pid: number;
  gain: number;
  is_muted: boolean;
  /** Per-bus send state, in `mix_id` order. This is the authoritative routing. */
  sends?: EngineSendDto[];
}

export interface EngineMixDto {
  id: number;
  name: string;
  gain: number;
  is_muted: boolean;
}

export interface EngineStateDto {
  channels: EngineChannelDto[];
  mixes: EngineMixDto[];
  output_device_id: string;
  output_device_name: string;
}

/**
 * Create a real input channel in the audio engine.
 * For `SoftwareProcess` this actually starts a Process-Loopback capture for `pid`.
 * Returns the numeric channel id to use for all later control calls.
 */
export const addChannel = (name: string, channel_type: string, pid: number): Promise<number> =>
  invokeCommand<number>("add_channel", { name, channelType: channel_type, pid });

/** Remove a channel and stop its capture. Returns true when it was removed. */
export const removeChannel = (channel_id: number): Promise<boolean> =>
  invokeCommand<boolean>("remove_channel", { channelId: channel_id });

/**
 * One virtual cable as reported by the daemon.
 *
 * `render` is the playback endpoint: write audio here to feed the cable.
 * `capture` is the recording endpoint: other applications (Discord, OBS) read
 * the cable from here. Either is `null` when Windows has not enumerated that
 * direction yet.
 */
export interface CableEndpointsDto {
  name: string;
  render: string | null;
  capture: string | null;
}

/**
 * Create (or rename) virtual audio cables so they appear in Windows Sound.
 *
 * `names` is the COMPLETE desired cable set: the daemon restarts its USB/IP
 * server with exactly these cables. Each name becomes a `Speakers (<name>)`
 * playback endpoint and a `Microphone (<name>)` recording endpoint.
 */
export const createCables = (names: string[]): Promise<CableEndpointsDto[]> =>
  invokeCommand<CableEndpointsDto[]>("create_cables", { names });

/** Names of the virtual cables the daemon currently exports. */
export const listCables = (): Promise<string[]> =>
  invokeCommand<string[]>("list_cables");

/**
 * Switch the physical playback endpoint (the `on <device>` selector).
 * `endpoint_id` must be a real IMMDeviceEnumerator id from
 * `getSystemAudioDevices().outputs[].id`. Returns the resolved device name.
 */
export const setOutputDevice = (endpoint_id: string): Promise<string> =>
  invokeCommand<string>("set_output_device", { endpointId: endpoint_id });

/** Authoritative engine state (channels, mixes, current output device). */
export const getEngineState = (): Promise<EngineStateDto> =>
  invokeCommand<EngineStateDto>("get_engine_state");

export const getTelemetrySnapshot = () =>
  invokeCommand<TelemetrySnapshotDto>("get_telemetry_snapshot");

export interface Vst3PluginDto {
  id: string;
  name: string;
  vendor: string;
  category: string;
  path: string;
  is_bundle: boolean;
}

export interface Vst3SlotDto {
  slot_id: number;
  plugin_id?: string;
  plugin_name?: string;
  category?: string;
  is_enabled: boolean;
  is_bypassed: boolean;
  dry_wet: number;
}

export const getVst3Plugins = async (): Promise<Vst3PluginDto[]> => {
  if (isTauri()) {
    return invokeCommand<Vst3PluginDto[]>("get_vst3_plugins");
  }
  return [];
};

export interface AudioProcessDto {
  pid: number;
  name: string;
  process_name: string;
  is_active: boolean;
  icon?: string;
}

export const bindAppAudio = (pid: number, endpoint_id: string): Promise<boolean> =>
  invokeCommand<boolean>("bind_app_audio", { pid, endpointId: endpoint_id });

export const getAppIcon = (pid?: number, process_name?: string): Promise<string | null> =>
  invokeCommand<string | null>("get_app_icon", { pid, processName: process_name });

export const setDefaultPlaybackDevice = (endpoint_id: string): Promise<boolean> =>
  invokeCommand<boolean>("set_default_playback_device", { endpointId: endpoint_id });

export const saveAppConfigFile = (config_json: string): Promise<boolean> =>
  invokeCommand<boolean>("save_app_config", { configJson: config_json });

export const loadAppConfigFile = (): Promise<string | null> =>
  invokeCommand<string | null>("load_app_config");

export interface AudioDeviceDto {
  id: string;
  name: string;
  is_default: boolean;
  flow: "input" | "output";
}

export interface SystemAudioDevicesDto {
  inputs: AudioDeviceDto[];
  outputs: AudioDeviceDto[];
}

export interface InstalledAppDto {
  name: string;
  exe_name: string;
  path: string;
  icon_path?: string;
}

export const getActiveAudioSessions = (): Promise<AudioProcessDto[]> =>
  invokeCommand<AudioProcessDto[]>("get_active_audio_sessions");

export const getSystemAudioDevices = (): Promise<SystemAudioDevicesDto> =>
  invokeCommand<SystemAudioDevicesDto>("get_system_audio_devices");

export const getInstalledApplications = (): Promise<InstalledAppDto[]> =>
  invokeCommand<InstalledAppDto[]>("get_installed_applications");

export const setVst3Slot = (
  channel_idx: number,
  slot_idx: number,
  params: { plugin_id?: string; is_bypassed?: boolean; dry_wet?: number; clear?: boolean }
) =>
  invokeCommand("set_vst3_slot", {
    channelIdx: channel_idx,
    slotIdx: slot_idx,
    pluginId: params.plugin_id,
    isBypassed: params.is_bypassed,
    dryWet: params.dry_wet,
    clear: params.clear,
  });

export const openVst3Editor = (channel_idx: number, slot_idx: number) =>
  invokeCommand("open_vst3_editor", { channelIdx: channel_idx, slotIdx: slot_idx });

