/** @type {import('tailwindcss').Config} */
// Design tokens are frozen in docs/INTERNAL_ENGINE_CONTRACT.md §6 and
// docs/03_ui_and_control/gui_specification/WAVE_LINK_3_UI_SPEC.md.
// Strict matte grey/graphite minimalism: NO neon, NO glows, NO gradients.
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        owl: {
          bg: "#16141a",           // app canvas
          inputRow: "#211d27",     // left channel row card
          card: "#272230",         // active cross-point card
          empty: "#191621",        // inactive matrix cell
          mixHeader: "#262130",    // mix column header card
          dropdown: "#201c2b",     // + Create channel panel
          search: "#16131f",       // dropdown search field bg
          searchBorder: "#3d374a", // dropdown search field border
          monitor: "#c084fc",      // 3px listening strip (top edge only)
          thumb: "#d88be2",        // slider thumb, NO glow
          track: "#332d3d",        // inactive slider track
          progress: "#a855f7",     // active slider track, NO glow
          muteBg: "#3a1c24",       // matte dark maroon
          muteBorder: "#ef4444",   // thin red
          category: "#9ca3af",     // 11px uppercase section headings
          primary: "#e4e4e7",      // primary text
          secondary: "#71717a",    // secondary text / captions
          meter: "#8b8b95",        // matte neutral VU fill
          border: "rgba(255, 255, 255, 0.06)",
        },
      },
      fontFamily: {
        sans: ['"Segoe UI Variable Text"', '"Segoe UI Variable"', '"Segoe UI"', 'system-ui', 'sans-serif'],
      },
    },
  },
  plugins: [],
}
