import React, { useState, useEffect } from "react";
import { MatrixView } from "./components/MatrixView";
import { FlyoutView } from "./components/FlyoutView";
import { HudOverlayView } from "./components/HudOverlayView";

export const App: React.FC = () => {
  const [route, setRoute] = useState<string>(() => window.location.hash);

  useEffect(() => {
    const handleHashChange = () => {
      setRoute(window.location.hash);
    };

    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  if (route.includes("/flyout")) {
    return <FlyoutView />;
  }

  if (route.includes("/overlay")) {
    return <HudOverlayView />;
  }

  return <MatrixView />;
};
