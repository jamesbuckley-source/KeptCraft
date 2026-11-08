# AGENTS.md: guide for AI agents and contributors

KeptCraft is a cross-platform screenshot tool written in Rust only.
The UI is native egui/eframe (glow renderer).
Never add Tauri, Electron or any webview or JavaScript UI layer.

It is a fork of [ShotCraft](https://github.com/mmartinez/shotcraft) by Miguel Martinez, and it was
called ShotCraft4Kept before the rename: keep ShotCraft's copyright and license notices, credit the
original project and author wherever the app names itself (the About page, the README and `NOTICE`),
and keep the code close enough to merge from upstream.

## Fork: defaults shaped for Kept

This fork is Kept's screenshot editor: captures are copied to the clipboard so Kept's history picks
them up, they save into `Pictures/KeptCraft`, the Quick Access card closes itself, and every default
shortcut avoids Kept's `Alt+V`/`Alt+Shift+V` and the `Alt+Shift` layout switch.
They live in `Settings::default()` and `config::tests::defaults_suit_a_clip_manager`; keep them when
merging upstream.

## Workspace map

```text
crates/
  doc        L0 document model, tiny-skia rendering, pixel effects, scrolling-capture stitching (pure data and pixels)
  engine     L1 screen capture (xcap), settings and state, history, OCR (ocrs), clipboard, save, upload, GitHub updates, launching modes, agent operations (ops)
  ui-egui    L2 every window: capture overlay, editor, Quick Access, pin, scrolling capture, history, settings, toasts
apps/
  keptcraft  L3 the binary: clap CLI (one subcommand per mode), the tray daemon and the MCP server (`keptcraft mcp`)
```

A crate depends only on lower layers.
Nothing below `ui-egui` may use egui, eframe, winit or rfd; `doc` uses `emath` and `ecolor` for geometry and colour.
File dialogs live in `ui-egui/src/dialogs.rs`.

Each mode runs in its own process (`keptcraft <command>`): the daemon and the windows launch modes with `engine::share::spawn`, so a crash in one capture never takes down the tray.

## Golden rules

### Never crash

A bad file, a corrupt settings file, a missing monitor or a full disk must produce an error the user can act on, never a panic.

- Non-test code never calls `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!` or `unimplemented!`.
  Return the crate's error type and propagate with `?`, or use `ok_or`, `let .. else` and `if let`.
  The one exception is a provably infallible literal, marked `#[allow(clippy::expect_used)]` with a comment saying why it cannot fail.
- Every crate root carries `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`; `clippy.toml` allows them in tests only.
- Use `get()` for indices that come from input, files, settings or arithmetic on them.
- Lock with `lock().unwrap_or_else(PoisonError::into_inner)` and treat thread joins as `Result`s.
- Pixel buffers sized by input go through `render::pixmap`, which refuses sizes over its cap with `doc::Error::TooLarge`; tiny-skia aborts the process on a failed allocation, so never call `Pixmap::new` directly.
- `apps/keptcraft/src/main.rs` catches an escaped panic around `dispatch` and reports it; keep `panic = "unwind"`.

### No `unsafe`

The workspace sets `unsafe_code = "forbid"`.
Platform interop that winit and egui do not cover goes in `ui-egui/src/platform.rs` and uses safe bindings (objc2 on macOS); capture-level queries the engine needs below the UI (the cursor position) stay in `engine/src/capture.rs`.

### Errors

`doc` and `engine` return their own `thiserror` types (`keptcraft_doc::Error`, `keptcraft_engine::Error`) so callers and future automation get structured errors.
`ui-egui` and the app use `anyhow` for window and flow glue.

### Clean-room

Never download, decompile or copy another product's binaries, code or assets.
Bundled third-party assets must be permissively licensed, keep their license file next to them and get a row in `ATTRIBUTION.md` in the same change.
Files the app downloads at runtime instead of bundling (the OCR models, CC BY-SA 4.0) are not redistributed, so the permissive rule does not apply; they still get a row in `ATTRIBUTION.md` naming their license.

### Licenses and icons

`THIRD-PARTY-LICENSES.md` is generated on Linux; never edit it by hand.
After changing dependencies, regenerate it on Linux with `cargo about generate --workspace --locked about.hbs -o THIRD-PARTY-LICENSES.md` (cargo-about 0.9.2).
A Windows run writes a different list, because cargo-about's `ring` workaround picks up different files there; if you cannot run Linux, push the change and take the `THIRD-PARTY-LICENSES` artifact the failing CI job uploads.
CI fails when the file is stale or a crate's license is not in `about.toml`'s `accepted` list.
The check compares the file's non-blank lines with their endings stripped and sorted in byte order, because cargo-about orders each crate's "used by" list at random and copies some license texts with their own CRLF endings.
Add a license to `accepted` only if it is permissive or, like MPL-2.0, file-level copyleft that applies to unmodified crates.

The app icon's master is `assets/app-icon/keptcraft.svg`; after editing it, run `packaging/icons.sh` (needs resvg) and commit the PNGs it writes.
The app id `io.github.jamesbuckley-source.keptcraft` (`theme::APP_ID`) names the window class, the desktop entry and the icons; change them together.

### Shared rendering

The editor preview and every export go through `doc::render`, so what the user sees is what gets saved.
Add drawing behaviour there, not in egui painting code.

### Files another app handed over

`open-annotate --save-to` and `--overwrite` are a contract with a caller (Kept, a script): Save writes
that exact path, prints it on stdout, and the write goes through a temporary file and a rename, so a
watcher never reads a partial image. Keep all three when touching the editor's save path.

### Updates

`engine::update` is the only network call the app makes on its own, and the installer it starts
outlives the app (`ui-egui/src/update_ui.rs` runs the check, the download and the installer off the
frame thread; the windows only draw its stage).

- Asset names are the contract: `packaging/` writes `keptcraft-<version>-<os>-<arch>.<ext>`, and
  `update::asset_fits` picks this system's file from that name. Change both together.
- A stable build is only offered stable releases; a pre-release build is offered pre-releases too,
  so a beta channel stays a beta channel.
- A failed check is a message, never a panic and never a blocking frame: the updater reads
  `Settings::update_auto` and the day-old timestamp in `State`, and the answer is cached there.
- The check must never send anything about the machine, and the download must never be run from the
  network: it is written to the data folder first, then handed to the system's installer.

## Checks

Run these before every commit; CI runs clippy and the tests on Linux, Windows and macOS, and the format check on Linux.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI also type-checks the workspace (`cargo check`) with the `rust-version` in `Cargo.toml` (the egui crates set that floor); raise it in the same change as code that needs a newer compiler.

Linux build packages are listed in the README's Building section; CI installs the same list.
For platform-specific code, also check the other targets, for example with `cargo-zigbuild clippy --target aarch64-apple-darwin` (ring's C code needs a cross C toolchain, which zig provides; `cargo zigbuild` only builds).

## Verifying UI changes

Run the real binary and look at the result.
On Linux a headless X server works: start `Xvfb :77 -screen 0 1920x1080x24` and a window manager, set `DISPLAY`, point `KEPTCRAFT_HOME` at a scratch directory, launch subcommands, drive them with XTest, and screenshot the root window.
`KEPTCRAFT_HOME` replaces the settings and data folders on every OS, so tests and scratch runs never touch a real user's captures.
The names it had before the rename (`SHOTCRAFT4KEPT_HOME`, `SHOTCRAFT_HOME`) still work, because Kept's handoff passes them.
Give egui at least 50 ms between synthetic input events or it misses keys.

## Agents and the command registry

`keptcraft mcp` (`apps/keptcraft/src/mcp.rs`) serves AI agents over the Model Context Protocol (rmcp, stdio).
Each tool is a thin wrapper over one function in `engine::ops`: a typed request in (serde and schemars, so the request type is the tool's JSON Schema), a typed result out.
Agent operations never toast, open windows or change the clipboard unless that is the operation's whole point.
History ids from an agent arrive as `history::HistoryId`, which is checked when it is decoded.
Tool names and parameter names are public API: hosts store them in configs and per-tool approvals, and `apps/keptcraft/tests/mcp.rs` fails when one changes.
In `keptcraft mcp`, stdout carries only MCP messages; log to stderr.

The planned next step is a command registry in `engine` (stable id, label, shortcut, JSON params), dispatched by id from the UI, the CLI, a JSON control channel and the MCP server; the `ops` request types become its JSON params unchanged.
New user-visible behaviour should be shaped so it can become such a command.
