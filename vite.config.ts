import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri` and `crates`, plus the
      //    large build artefacts: watching the 23MB dictionary source and the
      //    40MB generated database makes the dev server fall over with EBUSY
      //    while the build script is writing them.
      //
      //    `target/` matters since the cargo workspace split moved it to the
      //    repository root. It used to live under src-tauri/ and be covered by
      //    the first pattern; uncovered, Vite tries to watch the executable
      //    cargo is in the middle of linking and dies with EBUSY.
      ignored: [
        "**/src-tauri/**",
        "**/crates/**",
        "**/target/**",
        "**/.cache/**",
        "**/dist/**",
      ],
    },
  },
}));
