import React, { useEffect, useRef, useState } from "react";

interface StereoVuMeterProps {
  peakL: number; // 0.0 - 1.0
  peakR: number; // 0.0 - 1.0
  rmsL?: number;
  rmsR?: number;
  clipping?: boolean;
  orientation?: "vertical" | "horizontal";
  height?: number | string;
  width?: number | string;
  showLabels?: boolean;
}

/**
 * Continuous smooth VU meter bar in Elgato Wave Link green style.
 * Normal level: #22c55e (green)
 * Near peak:   #eab308 (yellow)
 * Clipping:    #ef4444 (red)
 */
export const StereoVuMeter: React.FC<StereoVuMeterProps> = ({
  peakL,
  peakR,
  clipping = false,
  orientation = "vertical",
  height = 140,
  width = 16,
  showLabels = false,
}) => {
  // Smooth decayed peak
  const [smoothL, setSmoothL] = useState(0);
  const [smoothR, setSmoothR] = useState(0);
  const lastTimeRef = useRef(performance.now());

  useEffect(() => {
    const now = performance.now();
    const dt = Math.min((now - lastTimeRef.current) / 1000, 0.1);
    lastTimeRef.current = now;

    // Fast attack (instant jump up), gentle decay (~30dB/sec)
    setSmoothL((prev) => (peakL >= prev ? peakL : Math.max(0, prev - dt * 2.5)));
    setSmoothR((prev) => (peakR >= prev ? peakR : Math.max(0, prev - dt * 2.5)));
  }, [peakL, peakR]);

  const pctL = Math.min(100, Math.max(0, smoothL * 100));
  const pctR = Math.min(100, Math.max(0, smoothR * 100));

  const getBarColor = (pct: number) => {
    if (clipping || pct >= 98) return "#ef4444";
    if (pct >= 85) return "#eab308";
    return "#22c55e";
  };

  if (orientation === "horizontal") {
    return (
      <div className="flex flex-col gap-1 w-full" style={{ width }}>
        <div className="h-1.5 w-full bg-[#181520] rounded-full overflow-hidden border border-white/[0.04]">
          <div
            className="h-full rounded-full transition-[width] duration-75 ease-out"
            style={{
              width: `${pctL}%`,
              backgroundColor: getBarColor(pctL),
            }}
          />
        </div>
        <div className="h-1.5 w-full bg-[#181520] rounded-full overflow-hidden border border-white/[0.04]">
          <div
            className="h-full rounded-full transition-[width] duration-75 ease-out"
            style={{
              width: `${pctR}%`,
              backgroundColor: getBarColor(pctR),
            }}
          />
        </div>
      </div>
    );
  }

  return (
    <div className="flex items-center gap-1 select-none" style={{ height }}>
      {showLabels && (
        <div className="flex flex-col justify-between text-[9px] text-[#71717a] font-mono h-full pr-0.5 leading-none">
          <span>0</span>
          <span>-6</span>
          <span>-18</span>
          <span>-36</span>
          <span>-∞</span>
        </div>
      )}

      <div
        className="flex gap-1 h-full p-1 bg-[#14121a] rounded-md border border-white/[0.06]"
        style={{ width }}
      >
        {/* Left channel */}
        <div className="flex-1 h-full bg-[#1f1b29] rounded-sm overflow-hidden flex flex-col justify-end">
          <div
            className="w-full rounded-sm transition-[height] duration-75 ease-out"
            style={{
              height: `${pctL}%`,
              backgroundColor: getBarColor(pctL),
            }}
          />
        </div>

        {/* Right channel */}
        <div className="flex-1 h-full bg-[#1f1b29] rounded-sm overflow-hidden flex flex-col justify-end">
          <div
            className="w-full rounded-sm transition-[height] duration-75 ease-out"
            style={{
              height: `${pctR}%`,
              backgroundColor: getBarColor(pctR),
            }}
          />
        </div>
      </div>
    </div>
  );
};
