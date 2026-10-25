# Brand files

The app icon and brand mark, from the design (#47, "Phase 5 Icon and Brand Mark"). The icons the app uses are in `src-tauri/icons/`; this folder keeps the sources and the files not wired in yet.

- `../icon.svg`: the master app icon (white mark on the #0a5cb8 tile). 48 px and up are this drawing.
- `mark.svg`: the brand mark for light backgrounds, the same as the mark in the app's sidebar.
- `tray-symbolic.svg`: the flat tray glyph for GNOME and KDE, which recolour it to match the panel. Not used yet: Tauri draws the tray from a PNG.
- `installer-sidebar.png` (164x314) and `installer-header.png` (150x57): the Windows installer art. Tauri's NSIS options (`sidebarImage`, `headerImage`) take bitmaps (`.bmp`), not PNGs, so they are not wired into `tauri.windows.conf.json` yet. Convert them to 24-bit BMP, flattened onto #0a5cb8, and set both options.

## What is in `src-tauri/icons/`

| File | From |
|---|---|
| `16x16.png`, `24x24.png`, `32x32.png` | The design's pixel-drawn frames. Never scale these from the SVG. |
| `48x48.png`, `64x64.png`, `256x256.png`, `icon.png` (512) | The design's exports. |
| `icon.ico` | The design's `app.ico` (16, 24, 32, 48, 64, 256). |
| `128x128.png` | Rendered from `icon.svg` at 128 px. The design has none. |
| `icon.icns` | Made by `pnpm tauri icon` (macOS is not built; the config lists it). |
| `tray/tray-{16,20,24,32}-{dark,light}.png` | The design's tray glyphs. `dark` is the white glyph, for dark panels; `light` is the near-black one. |

The window icon is the first PNG in `bundle.icon` on Linux (`128x128.png`), and the `.ico` resource on Windows. The tray uses `tray/` (see `background.rs`).

## Regenerating

Render `128x128.png` with a browser (a transparent background, exactly 128x128) from `../icon.svg`. For `icon.icns`, render the SVG at 1024 px and run `pnpm tauri icon <that.png> -o <a new empty folder>`, then take only `icon.icns` from its output: the rest of Tauri's output is not used, and the design's own files replace its PNGs and `.ico`. Do not run `tauri icon` over `src-tauri/icons/`. The release-config test (`the_icon_set_is_complete_for_both_platforms` in `src-tauri/src/updates.rs`) checks the set is complete.
