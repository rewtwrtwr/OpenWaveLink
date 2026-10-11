import React, { useState, useEffect } from "react";
import { Mic, MicOff, Headphones, Gamepad2, VolumeX } from "lucide-react";
import { HudNotificationDto, listenEvent } from "../ipc";

/**
 * Taskbar HUD notification — matte graphite, gradient-free.
 * The volume bar is a flat single-colour fill; red is reserved for mute.
 */
export const HudOverlayView: React.FC = () => {
  const [notif, setNotif] = useState<HudNotificationDto>({
    title: "Microphone",
    detail: "Hardware mute toggled",
    volume: 0.0,
    is_muted: true,
    icon: "mic",
  });
  const [visible, setVisible] = useState<boolean>(true);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let fadeTimer: ReturnType<typeof setTimeout> | undefined;

    listenEvent<HudNotificationDto>("hud-notification", (data) => {
      setNotif(data);
      setVisible(true);

      clearTimeout(fadeTimer);
      fadeTimer = setTimeout(() => {
        setVisible(false);
      }, 1900);
    }).then((un) => {
      unlisten = un;
    });

    return () => {
      unlisten?.();
      clearTimeout(fadeTimer);
    };
  }, []);

  const getIcon = () => {
    if (notif.icon === "mic") {
      return notif.is_muted ? (
        <MicOff className="w-5 h-5 text-[#ef4444]" />
      ) : (
        <Mic className="w-5 h-5 text-[#9ca3af]" />
      );
    }
    if (notif.icon === "headphones") {
      return notif.is_muted ? (
        <VolumeX className="w-5 h-5 text-[#ef4444]" />
      ) : (
        <Headphones className="w-5 h-5 text-[#9ca3af]" />
      );
    }
    return <Gamepad2 className="w-5 h-5 text-[#9ca3af]" />;
  };

  const volumePct = Math.round(Math.max(0, Math.min(1, notif.volume)) * 100);

  return (
    <div className="w-[380px] h-[86px] flex items-center justify-center p-2 select-none pointer-events-none">
      <div
        className={`w-full h-full bg-[#16141a] border border-white/[0.08] rounded-xl p-3 flex items-center gap-3 shadow-xl shadow-black/60 transition-all duration-300 transform ${
          visible ? "opacity-100 scale-100 translate-y-0" : "opacity-0 scale-95 -translate-y-2"
        }`}
      >
        <div
          className={`p-2.5 rounded-lg border flex items-center justify-center shrink-0 ${
            notif.is_muted
              ? "bg-[#3a1c24] border-[#ef4444]"
              : "bg-[#211d27] border-white/[0.06]"
          }`}
        >
          {getIcon()}
        </div>

        <div className="flex-1 min-w-0 flex flex-col justify-center">
          <div className="flex items-center justify-between mb-1">
            <span className="text-xs font-semibold text-[#e4e4e7] truncate">{notif.title}</span>
            <span
              className={`text-[11px] font-mono ${
                notif.is_muted ? "text-[#ef4444]" : "text-[#9ca3af]"
              }`}
            >
              {notif.is_muted ? "MUTED" : `${volumePct}%`}
            </span>
          </div>

          <div className="w-full h-2 bg-[#191621] rounded-full overflow-hidden border border-white/[0.06] relative">
            <div
              className={`h-full rounded-full transition-all duration-100 ${
                notif.is_muted ? "bg-[#ef4444]" : "bg-[#71717a]"
              }`}
              style={{ width: notif.is_muted ? "100%" : `${volumePct}%` }}
            />
          </div>

          <span className="text-[10px] text-[#71717a] mt-0.5 truncate">{notif.detail}</span>
        </div>
      </div>
    </div>
  );
};
