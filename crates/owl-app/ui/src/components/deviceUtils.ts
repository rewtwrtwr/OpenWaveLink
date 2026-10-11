import { AudioDeviceDto } from "../ipc";

/**
 * Honest virtual-endpoint detection (contract §7).
 *
 * On this machine `GetDefaultAudioEndpoint(eRender, eConsole)` returns
 * "Spotify (Elgato Virtual Audio)" — a virtual cable, not the user's
 * headphones — so blindly trusting `is_default` renders the monitor mix into a
 * cable the user cannot hear.
 *
 * We classify purely on the DEVICE NAME. No endpoint GUID is ever hardcoded.
 */
const VIRTUAL_NAME_MARKERS = [
  "elgato virtual audio",
  "vb-audio",
  "cable",
  "virtual",
  "fifine virtual",
];

/** True when the endpoint looks like a virtual audio cable rather than real hardware. */
export const isVirtualDevice = (name: string): boolean => {
  const lower = (name || "").toLowerCase();
  return VIRTUAL_NAME_MARKERS.some((marker) => lower.includes(marker));
};

/**
 * Choose the endpoint the monitor output should bind to on first run.
 *
 * Order of preference:
 *   1. a non-virtual device flagged as the system default
 *   2. any non-virtual device (real hardware beats a cable)
 *   3. the system default, even if virtual
 *   4. the first enumerated endpoint
 */
export const pickPreferredOutput = (
  outputs: AudioDeviceDto[]
): AudioDeviceDto | undefined => {
  if (!outputs || outputs.length === 0) return undefined;

  const physical = outputs.filter((d) => !isVirtualDevice(d.name));
  if (physical.length > 0) {
    return physical.find((d) => d.is_default) || physical[0];
  }

  return outputs.find((d) => d.is_default) || outputs[0];
};
