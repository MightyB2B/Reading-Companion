import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { initTheme } from "./lib/theme";
import { initPrefs } from "./lib/reading";

// Before the first paint, so the window never flashes the wrong palette.
initTheme();
// Both before React mounts, so the first frame is already set the way the
// reader left it rather than snapping into place afterwards.
initPrefs();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
