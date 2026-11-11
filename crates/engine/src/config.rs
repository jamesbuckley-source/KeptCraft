//! Settings, persistent state and on-disk locations.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::doc::model::{Background, OwnedGradient};

const APP_DIR: &str = "keptcraft";
/// Folders this fork used before it was renamed; a fresh install takes its settings from the newest
/// one that exists, so the save folder, palette and Kept editor path survive the renames.
const LEGACY_APP_DIRS: [&str; 2] = ["shotcraft4kept", "shotcraft"];

/// `KEPTCRAFT_HOME` keeps settings and data in that one folder instead of the platform's (tests,
/// sandboxes). The names it had before the rename stay, because existing scripts and Kept's handoff
/// pass them.
fn home_override() -> Option<PathBuf> {
    ["KEPTCRAFT_HOME", "SHOTCRAFT4KEPT_HOME", "SHOTCRAFT_HOME"].iter().find_map(|name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from))
}

pub fn config_dir() -> PathBuf {
    home_override().unwrap_or_else(|| dirs::config_dir().unwrap_or_else(std::env::temp_dir).join(APP_DIR))
}

pub fn data_dir() -> PathBuf {
    home_override().unwrap_or_else(|| dirs::data_dir().unwrap_or_else(std::env::temp_dir).join(APP_DIR))
}

pub fn history_dir() -> PathBuf {
    data_dir().join("history")
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn run_dir() -> PathBuf {
    data_dir().join("run")
}

/// Create the config and data folders readable only by this user: they hold screenshots and upload credentials.
pub fn ensure_private_dirs() -> Result<()> {
    for dir in [config_dir(), data_dir()] {
        make_private(&dir)?;
    }
    Ok(())
}

fn make_private(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // Windows and macOS user folders are already private; a Unix umask usually leaves them world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

fn state_path() -> PathBuf {
    data_dir().join("state.json")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Png,
    Jpeg,
    Webp,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp];

    pub fn ext(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Webp => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Png => "PNG",
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Webp => "WebP",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(ExportFormat::Png),
            "jpg" | "jpeg" => Some(ExportFormat::Jpeg),
            "webp" => Some(ExportFormat::Webp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowCaptureStyle {
    /// The window pixels only.
    Plain,
    /// Transparent margin with a drop shadow.
    Shadow,
    /// The window placed on the default background.
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuickAccessSide {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub all_in_one: String,
    pub capture_area: String,
    pub capture_previous_area: String,
    pub capture_fullscreen: String,
    pub capture_window: String,
    pub scrolling_capture: String,
    pub self_timer: String,
    pub capture_text: String,
    pub open_history: String,
    pub restore_recently_closed: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        // Kept owns Alt+V and Alt+Shift+V, and Windows switches keyboard layouts on Alt+Shift,
        // so every default lives on Ctrl+Shift.
        Self {
            all_in_one: "Ctrl+Shift+Digit5".into(),
            capture_area: "Ctrl+Shift+Digit4".into(),
            capture_previous_area: "Ctrl+Shift+Digit8".into(),
            capture_fullscreen: "Ctrl+Shift+Digit3".into(),
            capture_window: "Ctrl+Shift+Digit6".into(),
            scrolling_capture: "Ctrl+Shift+Digit7".into(),
            self_timer: String::new(),
            capture_text: "Ctrl+Shift+Digit2".into(),
            open_history: "Ctrl+Shift+KeyH".into(),
            restore_recently_closed: String::new(),
        }
    }
}

impl Hotkeys {
    /// (label, CLI subcommand, binding) for every bindable action.
    pub fn entries_mut(&mut self) -> [(&'static str, &'static str, &mut String); 10] {
        [
            ("All-In-One", "all-in-one", &mut self.all_in_one),
            ("Capture Area", "capture-area", &mut self.capture_area),
            ("Capture Previous Area", "capture-previous-area", &mut self.capture_previous_area),
            ("Capture Fullscreen", "capture-fullscreen", &mut self.capture_fullscreen),
            ("Capture Window", "capture-window", &mut self.capture_window),
            ("Scrolling Capture", "scrolling-capture", &mut self.scrolling_capture),
            ("Self-Timer", "self-timer", &mut self.self_timer),
            ("Capture Text (OCR)", "capture-text", &mut self.capture_text),
            ("Open History", "open-history", &mut self.open_history),
            ("Restore Recently Closed", "restore-recently-closed", &mut self.restore_recently_closed),
        ]
    }

    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let mut copy = self.clone();
        copy.entries_mut().into_iter().map(|(_, cmd, key)| (cmd, key.clone())).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadSettings {
    /// Endpoint receiving a `multipart/form-data` POST. Empty disables uploads.
    pub url: String,
    pub file_field: String,
    /// Extra request headers, one `Name: value` per entry (e.g. authorization).
    pub headers: Vec<String>,
    /// JSON path to the link in the response (`data.link`); empty means the body is the link.
    pub response_path: String,
}

impl Default for UploadSettings {
    fn default() -> Self {
        Self { url: String::new(), file_field: "file".into(), headers: Vec::new(), response_path: String::new() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeptSettings {
    /// Point Kept's image editor at this binary, and offer its recent clips in the editor.
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub background: Background,
    /// Numbered-step settings: a preset brings back the counter look as well as the background.
    #[serde(default)]
    pub counter: Option<keptcraft_doc::model::CounterStyle>,
    /// The comment band's look, without the comments themselves.
    #[serde(default)]
    pub commentary: Option<keptcraft_doc::model::Commentary>,
}

/// The extension a shared preset travels in. The file is a preset's own JSON: a background image or
/// watermark is already part of it as base64 PNG, so the one file carries everything.
pub const PRESET_EXT: &str = "keptpreset";
/// The extension a preset had before the rename; those files still import.
pub const LEGACY_PRESET_EXT: &str = "shotpreset";

impl Preset {
    /// Write this preset where another copy of KeptCraft can read it.
    pub fn save_file(&self, path: &Path) -> Result<()> {
        write_json(path, self)
    }

    /// Read a preset written by [`Preset::save_file`], with the comments it may carry dropped.
    pub fn load_file(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let mut preset: Self = serde_json::from_slice(&bytes)?;
        preset.name = preset.name.trim().to_owned();
        if preset.name.is_empty() {
            preset.name = path.file_stem().map_or_else(|| "Imported preset".to_owned(), |n| n.to_string_lossy().to_string());
        }
        if let Some(commentary) = &mut preset.commentary {
            commentary.entries.clear();
        }
        Ok(preset)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub save_dir: PathBuf,
    pub export_format: ExportFormat,
    pub jpeg_quality: u8,
    pub show_quick_access: bool,
    pub show_in_taskbar: bool,
    pub auto_create_projects: bool,
    pub copy_after_capture: bool,
    pub save_after_capture: bool,
    pub show_crosshair: bool,
    pub show_magnifier: bool,
    pub window_style: WindowCaptureStyle,
    pub quick_access_side: QuickAccessSide,
    /// Seconds before the Quick Access Overlay closes itself; 0 keeps it open.
    pub quick_access_auto_close: u32,
    pub close_quick_access_after_action: bool,
    pub timer_seconds: u32,
    pub ocr_keep_linebreaks: bool,
    pub history_days: u32,
    /// How many of your own recent edits the start screen shows.
    pub recent_edits: u32,
    /// How many of Kept's newest captures the start screen shows; 0 leaves them out.
    pub kept_captures: u32,
    pub hotkeys: Hotkeys,
    pub upload: UploadSettings,
    pub kept: KeptSettings,
    /// Custom backgrounds built in the background tool, offered beside the built-in gradients.
    pub gradients: Vec<OwnedGradient>,
    pub presets: Vec<Preset>,
    pub palette: Vec<[u8; 4]>,
    /// Check GitHub for a new version once a day when KeptCraft opens.
    pub update_auto: bool,
}

impl Default for Settings {
    fn default() -> Self {
        // Captures get a folder of their own: a Kept user should not find them on the Desktop, and
        // Kept watches this folder for edits (it reads `save_dir` from settings.json).
        let save_dir = dirs::picture_dir().or_else(dirs::desktop_dir).or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("KeptCraft");
        Self {
            save_dir,
            export_format: ExportFormat::Png,
            jpeg_quality: 90,
            show_quick_access: true,
            show_in_taskbar: true,
            auto_create_projects: false,
            // Every capture reaches Kept through the clipboard without a second action.
            copy_after_capture: true,
            save_after_capture: false,
            show_crosshair: true,
            show_magnifier: true,
            window_style: WindowCaptureStyle::Shadow,
            quick_access_side: QuickAccessSide::Left,
            // The clip is already in Kept, so the card is a way to annotate rather than a keeper.
            quick_access_auto_close: 10,
            close_quick_access_after_action: true,
            timer_seconds: 5,
            ocr_keep_linebreaks: true,
            history_days: 30,
            recent_edits: 9,
            kept_captures: 30,
            hotkeys: Hotkeys::default(),
            upload: UploadSettings::default(),
            kept: KeptSettings::default(),
            gradients: Vec::new(),
            presets: Vec::new(),
            palette: default_palette(),
            // A screenshot tool that gets fixes quietly is worth more than one that has to be
            // remembered about; the check is one request a day and the update is the user's choice.
            update_auto: true,
        }
    }
}

/// The colours a fresh palette starts with. Solid Colors keeps these; anything you add can be removed.
pub fn default_palette() -> Vec<[u8; 4]> {
    vec![
        [255, 59, 48, 255],
        [255, 149, 0, 255],
        [255, 204, 0, 255],
        [52, 199, 89, 255],
        [10, 132, 255, 255],
        [175, 82, 222, 255],
        [255, 255, 255, 255],
        [28, 28, 30, 255],
    ]
}

impl Settings {
    pub fn load() -> Self {
        if let Some(settings) = read_json(&settings_path()) {
            return settings;
        }
        // The fork had other names before this one: a fresh folder inherits the newest settings found
        // there, so the save folder, palette and the Kept editor path survive the rename.
        let legacy = home_override()
            .is_none()
            .then(|| LEGACY_APP_DIRS.iter().filter_map(|dir| dirs::config_dir().map(|d| d.join(dir).join("settings.json"))).find(|path| path.is_file()))
            .flatten()
            .and_then(|legacy| read_json::<Self>(&legacy));
        match legacy {
            Some(settings) => {
                let _ = settings.save();
                settings
            }
            None => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        write_json(&settings_path(), self)
    }

    /// Change fields on the newest settings on disk, so edits other windows made since we loaded survive.
    pub fn update(f: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut settings = Self::load();
        f(&mut settings);
        settings.save()?;
        Ok(settings)
    }

    /// Save the fields that differ from `base` (what this window loaded) over the newest settings on disk.
    pub fn save_changes(&self, base: &Settings) -> Result<Settings> {
        let merged = merge_changes(Self::load(), base, self)?;
        merged.save()?;
        Ok(merged)
    }

    pub fn modified() -> Option<std::time::SystemTime> {
        std::fs::metadata(settings_path()).and_then(|m| m.modified()).ok()
    }
}

/// `disk` with every top-level field that `edited` changed relative to `base`.
fn merge_changes(disk: Settings, base: &Settings, edited: &Settings) -> Result<Settings> {
    let mut out = serde_json::to_value(disk)?;
    if let (Some(out), serde_json::Value::Object(base), serde_json::Value::Object(edited)) =
        (out.as_object_mut(), serde_json::to_value(base)?, serde_json::to_value(edited)?)
    {
        for (key, value) in edited {
            if base.get(&key) != Some(&value) {
                out.insert(key, value);
            }
        }
    }
    Ok(serde_json::from_value(out)?)
}

/// The last selected area, in physical pixels of the monitor it was taken on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastArea {
    pub monitor_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// An edge of the editor window a toolbar strip can dock to, or the window's own top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StripDock {
    Top,
    Bottom,
    Left,
    Right,
    /// In the top bar beside File and View, rather than on a row of its own.
    TopBar,
}

/// Where one of the editor's toolbar strips sits: on an edge, or floating inside the window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StripPlacement {
    pub visible: bool,
    /// The edge it is docked to; `None` floats the strip at `pos`.
    pub dock: Option<StripDock>,
    /// Top-left of a floating strip, in points from the window's top-left corner.
    pub pos: [f32; 2],
    /// Whether its buttons run down the screen; a torn-off strip keeps the way it was laid out.
    pub vertical: bool,
    /// Floating strips that share a card: 0 is a card of its own, anything else is a group id.
    pub group: u8,
}

impl Default for StripPlacement {
    fn default() -> Self {
        Self { visible: true, dock: None, pos: [80.0, 160.0], vertical: true, group: 0 }
    }
}

/// The strips the editor's tools are grouped into, and where each one sits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorLayout {
    /// The drawing tools, vertical on the left by default.
    pub tools: StripPlacement,
    /// Crop and the background tool.
    pub document: StripPlacement,
    /// The options that belong to the tool in use.
    pub properties: StripPlacement,
    /// Pixelate, spotlight, steps and the freehand tools.
    pub marks: StripPlacement,
    /// Pin, upload and Copy Text (OCR).
    pub share: StripPlacement,
    /// Rotate and flip.
    pub transform: StripPlacement,
}

impl Default for EditorLayout {
    fn default() -> Self {
        let placed = |dock, vertical, group| StripPlacement { visible: true, dock: Some(dock), pos: [0.0; 2], vertical, group };
        Self {
            // Crop and the background tool share the tools' column, so a fresh window is not all
            // bars: they sit under the drawing tools, on their own bar with a grip of their own.
            tools: placed(StripDock::Left, true, 1),
            document: placed(StripDock::Left, true, 1),
            // The tool options get the row under the top bar, where the settings for the tool in
            // use belong.
            properties: placed(StripDock::Top, false, 0),
            // The rest of the tools ride in the top bar beside File and View, leaving the canvas
            // the height of a window rather than a stack of rows.
            marks: placed(StripDock::TopBar, false, 0),
            share: placed(StripDock::TopBar, false, 0),
            transform: placed(StripDock::TopBar, false, 0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub last_area: Option<LastArea>,
    /// Where the editor's toolbar strips sit.
    pub editor_layout: EditorLayout,
    /// History ids of Quick Access Overlays closed without an action, most recent last.
    pub recently_closed: Vec<String>,
    /// Files opened in the editor, most recent last: the File menu's Recent Files.
    pub recent_files: Vec<PathBuf>,
    /// Width of the comment panel, in points; the user drags it.
    pub commentary_width: Option<f32>,
    /// Whether the comment panel was left collapsed to its rail.
    pub commentary_collapsed: Option<bool>,
    /// Width of the background panel when it is open, in points.
    pub background_width: Option<f32>,
    /// Whether the background panel was left collapsed to its rail.
    pub background_collapsed: Option<bool>,
    /// Unix time of the last check for a new release; an automatic check waits a day.
    pub update_checked_at: Option<i64>,
    /// A version the user asked not to be told about again.
    pub update_skipped: Option<String>,
}

impl State {
    pub fn load() -> Self {
        read_json(&state_path()).unwrap_or_default()
    }

    /// Load, modify and save in one step so concurrent processes lose as little as possible.
    pub fn update(f: impl FnOnce(&mut State)) {
        let mut state = Self::load();
        f(&mut state);
        if let Err(err) = write_json(&state_path(), &state) {
            eprintln!("failed to save state: {err:#}");
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(err) => {
            // Set the file aside rather than let the next save replace it with defaults.
            let mut kept = path.as_os_str().to_owned();
            kept.push(".corrupt");
            let kept = PathBuf::from(kept);
            match std::fs::rename(path, &kept) {
                Ok(()) => {
                    eprintln!("unreadable {} ({err}); moved to {}", path.display(), kept.display());
                    crate::share::warn("Unreadable file set aside", &format!("Using defaults. The old file is now {}", kept.display()));
                }
                Err(e) => eprintln!("ignoring unreadable {}: {err} (could not set it aside: {e})", path.display()),
            }
            None
        }
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::doc::write_atomic(path, &serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fork's defaults are shaped for Kept, so a fresh install needs no settings trip first.
    #[test]
    fn defaults_suit_a_clip_manager() {
        let s = Settings::default();
        assert!(s.copy_after_capture, "a capture should reach Kept without a second action");
        assert!(s.show_in_taskbar);
        assert!(!s.auto_create_projects, "project sidecars should require opting in");
        assert_eq!(s.save_dir.file_name().and_then(|n| n.to_str()), Some("KeptCraft"), "captures get their own folder, not the Desktop");
        assert!(s.quick_access_auto_close > 0, "the card should not linger once the clip is in Kept");
        assert!(s.update_auto, "a new version should be offered without a settings trip");
        // Kept binds Alt+V and Alt+Shift+V, and Windows switches keyboard layouts on Alt+Shift.
        let keys = s.hotkeys.entries().into_iter().map(|(_, key)| key).filter(|key| !key.is_empty());
        for key in keys {
            assert!(key.starts_with("Ctrl+Shift+"), "{key} leaves the modifier family Kept and Windows use");
        }
    }

    /// The start screen fills a fresh window without a settings trip first.
    #[test]
    fn start_screen_shows_nine_edits_and_thirty_captures() {
        let s = Settings::default();
        assert_eq!(s.recent_edits, 9);
        assert_eq!(s.kept_captures, 30);
    }

    /// The editor opens with the drawing tools down the left and the other bars along the top,
    /// which is the bar KeptCraft has always had, plus the tools moved out of it.
    #[test]
    fn the_editor_starts_with_the_tools_on_the_left_and_the_rest_on_top() {
        let layout = EditorLayout::default();
        assert_eq!(layout.tools.dock, Some(StripDock::Left));
        assert!(layout.tools.vertical, "the drawing tools read down the screen");
        assert_eq!(layout.document.dock, Some(StripDock::Left), "crop and background sit under the tools");
        assert_eq!(layout.document.group, layout.tools.group, "on the tools' own bar");
        assert_eq!(layout.properties.dock, Some(StripDock::Top), "the tool options get the row under the top bar");
        assert!(!layout.properties.vertical);
        for strip in [&layout.marks, &layout.share, &layout.transform] {
            assert_eq!(strip.dock, Some(StripDock::TopBar), "the rest of the tools ride in the top bar");
            assert!(!strip.vertical, "a strip in the top bar lies along it");
            assert!(strip.visible);
        }
    }

    /// A state file written before the strips existed still loads, and a moved strip comes back
    /// where it was left.
    #[test]
    fn a_state_file_without_a_layout_gets_the_default_one() {
        let empty: State = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.editor_layout, EditorLayout::default());

        let moved = EditorLayout { share: StripPlacement { visible: false, dock: None, pos: [120.0, 240.0], vertical: true, group: 3 }, ..Default::default() };
        let back: EditorLayout = serde_json::from_str(&serde_json::to_string(&moved).unwrap()).unwrap();
        assert_eq!(back, moved);
    }

    #[test]
    fn merge_keeps_fields_other_windows_changed() {
        let base = Settings::default();
        let mut disk = base.clone();
        disk.palette.push([1, 2, 3, 255]);
        let mut edited = base.clone();
        edited.upload.url = "https://example.test/upload".into();
        let merged = merge_changes(disk, &base, &edited).unwrap();
        assert_eq!(merged.upload.url, "https://example.test/upload");
        assert_eq!(merged.palette.last(), Some(&[1, 2, 3, 255]));
    }

    #[test]
    fn presets_carry_the_counter_and_comment_look_and_old_ones_still_load() {
        use keptcraft_doc::model::{Commentary, CommentarySide, CounterLabel, CounterStyle};
        let style = CounterStyle { size: 36.0, label: CounterLabel::Letters, ..Default::default() };
        let commentary = Commentary {
            side: CommentarySide::Right,
            width: 320.0,
            spacing: 12.0,
            padding: 18.0,
            text_color: keptcraft_doc::model::Commentary::default().text_color,
            header: Some("Notes".into()),
            ..Default::default()
        };
        let preset = Preset { name: "Talk".into(), background: Background::default(), counter: Some(style.clone()), commentary: Some(commentary.clone()) };
        let json = serde_json::to_string(&preset).unwrap();
        let back: Preset = serde_json::from_str(&json).unwrap();
        assert_eq!(back.counter, Some(style), "the preset brings the counter look back");
        assert_eq!(back.commentary, Some(commentary), "and every comment band setting with it");

        // A preset saved before either existed carries neither key.
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.as_object_mut().is_some_and(|o| o.remove("counter").is_some() && o.remove("commentary").is_some()), "the keys are there to drop");
        let old: Preset = serde_json::from_value(value).unwrap();
        assert_eq!(old.counter, None, "an old preset loads with the default counter style");
        assert_eq!(old.commentary, None, "and no comment band settings");
    }

    #[test]
    fn a_preset_file_carries_its_own_images() {
        let image = image::RgbaImage::from_pixel(3, 2, image::Rgba([9, 8, 7, 255]));
        let preset = Preset {
            name: "Brand".into(),
            background: Background { fill: crate::doc::model::BgFill::Image(std::sync::Arc::new(image)), ..Background::default() },
            counter: None,
            commentary: None,
        };
        let path = std::env::temp_dir().join(format!("keptcraft-preset-{}.{PRESET_EXT}", std::process::id()));
        preset.save_file(&path).unwrap();
        // One file: the image travels inside the JSON as base64 PNG, so nothing else is needed.
        assert_eq!(Preset::load_file(&path).unwrap(), preset, "the image comes back with the preset");
        std::fs::remove_file(&path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn private_dir_is_owner_only_even_if_it_existed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("keptcraft-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        make_private(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&dir).ok();
    }
}
