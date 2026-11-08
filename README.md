# ShotCraft4Kept

A screenshot and annotation tool for the [Kept](https://github.com/jamesbuckley-source/kept) clipboard
manager, built on [ShotCraft](https://github.com/mmartinez/shotcraft) by Miguel Martinez. Pure Rust,
with an egui/eframe UI and no webview.

This is a **custom fork**, not upstream ShotCraft: it ships Kept-shaped defaults, saves where Kept
looks for edits, and adds a File menu, a Kept integration setting and an "Open from Kept DB" list.
Upstream's documentation below still applies where this fork has not changed something, and the code
stays close enough to merge from upstream.

![The ShotCraft4Kept editor with numbered steps, a callout arrow, a highlighted line and a pixelated secret key on a gradient background](assets/screenshots/editor.png)

## Features

| Feature | ShotCraft4Kept |
|---|---|
| One shortcut, every capture mode (All-In-One) | `all-in-one`: area, fullscreen, window, scrolling, timer and OCR from one bar, with resize handles, typed size and locked aspect ratio |
| Area, previous area, fullscreen, window capture | `capture-area`, `capture-previous-area`, `capture-fullscreen`, `capture-window` over a frozen screen, with crosshair and magnifier; windows can get a shadow or a background |
| Self-timer | `self-timer`: select, then a countdown beside the region |
| Annotate, redact and polish | Editor with arrow (4 styles, curved), rectangle, filled rectangle, ellipse, line, pen with smoothing, highlighter, text (7 styles, single line or a wrapping paragraph you resize by its corners), counter (ABC or 123, size, ring, drop shadow with strength, blur, offset and direction), spotlight, blur, pixelate and solid redaction, crop with aspect ratios, image drop-in, colour palette and picker |
| Zoom and move the canvas | Wheel zooms smoothly around the pointer, the toolbar slider and ± buttons zoom, a reset returns to the fit; hold Space (or pick the hand, or drag with the middle button) to move around a zoomed image |
| Numbered steps with commentary | A resizable panel on the right (250 px wide by default, its width and fold remembered) with a box per step: select text in a comment to make it bold or italic from the B/I buttons, bullet the lines you select, rename a step's badge by double-clicking it, an optional "Comments" header you can rename, band and text colours, text size, space between items and padding, on any edge — right for a tall image, bottom for a wide one, with round badges in each counter's colour |
| Background tool | Gradients (built-in and your own — double-click one to edit it), solid colours you can add to and remove from your palette, image and blurred fills, padding, corner radius, shadow, aspect ratio, alignment, auto-balance, a draggable image watermark, saved presets that carry the background, the counter look and every comment band setting, shared as a single self-contained .shotpreset file (File ▸ Export Preset… / Import Preset…) Every colour picker has an eyedropper for colours in the image |
| Start screen | Launching the editor with no file shows the last six files you edited and Kept's nine newest captures as thumbnails (three to a row, tall shots cropped to fill the tile), with an Open a file… card when there is no history yet |
| Editable project files | `.shotcraft` (JSON with the base image and every annotation) |
| Scrolling capture | Stitches frames while you or auto-scroll move the content |
| Quick Access Overlay | Floating card after each capture: copy, save, annotate, pin, upload, OCR, or drag the file into another app |
| Text recognition | On-device OCR (ocrs); QR codes are read first |
| Pin screenshots | Always-on-top image with zoom, opacity (1 to 0 keys) and arrow-key nudging |
| Capture history | One month of captures with filters, restore, copy, save, annotate and pin |
| Upload and share a link | Multipart POST to an endpoint you configure; the link from the response is copied |

Not covered: hosted cloud sharing with comment threads, screen recording, and hiding desktop icons.

## Commands

Every mode is a subcommand of one binary (`shotcraft4kept capture-area`), so any of them can be bound to an OS shortcut.
Running `shotcraft4kept` with no subcommand starts the tray daemon with global shortcuts.

| Command | Options |
|---|---|
| `all-in-one` | `--x --y --width --height --display` |
| `capture-area` | `--x --y --width --height --display --action copy\|save\|annotate\|upload` |
| `capture-previous-area` | `--action` |
| `capture-fullscreen` | `--display --action` |
| `capture-window`, `self-timer` | `--action` |
| `scrolling-capture` | `--x --y --width --height --display --start --autoscroll` |
| `capture-text` | `--filepath --x --y --width --height --display --linebreaks true\|false` |
| `pin`, `add-quick-access-overlay` | `--filepath` |
| `open-annotate` | `--filepath --save-to PATH --overwrite` |
| `open-from-clipboard`, `open-history`, `restore-recently-closed` | |
| `open-settings` | `--tab general\|shortcuts\|quickaccess\|screenshots\|cloud\|advanced\|about` |
| `mcp` | Serves AI agents over stdin and stdout, see below |

Coordinates are pixels of the chosen display with the origin at its top-left corner.
`--display 1` is the main display.

## Use with Kept (or another clip manager)

Kept, and any app that stores images, can hand a file to `open-annotate` and get the edit back at a
path it already knows, instead of watching the save folder for a new file:

```sh
shotcraft4kept open-annotate --filepath "C:\clips\shot.png" --save-to "C:\clips\shot-edited.png"
shotcraft4kept open-annotate --filepath "C:\clips\shot.png" --overwrite
```

- `--save-to PATH` makes Save write that file, creating its folder when missing. A name without an
  extension gets the configured format's.
- `--overwrite` saves back over the file being annotated.
- Both print the saved path on stdout, one line per save, so the caller can pick up the result
  without guessing.
- The file is written beside the target and renamed over it, so a watcher never opens a half-written
  image. Saving twice replaces the same file.
- Save As and the other editor actions keep working; they do not move the destination.

Kept's image editor setting takes the whole command line, so point it at:

```text
"C:\Program Files\ShotCraft4Kept\shotcraft4kept.exe" open-annotate --filepath {file} --overwrite
```

### Defaults in this fork

A fresh install is already set up for Kept; Settings changes any of it:

| Setting | Default | Why |
|---|---|---|
| Copy after capture | on | Every capture lands in Kept's history without a second keystroke |
| Save folder | `Pictures\ShotCraft4Kept` | Captures stay off the Desktop, and Kept watches this folder for edits |
| Quick Access | closes after 10 s | The clip is already in Kept; the card is there to annotate |
| Shortcuts | `Ctrl+Shift+2`…`Ctrl+Shift+8`, `Ctrl+Shift+H` | Kept owns `Alt+V` and `Alt+Shift+V`, and Windows switches keyboard layouts on `Alt+Shift` |

## Kept integration

Settings → Kept finds Kept through its own configuration (`%APPDATA%\ikept.it\storage.json` names the
data folder, and `kept.exe` sits in `%LOCALAPPDATA%\Kept`). Turning **Integrate with Kept** on gives
two things:

- **File → Open from Kept DB** in the editor lists the 20 newest image clips straight out of Kept's
  `copypaste.db`, and opening one loads it for annotation.
- Kept's **image editor** setting is pointed at this binary, so Kept's edit button opens
  ShotCraft4Kept's editor with `--overwrite`, and the result lands back in the clip's own file.

The image editor setting lives inside Kept's database, and Kept keeps its settings in memory while it
runs, so ShotCraft4Kept only writes it when Kept is closed. With Kept running, the page shows the exact
command to paste into Kept's Settings, Image editor, with a Copy button. Reading the clip list works
in either case, though a Kept data folder on a synced drive (OneDrive, Google Drive) can be locked for
a moment while Kept writes; the list says so rather than showing nothing.

## Use with AI agents

`shotcraft4kept mcp` speaks the [Model Context Protocol](https://modelcontextprotocol.io), so an agent such as Claude Code, Codex or Cursor can capture the screen, read text and hand captures back to you.
Register it with your agent:

```sh
claude mcp add shotcraft4kept -- shotcraft4kept mcp
```

```toml
# ~/.codex/config.toml
[mcp_servers.shotcraft]
command = "shotcraft4kept"
args = ["mcp"]
```

```json
{ "mcpServers": { "shotcraft4kept": { "command": "shotcraft4kept", "args": ["mcp"] } } }
```

The JSON form goes in `.mcp.json` (Claude Code), `.cursor/mcp.json` (Cursor) or your agent's equivalent.

| Tool | What it does |
|---|---|
| `capture_screen` | Captures a display (`display` 1 is the main one, the default) or an `area` of it |
| `capture_window` | Captures one window by the id `list_windows` returns |
| `list_windows` | Lists the visible windows of other apps, topmost first |
| `list_captures`, `get_capture` | Browse the capture history and fetch an entry again, optionally cropped to an `area` |
| `read_text` | Returns the text or QR code contents of a capture |
| `copy_capture`, `save_capture`, `upload_capture` | Copy a capture to the clipboard, save it into your screenshots folder, or upload it to your configured endpoint |
| `show_capture` | Opens a capture for you in the editor, as a pin or in Quick Access |

Every capture goes into your capture history, and the other tools refer to it by its history id.
The image an agent sees is scaled down to 1568 pixels on its long edge; `area` coordinates are always full-resolution pixels, and the result's `path` is the full capture.
An agent's capture shows no notification or Quick Access and leaves the clipboard alone unless the agent calls `copy_capture`.
On macOS, grant Screen Recording permission to the app that runs the agent (Terminal, Cursor, Claude), not to ShotCraft4Kept.

## Install

Download the package for your system from the [releases page](https://github.com/jamesbuckley-source/shotcraft4kept/releases); `SHA256SUMS` lists each file's checksum.
The builds are not code-signed yet, so Windows and macOS ask you to confirm the first launch.

| System | Packages | First launch |
|---|---|---|
| Linux x86_64 and aarch64, glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 and later) | `.deb`, `.rpm`, `.AppImage`, `.tar.gz` | The `.deb` and `.rpm` add ShotCraft4Kept to the app menu. Make the AppImage executable (`chmod +x`) and run it. The `.tar.gz` holds `bin/shotcraft4kept` and its `share/` files. |
| Windows 10 and 11, x64 and arm64 | `.msi` installer, `.zip` | When SmartScreen says "Windows protected your PC", choose More info, then Run anyway. The installer adds ShotCraft4Kept to the Start menu. |
| macOS 11 or later, Apple silicon and Intel | `.dmg` | Drag ShotCraft4Kept to Applications and open it. When macOS says it cannot verify the app, open System Settings, Privacy & Security, and choose Open Anyway. |

## Building

Build from source with Rust 1.95 or newer (<https://rustup.rs>).

```sh
cargo build --release
```

The binary is `target/release/shotcraft4kept` (`shotcraft4kept.exe` on Windows).
Each platform also needs a C toolchain and, on Linux, some development packages:

- Linux (Debian and Ubuntu package names): `build-essential pkg-config libclang-dev libxcb1-dev libgbm-dev libegl-dev libpipewire-0.3-dev libspa-0.2-dev`.
- Windows: the MSVC toolchain, with the "Desktop development with C++" workload of the Visual Studio Build Tools (the rustup installer offers it).
- macOS: the Xcode Command Line Tools (`xcode-select --install`).

### Linux desktop integration

`packaging/linux/install.sh` installs the release binary, a desktop entry, the app icons and AppStream metadata into `~/.local` (set `PREFIX` for another location).
The launcher's right-click menu offers the capture modes directly.
`packaging/linux/install.sh --uninstall` removes them again; settings and capture history stay.

## Platform notes

- macOS: grant Screen Recording permission on first capture.
  Auto-scroll in scrolling capture also needs Accessibility permission, because it moves the pointer and scrolls.
  The capture overlay raises its windows above the menu bar and Dock.
- Wayland: ShotCraft4Kept's global shortcuts and window placement need X11.
  On a Wayland session, bind your desktop's shortcuts to the subcommands instead.
  Only X11 (including XWayland, with `WAYLAND_DISPLAY` unset) has been tested.
- Linux drag and drop out of Quick Access speaks XDND; it has been tested with X11 apps.
- Copying on Linux needs a local X display with the X-Resource extension, which Xorg, XWayland and Xvfb provide: ShotCraft4Kept reports a copy only after confirming it owns the clipboard, so it refuses to copy over `ssh -X` or other network displays, where the X server cannot tell its clients apart.
- The OCR models (about 12 MB) download on first use into the folder shown in Settings, Advanced.

## Layout

```text
crates/doc        document model, tiny-skia rendering, pixel effects, scrolling-capture stitching (no UI, no I/O layout)
crates/engine     screen capture, settings and state, history, OCR, clipboard, save, upload, launching modes, agent operations
crates/ui-egui    every window: overlay, editor, Quick Access, pin, scrolling capture, history, settings, toasts
apps/shotcraft4kept    the binary: CLI, tray daemon and MCP server
assets/app-icon   the app icon (SVG master and generated theme PNGs)
packaging         icon generation, the Linux desktop entry and installer, and the release packages for each platform
```

See [`AGENTS.md`](AGENTS.md) for the rules the code follows and [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to contribute.
Report bugs and request features in the [issue tracker](https://github.com/jamesbuckley-source/shotcraft4kept/issues).

## License

MIT OR Apache-2.0, at your option.
See [`NOTICE`](NOTICE) for the bundled third-party material, [`ATTRIBUTION.md`](ATTRIBUTION.md) for the per-file list, and [`THIRD-PARTY-LICENSES.md`](THIRD-PARTY-LICENSES.md) for the licenses of the Rust crates compiled into the app.

Contributions are dual-licensed the same way; see [`CONTRIBUTING.md`](CONTRIBUTING.md#license).
To report a vulnerability, see [`SECURITY.md`](SECURITY.md).


