//! Annotate window: markup tools, redaction, crop, background tool, presets and export.
//!
//! The canvas shows the document rendered by [`render`] at screen resolution, so the
//! preview is pixel-for-pixel what gets saved. Only handles and the live text editor are
//! drawn by egui on top.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Key, Mesh, Pos2, Rect, RichText, Sense, Shape as EShape, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;
use image::RgbaImage;

use shotcraft_engine::config::{ExportFormat, Preset, Settings, State};
use shotcraft_engine::doc::fonts;
use shotcraft_engine::doc::model::{
    Anchor, ArrowStyle, Aspect, Background, BgFill, Commentary, CommentarySide, CounterLabel, CounterStyle, Doc, GRADIENTS, GradientStop, Item, Orient,
    OwnedGradient, PROJECT_EXT, RedactMode, Shape, TextStyle, Watermark, constrain_aspect,
};
use shotcraft_engine::doc::render::{self, RenderOpts};
use shotcraft_engine::{capture, history, ocr, share};

use crate::{dialogs, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Select,
    /// Drag the canvas around: the hand in the toolbar, or Space held down.
    Pan,
    Rect,
    FilledRect,
    Ellipse,
    Line,
    Arrow,
    Text,
    Counter,
    Pen,
    Highlighter,
    Redact,
    Spotlight,
}

/// Zoom bounds: 10 % of the fit and 16× it, the same range the slider and the shortcuts use.
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 16.0;

/// Room the background panel opens at: wide enough for a preset name and its two buttons.
const BACKGROUND_WIDTH: f32 = 272.0;
/// Room the comment panel opens at.
const COMMENTARY_WIDTH: f32 = 300.0;
/// Width of the collapsed rail the panels fold down to.
const RAIL_WIDTH: f32 = 40.0;

const TOOLS: [(Tool, theme::ToolIcon, &str, Key); 12] = [
    (Tool::Select, theme::ToolIcon::Glyph(icon::CURSOR), "Select (V)", Key::V),
    (Tool::Rect, theme::ToolIcon::Glyph(icon::SQUARE), "Rectangle (R)", Key::R),
    (Tool::FilledRect, theme::ToolIcon::FilledSquare, "Filled rectangle (F)", Key::F),
    (Tool::Ellipse, theme::ToolIcon::Glyph(icon::CIRCLE), "Ellipse (O)", Key::O),
    (Tool::Line, theme::ToolIcon::Glyph(icon::LINE_SEGMENT), "Line (L)", Key::L),
    (Tool::Arrow, theme::ToolIcon::Glyph(icon::ARROW_UP_RIGHT), "Arrow (A)", Key::A),
    (Tool::Text, theme::ToolIcon::Glyph(icon::TEXT_T), "Text (T)", Key::T),
    (Tool::Redact, theme::ToolIcon::Glyph(icon::CHECKERBOARD), "Pixelate / Blur (P)", Key::P),
    (Tool::Spotlight, theme::ToolIcon::Glyph(icon::FLASHLIGHT), "Spotlight (S)", Key::S),
    (Tool::Counter, theme::ToolIcon::Glyph(icon::NUMBER_CIRCLE_ONE), "Counter (N)", Key::N),
    (Tool::Pen, theme::ToolIcon::Glyph(icon::PENCIL_SIMPLE), "Pencil (D)", Key::D),
    (Tool::Highlighter, theme::ToolIcon::Glyph(icon::HIGHLIGHTER), "Highlighter (H)", Key::H),
];

#[derive(Clone)]
struct Snapshot {
    base: Arc<RgbaImage>,
    items: Vec<Item>,
    crop: Option<Rect>,
    background: Option<Background>,
}

#[derive(Clone, Copy)]
enum EditDrag {
    Create {
        index: usize,
        start: Pos2,
    },
    Move {
        index: usize,
        last: Pos2,
    },
    Handle {
        index: usize,
        handle: usize,
    },
    Pan {
        last: Pos2,
    },
    /// Dragging the watermark, in composed-canvas coordinates.
    Watermark {
        last: Pos2,
    },
}

enum CropAction {
    Done,
    Cancel,
    Reset,
    Aspect(Option<(u32, u32)>),
}

/// What the colour window does when it closes.
enum ColourAction {
    Use(Color32),
    Add(Color32),
    Save(Color32),
    Delete,
    Cancel,
}

#[derive(Clone, Copy)]
enum CropDrag {
    New(Pos2),
    Move(Pos2),
    Corner(usize),
}

struct CropState {
    rect: Rect,
    aspect: Option<(u32, u32)>,
    drag: Option<CropDrag>,
}

/// Mapping between screen points and base pixels for the current frame.
#[derive(Clone, Copy)]
struct View {
    img_rect: Rect,
    region: Rect,
    offset: Vec2,
    out_scale: f32,
    out_size: Vec2,
}

impl Default for View {
    fn default() -> Self {
        Self { img_rect: Rect::ZERO, region: Rect::ZERO, offset: Vec2::ZERO, out_scale: 1.0, out_size: Vec2::splat(1.0) }
    }
}

impl View {
    fn to_base(self, p: Pos2) -> Pos2 {
        let out = (p - self.img_rect.min) * (self.out_size.x / self.img_rect.width());
        self.region.min + (out - self.offset) / self.out_scale
    }

    fn to_screen(self, b: Pos2) -> Pos2 {
        let out = self.offset + (b - self.region.min) * self.out_scale;
        self.img_rect.min + out * (self.img_rect.width() / self.out_size.x)
    }

    /// Where a screen point lands on the composed canvas, background included.
    fn to_canvas(self, p: Pos2) -> Pos2 {
        let v: Vec2 = (p - self.img_rect.min) * (self.out_size.x / self.img_rect.width());
        pos2(v.x, v.y)
    }

    /// Screen points per base pixel.
    fn ppb(self) -> f32 {
        self.out_scale * self.img_rect.width() / self.out_size.x
    }
}

struct Editor {
    doc: Doc,
    history_id: Option<String>,
    project_path: Option<PathBuf>,
    /// Where Save writes when the caller chose a destination (`open-annotate --save-to`).
    save_target: Option<PathBuf>,
    /// Files the File menu offers under Recent Files, most recent last.
    recent: Vec<PathBuf>,
    /// The gradient being built in the background panel's popup, while it is open.
    gradient_draft: Option<OwnedGradient>,
    /// Which of the user's gradients the draft rewrites, when it was opened from a tile.
    gradient_edit: Option<usize>,
    /// The solid colour being chosen, and the palette entry it rewrites when editing.
    colour_draft: Option<Color32>,
    colour_edit: Option<usize>,
    settings: Settings,
    tool: Tool,
    color: Color32,
    width: f32,
    arrow_style: ArrowStyle,
    text_style: TextStyle,
    text_size: f32,
    /// New text wraps at the image's right edge instead of running off it.
    text_paragraph: bool,
    /// Defaults for new numbered steps, including the ones a preset restores.
    counter_style: CounterStyle,
    redact_mode: RedactMode,
    spotlight_ellipse: bool,
    selected: Option<usize>,
    drag: Option<EditDrag>,
    editing: Option<(usize, String)>,
    focus_text: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    zoom: f32,
    pan: Vec2,
    /// Space held: a temporary hand, so a drag moves the canvas instead of drawing.
    space_down: bool,
    show_background: bool,
    crop: Option<CropState>,
    picking_color: bool,
    export_percent: u32,
    view: View,
    tex: Option<TextureHandle>,
    render_key: Option<(u64, u32)>,
    base_cache: Option<(u32, RgbaImage)>,
    revision: u64,
    saved_revision: u64,
    status: Option<(String, Instant)>,
    task: Option<(&'static str, Receiver<anyhow::Result<String>>)>,
    confirm_close: bool,
    allow_close: bool,
    preset_name: String,
    /// The preset last loaded or saved, so a tweak can be written back to it.
    preset_active: Option<String>,
    /// Height of the comment panel, as last dragged.
    commentary_width: f32,
    /// The comment panel is a rail until it is opened again.
    commentary_collapsed: bool,
    /// The comment panel is on screen.
    show_commentary: bool,
    /// Width of the background panel, as last dragged.
    background_width: f32,
    /// The background panel is a rail until it is opened again.
    background_collapsed: bool,
    /// A panel size changed during a drag and still needs writing to the state file.
    panels_dirty: bool,
}

/// `save_target` is the file Save writes, for a caller that needs a known result path (a clip
/// manager, a script); without it Save exports a new timestamped file into the save folder.
pub fn run(doc: Doc, history_id: Option<String>, path: Option<PathBuf>, save_target: Option<PathBuf>) -> anyhow::Result<()> {
    let screen = capture::primary_logical_rect();
    let scale = capture::primary_scale();
    let natural = vec2(doc.base.width() as f32, doc.base.height() as f32) / scale + vec2(80.0, 140.0);
    // Not `clamp`: it panics when a small screen makes the upper bound less than the minimum.
    let size = natural.max(vec2(1300.0, 740.0)).min(screen.size() * 0.88);
    let project_path = path.as_ref().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXT))).cloned();
    // Name the file in the title: several editors can be open, and one of them is a caller's clip.
    let title = match path.as_ref().or(project_path.as_ref()).and_then(|p| p.file_name()) {
        Some(name) => format!("{} — ShotCraft4Kept", name.to_string_lossy()),
        None => "ShotCraft4Kept — Annotate".to_owned(),
    };
    let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size(size).with_min_inner_size(vec2(1040.0, 520.0)).with_drag_and_drop(true);
    let settings = Settings::load();
    let color = settings.palette.first().map_or(Color32::from_rgb(255, 59, 48), |c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]));
    let show_background = doc.background.is_some();
    let state = State::load();
    let commentary_width = state.commentary_width.unwrap_or(COMMENTARY_WIDTH);
    let commentary_collapsed = state.commentary_collapsed.unwrap_or(false);
    let show_commentary = doc.commentary.is_some();
    let background_width = state.background_width.unwrap_or(BACKGROUND_WIDTH);
    let background_collapsed = state.background_collapsed.unwrap_or(false);
    let mut recent = state.recent_files;
    if let Some(path) = path.as_ref() {
        remember_recent(&mut recent, path);
    }
    theme::run("shotcraft-editor", builder, move |_| Editor {
        doc,
        history_id,
        project_path,
        save_target,
        recent,
        gradient_draft: None,
        gradient_edit: None,
        colour_draft: None,
        colour_edit: None,
        settings,
        tool: Tool::Arrow,
        color,
        width: 6.0,
        arrow_style: ArrowStyle::Tapered,
        text_style: TextStyle::Plain,
        text_size: 32.0,
        text_paragraph: false,
        counter_style: CounterStyle::default(),
        redact_mode: RedactMode::Pixelate,
        spotlight_ellipse: false,
        selected: None,
        drag: None,
        editing: None,
        focus_text: false,
        undo: Vec::new(),
        redo: Vec::new(),
        zoom: 1.0,
        pan: Vec2::ZERO,
        space_down: false,
        show_background,
        crop: None,
        picking_color: false,
        export_percent: 100,
        view: View::default(),
        tex: None,
        render_key: None,
        base_cache: None,
        revision: 0,
        saved_revision: 0,
        status: None,
        task: None,
        confirm_close: false,
        allow_close: false,
        preset_name: String::new(),
        preset_active: None,
        commentary_width,
        commentary_collapsed,
        show_commentary,
        background_width,
        background_collapsed,
        panels_dirty: false,
    })
}

fn swatch_color(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Keep `path` at the end of the recent list, the newest last, and on disk for the next window.
fn remember_recent(recent: &mut Vec<PathBuf>, path: &Path) {
    recent.retain(|p| p != path);
    recent.push(path.to_path_buf());
    let excess = recent.len().saturating_sub(10);
    recent.drain(..excess);
    let saved = recent.clone();
    State::update(|s| s.recent_files = saved);
}

/// Tell the caller that chose the destination which file it got. The editor has no console of its
/// own on Windows, so a failed write is not an error; the file itself is the contract.
fn announce_save(path: &Path) {
    use std::io::Write as _;
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{}", path.display());
    let _ = out.flush();
}

impl Editor {
    fn snapshot(&self) -> Snapshot {
        Snapshot { base: self.doc.base.clone(), items: self.doc.items.clone(), crop: self.doc.crop, background: self.doc.background.clone() }
    }

    fn restore(&mut self, s: Snapshot) {
        self.doc.base = s.base;
        self.doc.items = s.items;
        self.doc.crop = s.crop;
        self.doc.background = s.background;
        self.selected = self.selected.filter(|&i| i < self.doc.items.len());
        self.base_cache = None;
        self.touch();
    }

    /// Record the current state for undo, before a change.
    fn checkpoint(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn touch(&mut self) {
        self.revision += 1;
    }

    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(s);
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(s);
        }
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn report<T, E: std::fmt::Display>(&mut self, r: Result<T, E>) -> Option<T> {
        r.map_err(|e| self.flash(format!("{e:#}"))).ok()
    }

    fn export_image(&self) -> shotcraft_engine::Result<RgbaImage> {
        Ok(render::resize_percent(&render::flatten(&self.doc)?, self.export_percent))
    }

    /// False only when the history copy could not be written.
    fn sync_history(&mut self) -> bool {
        let Some(id) = self.history_id.clone() else { return true };
        let r = history::update(&id, &self.doc);
        self.report(r).is_some()
    }

    /// Apply one settings change to the newest settings on disk, so it can't undo another window's edits.
    fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) -> bool {
        let r = Settings::update(f);
        let Some(settings) = self.report(r) else { return false };
        self.settings = settings;
        true
    }

    fn copy(&mut self) {
        let r = self.export_image().and_then(|img| share::copy_image(&img));
        if self.report(r).is_some() {
            self.sync_history();
            self.flash("Copied to clipboard");
        }
    }

    fn save(&mut self) {
        // A caller that named the output (`--save-to`) gets exactly that file, even for a project.
        if let Some(target) = self.save_target.clone() {
            let r = self.export_image().and_then(|img| share::save_to(&img, &target, &self.settings));
            if let Some(path) = self.report(r) {
                self.saved_revision = self.revision;
                self.sync_history();
                self.flash(format!("Saved {}", path.display()));
                announce_save(&path);
            }
            return;
        }
        if let Some(path) = self.project_path.clone() {
            let r = self.doc.save_project(&path).map_err(|e| format!("Could not save {}: {e}", path.display()));
            if self.report(r).is_some() {
                self.saved_revision = self.revision;
                self.flash(format!("Saved {}", path.display()));
            }
            return;
        }
        let r = self.export_image().and_then(|img| share::save_default(&img, &self.settings));
        if let Some(path) = self.report(r) {
            self.saved_revision = self.revision;
            self.sync_history();
            self.flash(format!("Saved {}", path.display()));
        }
    }

    fn save_as(&mut self) {
        let r = self.export_image().and_then(|img| dialogs::save_as(&img, &self.settings));
        if let Some(Some(path)) = self.report(r) {
            self.saved_revision = self.revision;
            self.sync_history();
            remember_recent(&mut self.recent, &path);
            self.flash(format!("Saved {}", path.display()));
        }
    }

    /// The File menu: the editor's own document actions, separate from the tools and the ⋯ menu.
    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let has_recent = !self.recent.is_empty();
        ui.menu_button(format!("{}  File", icon::FOLDER_SIMPLE), |ui| {
            ui.set_min_width(240.0);
            if ui.button(format!("{}  Open…", icon::FOLDER_OPEN)).on_hover_text("Open an image or a project (Ctrl/Cmd+O)").clicked() {
                ui.close();
                if let Some(path) = dialogs::pick_image() {
                    self.open_path(&ctx, path);
                }
            }
            ui.add_enabled_ui(has_recent, |ui| {
                ui.menu_button(format!("{}  Recent Files", icon::CLOCK_COUNTER_CLOCKWISE), |ui| {
                    ui.set_min_width(280.0);
                    for path in self.recent.clone().into_iter().rev() {
                        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string());
                        if ui.button(name).on_hover_text(path.display().to_string()).clicked() {
                            ui.close();
                            self.open_path(&ctx, path);
                        }
                    }
                });
            });
            if ui.button(format!("{}  Import from Clipboard", icon::CLIPBOARD)).clicked() {
                ui.close();
                let image = self.report(share::clipboard_image());
                if let Some(image) = image {
                    self.load_document(Doc::new(image), None);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title("ShotCraft4Kept — Annotate".to_owned()));
                }
            }
            self.kept_menu(ui, &ctx);
            ui.separator();
            if ui.button(format!("{}  Save", icon::FLOPPY_DISK)).on_hover_text("Save (Ctrl/Cmd+S)").clicked() {
                ui.close();
                self.save();
            }
            if ui.button(format!("{}  Save As…", icon::DOWNLOAD_SIMPLE)).on_hover_text("Save As (Shift+Ctrl/Cmd+S)").clicked() {
                ui.close();
                self.save_as();
            }
            ui.separator();
            if ui.button(format!("{}  Close", icon::X)).on_hover_text("Close (Ctrl/Cmd+W)").clicked() {
                ui.close();
                // The viewport's close request, so unsaved project edits still ask first.
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Kept's newest image clips, when the integration is on: one click opens a clip for annotation.
    fn kept_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.settings.kept.enabled {
            return;
        }
        let Some(kept) = shotcraft_engine::kept::detect() else { return };
        let clips = match shotcraft_engine::kept::recent_images(&kept, 20) {
            Ok(clips) => clips,
            Err(err) => {
                ui.label(RichText::new("Kept's database is busy").size(12.0)).on_hover_text(format!("{err:#}"));
                return;
            }
        };
        ui.menu_button(format!("{}  Open from Kept DB", icon::DATABASE), |ui| {
            ui.set_min_width(340.0);
            for clip in &clips {
                if ui.button(clip.label()).on_hover_text(clip.path.display().to_string()).clicked() {
                    ui.close();
                    match image::open(&clip.path) {
                        Ok(image) => {
                            self.load_document(Doc::new(image.into_rgba8()), None);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Title("Kept clip — ShotCraft4Kept".to_owned()));
                        }
                        Err(err) => self.flash(format!("Could not open the Kept clip: {err}")),
                    }
                }
            }
        });
    }

    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        match Doc::open(&path) {
            Ok(doc) => {
                let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().to_string());
                self.load_document(doc, Some(path));
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — ShotCraft4Kept")));
            }
            Err(err) => self.flash(format!("Could not open {}: {err}", path.display())),
        }
    }

    /// Show a different document in this window; the old one's history id and save target go with it.
    fn load_document(&mut self, doc: Doc, path: Option<PathBuf>) {
        if let Some(path) = path.as_ref() {
            remember_recent(&mut self.recent, path);
        }
        self.doc = doc;
        self.show_background = self.doc.background.is_some();
        self.project_path = path.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXT)));
        self.history_id = None;
        self.save_target = None;
        self.undo.clear();
        self.redo.clear();
        self.selected = None;
        self.editing = None;
        self.crop = None;
        self.base_cache = None;
        self.tex = None;
        self.render_key = None;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.revision += 1;
        self.saved_revision = self.revision;
        self.status = None;
    }

    fn save_project_as(&mut self) {
        let name = format!("{}.{PROJECT_EXT}", share::default_file_name(ExportFormat::Png).trim_end_matches(".png"));
        let Some(chosen) =
            rfd::FileDialog::new().set_directory(&self.settings.save_dir).set_file_name(name).add_filter("ShotCraft project", &[PROJECT_EXT]).save_file()
        else {
            return;
        };
        let path = if chosen.extension().is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXT)) {
            chosen
        } else {
            // Append rather than replace: "Shot 18.25.31" must not become "Shot 18.25.shotcraft".
            let mut name = chosen.into_os_string();
            name.push(".");
            name.push(PROJECT_EXT);
            let path = PathBuf::from(name);
            // The dialog only confirmed overwriting the name it showed.
            if path.exists() {
                self.flash(format!("{} already exists; choose another name", path.display()));
                return;
            }
            path
        };
        let r = self.doc.save_project(&path).map_err(|e| format!("Could not save {}: {e}", path.display()));
        if self.report(r).is_some() {
            self.saved_revision = self.revision;
            self.flash(format!("Saved project {}", path.display()));
            self.project_path = Some(path);
        }
    }

    fn ensure_history(&mut self) -> Option<String> {
        if self.history_id.is_none() {
            let r = history::add(&self.doc, "edit");
            self.history_id = self.report(r).map(|e| e.id);
        } else {
            self.sync_history();
        }
        self.history_id.clone()
    }

    fn pin(&mut self) {
        if let Some(id) = self.ensure_history() {
            share::spawn(["pin", "--history", &id]);
        }
    }

    fn background_task(&mut self, label: &'static str, ctx: &egui::Context, job: impl FnOnce(RgbaImage, Settings) -> anyhow::Result<String> + Send + 'static) {
        if self.task.is_some() {
            return;
        }
        let r = self.export_image();
        let Some(img) = self.report(r) else { return };
        let (tx, rx) = channel();
        let (settings, ctx2) = (self.settings.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job(img, settings));
            ctx2.request_repaint();
        });
        self.task = Some((label, rx));
    }

    fn upload(&mut self, ctx: &egui::Context) {
        self.sync_history();
        self.background_task("Uploading…", ctx, |img, settings| {
            let link = share::upload(&img, &settings)?;
            share::copy_text(&link)?;
            Ok(format!("Link copied: {link}"))
        });
    }

    fn copy_text(&mut self, ctx: &egui::Context) {
        self.background_task("Recognizing text…", ctx, |img, settings| {
            let text = ocr::extract(&img, settings.ocr_keep_linebreaks)?;
            anyhow::ensure!(!text.trim().is_empty(), "no text found");
            share::copy_text(&text)?;
            Ok("Text copied".into())
        });
    }

    fn new_item(&self, shape: Shape) -> Item {
        Item { shape, color: self.color, width: self.width }
    }

    /// The shape a drag with the current tool starts at `b`; `None` for tools that don't draw by dragging.
    fn new_shape(&self, b: Pos2) -> Option<Shape> {
        let r = Rect::from_two_pos(b, b);
        Some(match self.tool {
            Tool::Rect => Shape::Rect { rect: r, filled: false },
            Tool::FilledRect => Shape::Rect { rect: r, filled: true },
            Tool::Ellipse => Shape::Ellipse { rect: r, filled: false },
            Tool::Line => Shape::Line { a: b, b },
            Tool::Arrow => Shape::Arrow { a: b, b, style: self.arrow_style, bend: if self.arrow_style == ArrowStyle::Curved { 0.25 } else { 0.0 } },
            Tool::Pen => Shape::Pen { pts: vec![b] },
            Tool::Highlighter => Shape::Highlighter { pts: vec![b] },
            Tool::Redact => Shape::Redact { rect: r, mode: self.redact_mode, seed: rand_seed() },
            Tool::Spotlight => Shape::Spotlight { rect: r, ellipse: self.spotlight_ellipse },
            Tool::Select | Tool::Pan | Tool::Text | Tool::Counter => return None,
        })
    }

    fn add_image_item(&mut self, img: RgbaImage) {
        let content = self.doc.content_rect();
        let size = vec2(img.width() as f32, img.height() as f32);
        let k = (content.width() * 0.5 / size.x).min(content.height() * 0.5 / size.y).min(1.0);
        let rect = Rect::from_center_size(content.center(), size * k);
        self.checkpoint();
        self.doc.items.push(Item { shape: Shape::Image { rect, image: Arc::new(img) }, color: Color32::WHITE, width: 0.0 });
        self.selected = Some(self.doc.items.len() - 1);
        self.tool = Tool::Select;
        self.touch();
    }

    fn reorient(&mut self, op: Orient) {
        self.commit_text();
        self.checkpoint();
        self.doc.reorient(op);
        self.base_cache = None;
        self.touch();
    }

    fn commit_text(&mut self) {
        let Some((i, text)) = self.editing.take() else { return };
        if i >= self.doc.items.len() {
            return;
        }
        if text.trim().is_empty() {
            self.doc.items.remove(i);
            self.selected = None;
        } else if let Shape::Text { text: t, .. } = &mut self.doc.items[i].shape {
            *t = text;
        }
        self.touch();
    }

    fn start_text_edit(&mut self, i: usize) {
        if let Shape::Text { text, .. } = &self.doc.items[i].shape {
            self.editing = Some((i, text.clone()));
            self.selected = Some(i);
            self.focus_text = true;
            self.touch();
        }
    }

    /// Apply the toolbar's style to the selected item.
    fn restyle_selected(&mut self, f: impl FnOnce(&mut Item)) {
        if let Some(i) = self.selected.filter(|&i| i < self.doc.items.len()) {
            self.checkpoint();
            f(&mut self.doc.items[i]);
            self.touch();
        }
    }

    fn select_tool(&mut self, tool: Tool) {
        self.commit_text();
        self.tool = tool;
        if tool != Tool::Select {
            self.selected = None;
        }
        self.crop = None;
    }

    /// True while a drag should move the canvas: the hand tool, or Space held down.
    fn hand(&self) -> bool {
        self.tool == Tool::Pan || self.space_down
    }

    /// Zoom to `zoom`, keeping the base pixel under `anchor` where it is on screen.
    fn zoom_to(&mut self, zoom: f32, anchor: Option<Pos2>) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let factor = zoom / self.zoom;
        if (factor - 1.0).abs() < 1e-4 {
            return;
        }
        // `img_rect` is the image as drawn last frame, so its centre is the canvas centre plus the pan.
        if let Some(p) = anchor.filter(|_| self.view.img_rect.width() > 0.0) {
            self.pan = zoomed_pan(self.pan, self.view.img_rect.center() - self.pan, p, factor);
        }
        self.zoom = zoom;
    }

    fn zoom_by(&mut self, factor: f32) {
        self.zoom_to(self.zoom * factor, None);
    }

    fn render_if_needed(&mut self, ctx: &egui::Context, canvas: Rect) {
        let crop_mode = self.crop.is_some();
        let opts = |scale: f32| RenderOpts {
            scale,
            hide: self.editing.as_ref().map(|e| e.0),
            ignore_crop: crop_mode,
            background: self.show_background || self.doc.background.is_some(),
        };
        let full = render::output_size(&self.doc, &opts(1.0));
        let ppp = ctx.pixels_per_point();
        let fit = ((canvas.width() - 48.0) / full.x).min((canvas.height() - 48.0) / full.y).min(1.0 / ppp).max(0.01);
        let points_per_out1 = fit * self.zoom;
        // Render at physical-pixel resolution, never above the base image's own resolution.
        let scale = (points_per_out1 * ppp).min(1.0);
        let scale_key = (scale * 1000.0).round() as u32;
        let key = (self.revision, scale_key);
        if self.render_key != Some(key) || self.tex.is_none() {
            let base = match &self.base_cache {
                Some((k, scaled)) if *k == scale_key => scaled,
                _ => {
                    let base = &self.doc.base;
                    let scaled = if scale >= 0.999 {
                        (**base).clone()
                    } else {
                        let w = ((base.width() as f32 * scale).round() as u32).max(1);
                        let h = ((base.height() as f32 * scale).round() as u32).max(1);
                        image::imageops::resize(&**base, w, h, image::imageops::FilterType::Triangle)
                    };
                    &self.base_cache.insert((scale_key, scaled)).1
                }
            };
            match render::render(&self.doc, base, &opts(scale)) {
                Ok(out) => {
                    let size = [out.pixmap.width() as usize, out.pixmap.height() as usize];
                    let ci = egui::ColorImage::from_rgba_premultiplied(size, out.pixmap.data());
                    let filter = if self.zoom > 1.5 { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
                    match &mut self.tex {
                        Some(t) => t.set(ci, filter),
                        None => self.tex = Some(ctx.load_texture("canvas", ci, filter)),
                    }
                    self.view.region = out.region;
                    self.view.offset = out.offset;
                    self.view.out_scale = out.scale;
                    self.view.out_size = vec2(size[0] as f32, size[1] as f32);
                }
                Err(e) => self.flash(e.to_string()),
            }
            self.render_key = Some(key);
        }
        let display = full * points_per_out1;
        self.view.img_rect = Rect::from_center_size(canvas.center() + self.pan, display);
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // Space is a temporary hand: hold it to move the canvas, let go to get the tool back.
        // While a text field has the keyboard it is just a space.
        let typing = self.editing.is_some() || ctx.text_edit_focused();
        self.space_down = !typing && ctx.input(|i| i.key_down(Key::Space));
        if self.editing.is_some() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.commit_text();
            }
            return;
        }
        if ctx.text_edit_focused() {
            return;
        }
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        if cmd {
            if pressed(Key::Z) {
                if shift { self.redo() } else { self.undo() }
            } else if pressed(Key::Y) {
                self.redo();
            } else if pressed(Key::C) {
                self.copy();
            } else if pressed(Key::S) {
                if shift { self.save_as() } else { self.save() }
            } else if pressed(Key::O) {
                if let Some(path) = dialogs::pick_image() {
                    self.open_path(ctx, path);
                }
            } else if pressed(Key::W) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if pressed(Key::Num0) {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            } else if pressed(Key::Plus) || pressed(Key::Equals) {
                self.zoom_by(1.25);
            } else if pressed(Key::Minus) {
                self.zoom_by(1.0 / 1.25);
            }
            return;
        }
        if (pressed(Key::Delete) || pressed(Key::Backspace))
            && let Some(i) = self.selected.take().filter(|&i| i < self.doc.items.len())
        {
            self.checkpoint();
            self.doc.items.remove(i);
            self.touch();
        }
        if pressed(Key::Escape) {
            if self.crop.is_some() {
                self.crop = None;
                self.touch();
            } else {
                self.selected = None;
            }
        }
        if pressed(Key::Enter) && self.crop.is_some() {
            self.apply_crop();
        }
        for (tool, _, _, key) in TOOLS {
            if pressed(key) {
                self.select_tool(tool);
            }
        }
        if pressed(Key::C) {
            self.enter_crop();
        }
        if pressed(Key::B) {
            self.toggle_background();
        }
    }

    /// Show or hide the comment panel, starting a band the first time it opens.
    fn toggle_commentary(&mut self) {
        self.show_commentary = !self.show_commentary;
        if self.show_commentary && self.doc.commentary.is_none() {
            self.checkpoint();
            self.doc.commentary = Some(Commentary::default());
            self.touch();
        }
    }

    fn toggle_background(&mut self) {
        // A folded panel opens first; pressing again hides it.
        if self.show_background && self.background_collapsed {
            self.background_collapsed = false;
            State::update(|s| s.background_collapsed = Some(false));
            return;
        }
        self.show_background = !self.show_background;
        if self.show_background && self.doc.background.is_none() {
            self.checkpoint();
            self.doc.background = Some(Background::default());
        }
        self.touch();
    }

    fn enter_crop(&mut self) {
        self.commit_text();
        self.crop = Some(CropState { rect: self.doc.content_rect(), aspect: None, drag: None });
        self.selected = None;
        self.touch();
    }

    fn apply_crop(&mut self) {
        if let Some(c) = self.crop.take() {
            let full = self.doc.full_rect();
            let r = Rect::from_min_max(c.rect.min.round(), c.rect.max.round()).intersect(full);
            if r.width() >= 2.0 && r.height() >= 2.0 {
                self.checkpoint();
                self.doc.crop = (r != full).then_some(r);
            }
            self.touch();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal_centered(|ui| {
            self.file_menu(ui);
            ui.separator();
            if theme::tool_button(ui, icon::CROP, None, self.crop.is_some(), false).on_hover_text("Crop (C)").clicked() {
                if self.crop.is_some() { self.apply_crop() } else { self.enter_crop() }
            }
            if theme::tool_button(ui, icon::IMAGE_SQUARE, None, self.show_background, false).on_hover_text("Background tool (B)").clicked() {
                self.toggle_background();
            }
            ui.separator();
            if theme::tool_button(ui, icon::HAND, None, self.hand() && self.crop.is_none(), false)
                .on_hover_text("Move the canvas — hold Space, or drag with the middle button")
                .clicked()
            {
                self.select_tool(Tool::Pan);
            }
            for (tool, ic, tip, _) in TOOLS {
                if theme::tool_button_icon(ui, ic, None, self.tool == tool && self.crop.is_none(), false).on_hover_text(tip).clicked() {
                    self.select_tool(tool);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(RichText::new(icon::DOTS_THREE).size(18.0), |ui| {
                    if ui.button(format!("{}  Save As…", icon::DOWNLOAD_SIMPLE)).clicked() {
                        self.save_as();
                    }
                    if ui.button(format!("{}  Save Project…", icon::FILE_IMAGE)).clicked() {
                        self.save_project_as();
                    }
                    ui.separator();
                    if ui.button(format!("{}  Pin to Screen", icon::PUSH_PIN)).clicked() {
                        self.pin();
                    }
                    if ui.button(format!("{}  Upload", icon::CLOUD_ARROW_UP)).clicked() {
                        self.upload(&ctx);
                    }
                    if ui.button(format!("{}  Copy Text (OCR)", icon::TEXT_AA)).clicked() {
                        self.copy_text(&ctx);
                    }
                    ui.separator();
                    for (op, ic, label) in [
                        (Orient::RotateCcw, icon::ARROW_COUNTER_CLOCKWISE, "Rotate Left"),
                        (Orient::RotateCw, icon::ARROW_CLOCKWISE, "Rotate Right"),
                        (Orient::FlipH, icon::FLIP_HORIZONTAL, "Flip Horizontal"),
                        (Orient::FlipV, icon::FLIP_VERTICAL, "Flip Vertical"),
                    ] {
                        if ui.button(format!("{ic}  {label}")).clicked() {
                            self.reorient(op);
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}  Paste Image", icon::CLIPBOARD)).clicked() {
                        let r = share::clipboard_image();
                        if let Some(img) = self.report(r) {
                            self.add_image_item(img);
                        }
                    }
                });
                let save = egui::Button::new(RichText::new(format!("{}  Save", icon::FLOPPY_DISK)).color(Color32::WHITE)).fill(theme::ACCENT);
                let save_hint = match &self.save_target {
                    Some(target) => format!("Save to {}", target.display()),
                    None => "Save (Ctrl/Cmd+S)".to_owned(),
                };
                if ui.add(save).on_hover_text(save_hint).clicked() {
                    self.save();
                }
                if ui.button(format!("{}  Copy", icon::COPY)).on_hover_text("Copy (Ctrl/Cmd+C)").clicked() {
                    self.copy();
                }
                ui.add_enabled_ui(!self.redo.is_empty(), |ui| {
                    if theme::tool_button(ui, icon::ARROW_U_UP_RIGHT, None, false, false).on_hover_text("Redo").clicked() {
                        self.redo();
                    }
                });
                ui.add_enabled_ui(!self.undo.is_empty(), |ui| {
                    if theme::tool_button(ui, icon::ARROW_U_UP_LEFT, None, false, false).on_hover_text("Undo").clicked() {
                        self.undo();
                    }
                });
                ui.separator();
                // A button rather than a checkbox: the same switch in half the width.
                if ui
                    .add(egui::Button::selectable(self.show_commentary, "Commentary"))
                    .on_hover_text("Show the comment panel: one box per step, drawn beside the image")
                    .clicked()
                {
                    self.toggle_commentary();
                }
                // The cluster is the last thing added to the row, so it only draws when the tools
                // and the actions above have left room for it.
                if ui.available_width() > 132.0 {
                    ui.separator();
                    self.zoom_control(ui);
                }
            });
        });
    }

    /// Zoom cluster: smaller, a slider you can drag, bigger, and a reset back to the fit.
    fn zoom_control(&mut self, ui: &mut egui::Ui) {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if tiny_button(ui, icon::MINUS, "Zoom out") {
                self.zoom_by(1.0 / 1.25);
            }
            let mut zoom = self.zoom;
            let slider = egui::Slider::new(&mut zoom, MIN_ZOOM..=MAX_ZOOM).logarithmic(true).show_value(false);
            if ui.add_sized([36.0, 16.0], slider).on_hover_text("Zoom").changed() {
                self.zoom_to(zoom, None);
            }
            if tiny_button(ui, icon::PLUS, "Zoom in") {
                self.zoom_by(1.25);
            }
            if tiny_button(ui, icon::ARROWS_CLOCKWISE, "Fit to the window (Ctrl/Cmd+0)") {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
        });
    }

    /// The gradient editor behind the + tile, and behind a double click on one of your own tiles.
    /// It is a window rather than a popup: the colour picker is itself a popup, and a click in it
    /// would count as "outside" and close a popup host.
    fn gradient_window(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.gradient_draft.take() else { return };
        let editing = self.gradient_edit;
        let mut open = true;
        let mut added = None;
        let mut cancelled = false;
        egui::Window::new(if editing.is_some() { "Edit gradient" } else { "Custom gradient" })
            .id(egui::Id::new("gradient-editor"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .default_width(320.0)
            .show(ctx, |ui| {
                let (preview, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
                paint_stops(ui.painter(), preview, draft.angle, |t| swatch_color(draft.color_at(t)));
                ui.add_space(4.0);
                let mut remove = None;
                for i in 0..draft.stops.len() {
                    ui.horizontal(|ui| {
                        let mut colour = swatch_color(draft.stops[i].color);
                        if ui.color_edit_button_srgba(&mut colour).changed() {
                            draft.stops[i].color = colour.to_array();
                        }
                        let mut percent = draft.stops[i].at * 100.0;
                        if ui.add(egui::DragValue::new(&mut percent).range(0.0..=100.0).suffix("%").speed(1.0)).changed() {
                            draft.stops[i].at = percent / 100.0;
                        }
                        if draft.stops.len() > 2 && ui.small_button(icon::X).on_hover_text("Remove this colour").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    draft.stops.remove(i);
                }
                let last = draft.sorted_stops().last().copied().unwrap_or(GradientStop { at: 1.0, color: [255, 255, 255, 255] });
                if ui.small_button(format!("{}  Add colour", icon::PLUS)).clicked() {
                    draft.stops.push(GradientStop { at: 1.0, color: last.color });
                }
                ui.horizontal(|ui| {
                    ui.label("Angle");
                    ui.add(egui::DragValue::new(&mut draft.angle).range(0.0..=360.0).suffix("°").speed(1.0));
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let save = if editing.is_some() { "Update gradient" } else { "Add to gradients" };
                    if ui.button(save).clicked() {
                        added = Some(draft.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            });
        if let Some(gradient) = added {
            let edited = match editing {
                Some(i) if self.settings.gradients.get(i).is_some() => Settings::update(|s| {
                    if let Some(slot) = s.gradients.get_mut(i) {
                        *slot = gradient.clone();
                    }
                })
                .ok(),
                _ => None,
            };
            match edited {
                Some(settings) => {
                    self.settings = settings;
                    self.flash("Gradient updated");
                }
                None => {
                    if let Ok(settings) = Settings::update(|s| s.gradients.push(gradient.clone())) {
                        self.settings = settings;
                    }
                }
            }
            self.set_fill(BgFill::Custom(Box::new(gradient)));
            self.gradient_draft = None;
            self.gradient_edit = None;
        } else if cancelled || !open {
            self.gradient_draft = None;
            self.gradient_edit = None;
        } else {
            self.gradient_draft = Some(draft);
        }
    }

    fn style_controls(&mut self, ui: &mut egui::Ui) {
        // Colour swatch with palette popover.
        let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
        ui.painter().circle_filled(rect.center(), 11.0, self.color);
        ui.painter().circle_stroke(rect.center(), 11.0, Stroke::new(2.0, ui.visuals().widgets.inactive.bg_fill));
        let popup_id = ui.id().with("palette");
        egui::Popup::from_toggle_button_response(&resp).id(popup_id).show(|ui| {
            ui.set_min_width(220.0);
            ui.horizontal_wrapped(|ui| {
                let palette = self.settings.palette.clone();
                for c in palette {
                    let col = swatch_color(c);
                    let (r, sresp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 10.0, col);
                    if col == self.color {
                        ui.painter().circle_stroke(r.center(), 11.5, Stroke::new(2.0, theme::ACCENT));
                    }
                    if sresp.clicked() {
                        self.color = col;
                        self.restyle_selected(|it| it.color = col);
                    }
                }
            });
            ui.separator();
            let mut c = self.color;
            if ui.color_edit_button_srgba(&mut c).changed() {
                self.color = c;
                self.restyle_selected(|it| it.color = c);
            }
            ui.horizontal(|ui| {
                if ui.button(format!("{} Add to palette", icon::PLUS)).clicked() {
                    let c = self.color.to_srgba_unmultiplied();
                    self.update_settings(|s| s.palette.push(c));
                }
                if ui.button(format!("{} Pick from image", icon::EYEDROPPER)).clicked() {
                    self.picking_color = true;
                }
            });
        });
        ui.add_space(4.0);
        let mut w = self.width;
        if ui.add(egui::Slider::new(&mut w, 1.0..=30.0).show_value(false)).on_hover_text("Stroke width").changed() {
            self.width = w;
            if let Some(i) = self.selected.filter(|&i| i < self.doc.items.len()) {
                // One undo step per slider drag, not per frame.
                if !ui.ctx().input(|inp| inp.pointer.any_down()) || self.undo.is_empty() {
                    self.checkpoint();
                }
                self.doc.items[i].width = w;
                self.touch();
            }
        }
        let selected_shape = self.selected.and_then(|i| self.doc.items.get(i)).map(|it| it.shape.clone());
        let tool = match &selected_shape {
            Some(Shape::Arrow { .. }) => Tool::Arrow,
            Some(Shape::Text { .. }) => Tool::Text,
            Some(Shape::Counter { .. }) => Tool::Counter,
            Some(Shape::Redact { .. }) => Tool::Redact,
            Some(Shape::Spotlight { .. }) => Tool::Spotlight,
            _ => self.tool,
        };
        match tool {
            Tool::Arrow => {
                let styles =
                    [(ArrowStyle::Tapered, "Tapered"), (ArrowStyle::Classic, "Classic"), (ArrowStyle::Double, "Double"), (ArrowStyle::Curved, "Curved")];
                let current = styles.iter().find(|s| s.0 == self.arrow_style).map_or("Tapered", |s| s.1);
                egui::ComboBox::from_id_salt("arrow-style").selected_text(current).width(90.0).show_ui(ui, |ui| {
                    for (s, l) in styles {
                        if ui.selectable_label(self.arrow_style == s, l).clicked() {
                            self.arrow_style = s;
                            self.restyle_selected(|it| {
                                if let Shape::Arrow { style, bend, .. } = &mut it.shape {
                                    *style = s;
                                    if s == ArrowStyle::Curved && *bend == 0.0 {
                                        *bend = 0.25;
                                    }
                                }
                            });
                        }
                    }
                });
            }
            Tool::Text => {
                egui::ComboBox::from_id_salt("text-style").selected_text(format!("{:?}", self.text_style)).width(90.0).show_ui(ui, |ui| {
                    for s in TextStyle::ALL {
                        if ui.selectable_label(self.text_style == s, format!("{s:?}")).clicked() {
                            self.text_style = s;
                            self.restyle_selected(|it| {
                                if let Shape::Text { style, .. } = &mut it.shape {
                                    *style = s;
                                }
                            });
                        }
                    }
                });
                let mut size = self.text_size;
                if ui.add(egui::DragValue::new(&mut size).range(8.0..=400.0).suffix(" pt")).changed() {
                    self.text_size = size;
                    self.restyle_selected(|it| {
                        if let Shape::Text { size: s, .. } = &mut it.shape {
                            *s = size;
                        }
                    });
                }
                // Single line never wraps; a paragraph wraps at the right edge of the image.
                let layout = if self.text_paragraph { "Paragraph" } else { "Single line" };
                egui::ComboBox::from_id_salt("text-layout").selected_text(layout).width(110.0).show_ui(ui, |ui| {
                    for paragraph in [false, true] {
                        let label = if paragraph { "Paragraph" } else { "Single line" };
                        if ui.selectable_label(self.text_paragraph == paragraph, label).clicked() {
                            self.text_paragraph = paragraph;
                            let base = self.doc.base.width() as f32;
                            let size = self.text_size;
                            self.restyle_selected(|it| {
                                if let Shape::Text { pos, wrap, .. } = &mut it.shape {
                                    *wrap = paragraph.then(|| (base - pos.x).max(size * 4.0));
                                }
                            });
                        }
                    }
                });
            }
            Tool::Counter => {
                // A step already on the image is what the row edits, so it can be restyled later.
                if let Some(style) = self.selected.and_then(|i| self.doc.items.get(i)).and_then(|it| match &it.shape {
                    Shape::Counter { style, .. } => Some(style.clone()),
                    _ => None,
                }) {
                    self.counter_style = style;
                }
                let mut style = self.counter_style.clone();
                let mut changed = false;
                let label = match style.label {
                    CounterLabel::Numbers => "123",
                    CounterLabel::Letters => "ABC",
                };
                egui::ComboBox::from_id_salt("counter-label").selected_text(label).width(64.0).show_ui(ui, |ui| {
                    for kind in [CounterLabel::Numbers, CounterLabel::Letters] {
                        let text = match kind {
                            CounterLabel::Numbers => "123",
                            CounterLabel::Letters => "ABC",
                        };
                        if ui.selectable_label(style.label == kind, text).clicked() {
                            style.label = kind;
                            changed = true;
                        }
                    }
                });
                changed |= ui.add(egui::DragValue::new(&mut style.size).range(14.0..=160.0).suffix(" px").speed(0.5)).on_hover_text("Diameter").changed();
                changed |= ui
                    .add_sized([64.0, 20.0], egui::Slider::new(&mut style.padding, 0.0..=0.5).show_value(false))
                    .on_hover_text("Padding around the label")
                    .changed();
                changed |= ui.add(egui::Checkbox::without_text(&mut style.shadow_on)).on_hover_text("Drop shadow").changed();
                ui.label("Shadow");
                if style.shadow_on {
                    changed |= ui
                        .add_sized([64.0, 20.0], egui::Slider::new(&mut style.shadow, 0.05..=1.0).show_value(false))
                        .on_hover_text("Shadow strength")
                        .changed();
                    changed |= ui
                        .add(egui::DragValue::new(&mut style.shadow_blur).range(0.0..=40.0).suffix(" px").speed(0.2))
                        .on_hover_text("Shadow blur: how far its soft edge reaches")
                        .changed();
                    changed |= ui
                        .add(egui::DragValue::new(&mut style.shadow_distance).range(0.0..=40.0).suffix(" off").speed(0.2))
                        .on_hover_text("Shadow offset from the circle")
                        .changed();
                    changed |= direction_dial(ui, &mut style.shadow_angle);
                }
                let mut ring = style.outline.unwrap_or(self.color);
                if ui.color_edit_button_srgba(&mut ring).on_hover_text("Ring colour").changed() {
                    style.outline = Some(ring);
                    changed = true;
                }
                if ui.selectable_label(style.outline.is_some(), "Ring").on_hover_text("Turn the ring off").clicked() {
                    style.outline = style.outline.is_none().then_some(ring);
                    changed = true;
                }
                if style.outline.is_some() {
                    changed |= ui
                        .add(egui::DragValue::new(&mut style.outline_width).range(0.0..=20.0).suffix(" px").speed(0.2))
                        .on_hover_text("Ring thickness")
                        .changed();
                }
                if changed {
                    self.counter_style = style.clone();
                    self.restyle_selected(|it| {
                        if let Shape::Counter { style: s, .. } = &mut it.shape {
                            *s = style.clone();
                        }
                    });
                }
            }
            Tool::Redact => {
                let modes = [
                    (RedactMode::Pixelate, "Pixelate"),
                    (RedactMode::SecureBlur, "Secure blur"),
                    (RedactMode::Blur, "Smooth blur"),
                    (RedactMode::Solid, "Solid"),
                ];
                let current = modes.iter().find(|m| m.0 == self.redact_mode).map_or("Pixelate", |m| m.1);
                egui::ComboBox::from_id_salt("redact-mode").selected_text(current).width(100.0).show_ui(ui, |ui| {
                    for (m, l) in modes {
                        if ui.selectable_label(self.redact_mode == m, l).clicked() {
                            self.redact_mode = m;
                            self.restyle_selected(|it| {
                                if let Shape::Redact { mode, .. } = &mut it.shape {
                                    *mode = m;
                                }
                            });
                        }
                    }
                });
            }
            Tool::Spotlight => {
                let mut e = self.spotlight_ellipse;
                if ui.toggle_value(&mut e, "Ellipse").changed() {
                    self.spotlight_ellipse = e;
                    self.restyle_selected(|it| {
                        if let Shape::Spotlight { ellipse, .. } = &mut it.shape {
                            *ellipse = e;
                        }
                    });
                }
            }
            _ => {}
        }
    }

    fn background_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        // Brand presets: loading one brings back the background and the counter look.
        let names: Vec<String> = self.settings.presets.iter().map(|p| p.name.clone()).collect();
        let active = self.preset_active.clone().filter(|name| names.iter().any(|n| n == name));
        // Anything the user changed since the preset was loaded can be written back to it.
        let dirty = active.as_ref().is_some_and(|name| {
            self.settings.presets.iter().find(|p| &p.name == name).is_some_and(|p| {
                self.doc.background.as_ref() != Some(&p.background)
                    || p.counter.as_ref() != Some(&self.counter_style)
                    || p.commentary != self.commentary_style()
            })
        });
        ui.horizontal(|ui| {
            let shown = match &active {
                Some(name) if dirty => format!("{name} *"),
                Some(name) => name.clone(),
                None => "Brand preset".to_owned(),
            };
            egui::ComboBox::from_id_salt("preset").selected_text(shown).width((ui.available_width() - 88.0).max(90.0)).show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    if ui.selectable_label(false, name).clicked() {
                        let bg = self.settings.presets[i].background.clone();
                        let counter = self.settings.presets[i].counter.clone();
                        let commentary = self.settings.presets[i].commentary.clone();
                        self.checkpoint();
                        self.doc.background = Some(bg);
                        if let Some(counter) = counter {
                            self.counter_style = counter;
                        }
                        if let Some(style) = commentary {
                            // The preset carries the band's look; the comments already written stay.
                            let entries = self.doc.commentary.as_ref().map(|c| c.entries.clone()).unwrap_or_default();
                            self.doc.commentary = Some(Commentary { entries, ..style });
                            self.show_commentary = true;
                        }
                        self.preset_active = Some(name.clone());
                        self.touch();
                    }
                }
                if names.is_empty() {
                    ui.label(RichText::new("No presets yet").weak());
                }
            });
            if ui
                .add_enabled(dirty, egui::Button::new(icon::FLOPPY_DISK))
                .on_hover_text("Save these changes into the preset")
                .on_disabled_hover_text("Load a preset first, then change something")
                .clicked()
                && let Some(name) = active.clone()
                && let Some(background) = self.doc.background.clone()
            {
                let counter = self.counter_style.clone();
                let commentary = self.commentary_style();
                let saved = self.update_settings(|s| {
                    if let Some(p) = s.presets.iter_mut().find(|p| p.name == name) {
                        p.background = background;
                        p.counter = Some(counter);
                        p.commentary = commentary;
                    }
                });
                if saved {
                    self.flash("Preset updated");
                }
            }
            ui.menu_button(icon::TRASH, |ui| {
                let mut remove = None;
                for p in &self.settings.presets {
                    if ui.button(&p.name).clicked() {
                        remove = Some(p.name.clone());
                    }
                }
                if let Some(name) = remove {
                    if self.preset_active.as_deref() == Some(name.as_str()) {
                        self.preset_active = None;
                    }
                    self.update_settings(|s| s.presets.retain(|p| p.name != name));
                }
            })
            .response
            .on_hover_text("Delete a preset");
        });
        ui.horizontal(|ui| {
            let name_width = (ui.available_width() - 42.0).max(90.0);
            ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("Preset name").desired_width(name_width));
            let can = self.doc.background.is_some() && !self.preset_name.trim().is_empty();
            if ui.add_enabled(can, egui::Button::new(icon::PLUS)).on_hover_text("Save current background as a preset").clicked()
                && let Some(background) = self.doc.background.clone()
            {
                let name = self.preset_name.trim().to_owned();
                let counter = self.counter_style.clone();
                let commentary = self.commentary_style();
                let saved = self.update_settings(|s| {
                    s.presets.retain(|p| p.name != name);
                    s.presets.push(Preset { name: name.clone(), background, counter: Some(counter), commentary });
                });
                if saved {
                    self.preset_name.clear();
                    self.preset_active = Some(name);
                    self.flash("Preset saved");
                }
            }
        });
        if ui.add_sized([ui.available_width(), 28.0], egui::Button::selectable(self.doc.background.is_none(), "No background")).clicked() {
            self.checkpoint();
            self.doc.background = None;
            self.touch();
        }
        let fill = self.doc.background.as_ref().map(|b| b.fill.clone());
        section(ui, "Gradients");
        let mut remove_gradient = None;
        let mut edit_gradient = None;
        let mut chosen: Option<BgFill> = None;
        egui::Grid::new("gradients").spacing(vec2(6.0, 6.0)).show(ui, |ui| {
            let mut tiles = 0;
            for (i, g) in GRADIENTS.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                paint_stops(ui.painter(), r, g.angle, |t| sampled(g.stops, t));
                if fill == Some(BgFill::Gradient(i)) {
                    ui.painter().rect_stroke(r.expand(2.0), 10, Stroke::new(2.5, theme::ACCENT), StrokeKind::Outside);
                }
                if resp.on_hover_text(g.name).clicked() {
                    chosen = Some(BgFill::Gradient(i));
                }
                tiles += 1;
                if tiles % 4 == 0 {
                    ui.end_row();
                }
            }
            // The user's own gradients follow the built-in ones, each with an x to delete it.
            for (i, g) in self.settings.gradients.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                paint_stops(ui.painter(), r, g.angle, |t| swatch_color(g.color_at(t)));
                if fill == Some(BgFill::Custom(Box::new(g.clone()))) {
                    ui.painter().rect_stroke(r.expand(2.0), 10, Stroke::new(2.5, theme::ACCENT), StrokeKind::Outside);
                }
                let badge = Rect::from_center_size(r.right_top() + vec2(-2.0, 2.0), Vec2::splat(16.0));
                let del = ui.interact(badge, ui.id().with(("delete-gradient", i)), Sense::click());
                ui.painter().circle_filled(badge.center(), 8.0, Color32::from_black_alpha(170));
                ui.painter().text(badge.center(), Align2::CENTER_CENTER, icon::X, FontId::proportional(10.0), Color32::WHITE);
                let resp = resp.on_hover_text("Your gradient — double-click to edit it");
                if del.on_hover_text("Delete this gradient").clicked() {
                    remove_gradient = Some(i);
                } else if resp.double_clicked() {
                    // Open the same window the + tile opens, saving back over this tile.
                    edit_gradient = Some((i, g.clone()));
                } else if resp.clicked() {
                    chosen = Some(BgFill::Custom(Box::new(g.clone())));
                }
                tiles += 1;
                if tiles % 4 == 0 {
                    ui.end_row();
                }
            }
            // Build a gradient of your own: the same size as the samples beside it.
            let (r, plus) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
            ui.painter().rect_stroke(r, 10, Stroke::new(1.5, ui.visuals().widgets.inactive.fg_stroke.color), StrokeKind::Inside);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, icon::PLUS, FontId::proportional(20.0), ui.visuals().widgets.inactive.fg_stroke.color);
            if plus.on_hover_text("Build a gradient").clicked() {
                self.gradient_draft = Some(OwnedGradient {
                    angle: 135.0,
                    stops: vec![GradientStop { at: 0.0, color: [255, 95, 109, 255] }, GradientStop { at: 1.0, color: [255, 195, 113, 255] }],
                });
            }
            tiles += 1;
            if tiles % 4 == 0 {
                ui.end_row();
            }
        });
        if let Some(i) = remove_gradient {
            let had_it =
                self.doc.background.as_ref().is_some_and(|b| matches!(&b.fill, BgFill::Custom(g) if Some(g.as_ref()) == self.settings.gradients.get(i)));
            if let Ok(settings) = Settings::update(|s| {
                if i < s.gradients.len() {
                    s.gradients.remove(i);
                }
            }) {
                self.settings = settings;
            }
            if had_it {
                self.set_fill(BgFill::Gradient(3));
            }
            self.flash("Gradient removed");
        }
        if let Some((i, gradient)) = edit_gradient {
            self.gradient_edit = Some(i);
            self.gradient_draft = Some(gradient);
        }
        if let Some(fill) = chosen {
            self.set_fill(fill);
        }
        section(ui, "Solid Colors");
        let palette = self.settings.palette.clone();
        let mut remove_colour = None;
        let mut edit_colour = None;
        let mut open_custom = false;
        ui.horizontal_wrapped(|ui| {
            for (i, c) in palette.iter().enumerate() {
                let col = swatch_color(*c);
                let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                ui.painter().circle_filled(r.center(), 11.0, col);
                ui.painter().circle_stroke(r.center(), 11.0, ui.visuals().widgets.noninteractive.bg_stroke);
                if fill == Some(BgFill::Solid(col)) {
                    ui.painter().circle_stroke(r.center(), 13.0, Stroke::new(2.0, theme::ACCENT));
                }
                // A little x in the corner, like the gradient tiles.
                let badge = Rect::from_center_size(r.right_top() + vec2(-2.0, 2.0), Vec2::splat(13.0));
                let del = ui.interact(badge, ui.id().with(("delete-colour", i)), Sense::click());
                ui.painter().circle_filled(badge.center(), 6.5, Color32::from_black_alpha(170));
                ui.painter().text(badge.center(), Align2::CENTER_CENTER, icon::X, FontId::proportional(8.0), Color32::WHITE);
                let resp = resp.on_hover_text("Double-click to edit, or use the x to remove it");
                if del.on_hover_text("Remove this colour").clicked() {
                    remove_colour = Some(i);
                } else if resp.double_clicked() {
                    edit_colour = Some((i, col));
                } else if resp.clicked() {
                    self.set_fill(BgFill::Solid(col));
                }
            }
            // Any colour of your own: a circle with a plus, as tall as the swatches beside it.
            let (r, plus) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
            let ring = ui.visuals().widgets.inactive.fg_stroke.color;
            ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(1.5, ring));
            ui.painter().text(r.center(), Align2::CENTER_CENTER, icon::PLUS, FontId::proportional(12.0), ring);
            if plus.on_hover_text("Pick a colour of your own").clicked() {
                open_custom = true;
            }
        });
        if let Some(i) = remove_colour {
            let removed = palette.get(i).copied();
            self.update_settings(|s| {
                if i < s.palette.len() {
                    s.palette.remove(i);
                }
            });
            if let Some([r, g, b, a]) = removed
                && fill == Some(BgFill::Solid(Color32::from_rgba_unmultiplied(r, g, b, a)))
            {
                self.set_fill(BgFill::Transparent);
            }
        }
        if let Some((i, colour)) = edit_colour {
            self.colour_edit = Some(i);
            self.colour_draft = Some(colour);
        }
        if open_custom {
            self.colour_edit = None;
            self.colour_draft = Some(match &fill {
                Some(BgFill::Solid(c)) => *c,
                _ => Color32::WHITE,
            });
        }
        section(ui, "Effects");
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(fill == Some(BgFill::Blurred), format!("{} Blurred", icon::DROP)).clicked() {
                self.set_fill(BgFill::Blurred);
            }
            if ui.selectable_label(fill == Some(BgFill::Transparent), format!("{} Clear", icon::CHECKERBOARD)).clicked() {
                self.set_fill(BgFill::Transparent);
            }
            if ui.button(format!("{} Image…", icon::IMAGE)).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp"]).pick_file()
            {
                let r = image::open(&path).map_err(anyhow::Error::from);
                if let Some(img) = self.report(r) {
                    self.set_fill(BgFill::Image(Arc::new(img.into_rgba8())));
                }
            }
        });
        self.watermark_section(ui);
        let Some(bg) = self.doc.background.clone() else { return };
        section(ui, "Layout");
        let mut b = bg.clone();
        let mut changed = false;
        egui::Grid::new("bg-settings").num_columns(2).spacing(vec2(8.0, 8.0)).show(ui, |ui| {
            ui.label("Padding");
            changed |= ui.add(egui::Slider::new(&mut b.padding, 0.0..=400.0).show_value(false)).changed();
            ui.end_row();
            ui.label("Corners");
            changed |= ui.add(egui::Slider::new(&mut b.radius, 0.0..=80.0).show_value(false)).changed();
            ui.end_row();
            ui.label("Shadow");
            changed |= ui.add(egui::Slider::new(&mut b.shadow, 0.0..=1.0).show_value(false)).changed();
            ui.end_row();
            ui.label("Ratio");
            egui::ComboBox::from_id_salt("aspect").selected_text(b.aspect.label()).width(120.0).show_ui(ui, |ui| {
                for (a, l) in Aspect::PRESETS {
                    changed |= ui.selectable_value(&mut b.aspect, a, l).changed();
                }
            });
            ui.end_row();
            ui.label("Align");
            egui::Grid::new("align").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
                for row in [egui::Align::Min, egui::Align::Center, egui::Align::Max] {
                    for col in [egui::Align::Min, egui::Align::Center, egui::Align::Max] {
                        let a = egui::Align2([col, row]);
                        let on = b.align == a;
                        let glyph = RichText::new(if on { icon::RADIO_BUTTON } else { icon::CIRCLE }).color(if on {
                            theme::ACCENT
                        } else {
                            ui.visuals().weak_text_color()
                        });
                        if ui.add(egui::Button::new(glyph).small().frame(false)).clicked() {
                            b.align = a;
                            changed = true;
                        }
                    }
                    ui.end_row();
                }
            });
            ui.end_row();
        });
        changed |= ui.checkbox(&mut b.auto_balance, "Auto-balance").on_hover_text("Even out uneven margins around the content").changed();
        if changed {
            if !ui.ctx().input(|i| i.pointer.any_down()) || self.undo.last().is_none_or(|s| s.background.as_ref() != Some(&bg)) {
                self.checkpoint();
            }
            self.doc.background = Some(b);
            self.touch();
        }
    }

    /// The watermark's box in background pixels, if a click at `canvas` lands on it.
    fn watermark_hit(&self, canvas: Pos2) -> Option<Rect> {
        let bg = self.doc.background.as_ref()?;
        let watermark = bg.watermark.as_ref()?;
        let (size, _) = render::background_layout(self.base_size(), bg, 1.0);
        let k = size.x / self.view.out_size.x.max(1.0);
        let rect = watermark.rect(size, bg.padding);
        rect.contains(pos2(canvas.x * k, canvas.y * k)).then_some(rect)
    }

    /// Canvas pixels per background pixel: a drag measured on screen has to move the stored offset
    /// by this much.
    fn watermark_scale(&self) -> Option<f32> {
        let bg = self.doc.background.as_ref()?;
        bg.watermark.as_ref()?;
        let (size, _) = render::background_layout(self.base_size(), bg, 1.0);
        Some(size.x / self.view.out_size.x.max(1.0))
    }

    fn base_size(&self) -> Vec2 {
        let size = self.doc.full_rect().size();
        vec2(size.x.max(1.0), size.y.max(1.0))
    }

    /// Watermark section of the background panel: add, place, size and remove the mark.
    fn watermark_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("Watermark").strong());
        let Some(bg) = self.doc.background.clone() else {
            ui.label(RichText::new("Turn the background on to add a watermark.").color(ui.visuals().weak_text_color()));
            return;
        };
        let Some(mut mark) = bg.watermark.clone() else {
            if ui.button(format!("{} Add image…", icon::IMAGE)).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"]).pick_file()
            {
                match image::open(&path) {
                    Ok(img) => {
                        self.checkpoint();
                        let mut bg = bg.clone();
                        bg.watermark = Some(Watermark::new(img.into_rgba8()));
                        self.doc.background = Some(bg);
                        self.touch();
                    }
                    Err(err) => self.flash(format!("Could not open the watermark: {err}")),
                }
            }
            return;
        };
        let mut changed = false;
        let mut removed = false;
        egui::Grid::new("wm-anchor").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            for row in 0..3 {
                for col in 0..3 {
                    let anchor = Anchor::ALL[row * 3 + col];
                    let chosen = mark.anchor == anchor;
                    let glyph = RichText::new(if chosen { icon::RADIO_BUTTON } else { icon::CIRCLE }).color(if chosen {
                        theme::ACCENT
                    } else {
                        ui.visuals().weak_text_color()
                    });
                    if ui.add(egui::Button::new(glyph).small().frame(false)).on_hover_text(anchor.label()).clicked() {
                        mark.anchor = anchor;
                        // A new spot starts from the padding, without the old drag.
                        mark.offset = Vec2::ZERO;
                        changed = true;
                    }
                }
                ui.end_row();
            }
        });
        changed |= ui.add(egui::Slider::new(&mut mark.width, 0.04..=0.6).text("Size")).changed();
        changed |= ui.add(egui::Slider::new(&mut mark.opacity, 0.05..=1.0).text("Opacity")).changed();
        ui.horizontal(|ui| {
            if ui.button(format!("{} Remove", icon::TRASH)).clicked() {
                removed = true;
            }
            ui.label(RichText::new("Drag it on the image to move it").color(ui.visuals().weak_text_color()));
        });
        if removed {
            self.checkpoint();
            let mut bg = bg.clone();
            bg.watermark = None;
            self.doc.background = Some(bg);
            self.touch();
        } else if changed {
            if !ui.ctx().input(|i| i.pointer.any_down()) || self.undo.last().is_none_or(|s| s.background.as_ref() != Some(&bg)) {
                self.checkpoint();
            }
            let mut bg = bg.clone();
            bg.watermark = Some(mark);
            self.doc.background = Some(bg);
            self.touch();
        }
    }

    /// The colour picker behind the Solid Colors + circle and a double click on a swatch. A window
    /// for the same reason as the gradient editor: a nested colour picker closes a popup host.
    fn colour_window(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.colour_draft else { return };
        let editing = self.colour_edit;
        let mut open = true;
        let mut action = None;
        egui::Window::new(if editing.is_some() { "Edit colour" } else { "New colour" })
            .id(egui::Id::new("colour-editor"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .default_width(240.0)
            .show(ctx, |ui| {
                let (preview, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                ui.painter().rect_filled(preview, 6, draft);
                ui.add_space(4.0);
                ui.color_edit_button_srgba(&mut draft);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if editing.is_some() {
                        if ui.button("Save").clicked() {
                            action = Some(ColourAction::Save(draft));
                        }
                        if ui.button(format!("{} Delete", icon::TRASH)).clicked() {
                            action = Some(ColourAction::Delete);
                        }
                    } else {
                        if ui.button("Use").clicked() {
                            action = Some(ColourAction::Use(draft));
                        }
                        if ui.button("Add to Colors").clicked() {
                            action = Some(ColourAction::Add(draft));
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(ColourAction::Cancel);
                    }
                });
            });
        self.colour_draft = if open && action.is_none() { Some(draft) } else { None };
        match action {
            Some(ColourAction::Use(colour)) => {
                self.colour_edit = None;
                self.set_fill(BgFill::Solid(colour));
            }
            Some(ColourAction::Add(colour)) => {
                let rgba = colour.to_array();
                self.update_settings(|s| {
                    if !s.palette.contains(&rgba) {
                        s.palette.push(rgba);
                    }
                });
                self.colour_edit = None;
                self.set_fill(BgFill::Solid(colour));
            }
            Some(ColourAction::Save(colour)) => {
                if let Some(i) = editing {
                    let rgba = colour.to_array();
                    self.update_settings(|s| {
                        if let Some(slot) = s.palette.get_mut(i) {
                            *slot = rgba;
                        }
                    });
                }
                self.colour_edit = None;
                self.set_fill(BgFill::Solid(colour));
            }
            Some(ColourAction::Delete) => {
                if let Some(i) = editing {
                    self.update_settings(|s| {
                        if i < s.palette.len() {
                            s.palette.remove(i);
                        }
                    });
                }
                self.colour_edit = None;
            }
            Some(ColourAction::Cancel) | None => {
                self.colour_edit = None;
            }
        }
    }

    /// The comment band's look, ready to store in a preset: its settings without the comments.
    fn commentary_style(&self) -> Option<Commentary> {
        self.doc.commentary.clone().map(|mut c| {
            c.entries.clear();
            c
        })
    }

    /// The comment panel on the right: formatting, the header, the band's look, and one box per step.
    fn commentary_panel(&mut self, ui: &mut egui::Ui) {
        let counters: Vec<(u32, String)> = self
            .doc
            .items
            .iter()
            .filter_map(|item| match &item.shape {
                Shape::Counter { n, style, .. } => Some((*n, style.label_for(*n))),
                _ => None,
            })
            .collect();
        let Some(mut c) = self.doc.commentary.clone() else {
            ui.label(RichText::new("No comments yet.").color(ui.visuals().weak_text_color()));
            return;
        };
        let mut changed = false;
        let mut picked = None;

        // B / I / bullets, for the whole band.
        ui.horizontal(|ui| {
            if ui.selectable_label(c.bold, RichText::new("B").strong()).on_hover_text("Bold").clicked() {
                c.bold = !c.bold;
                changed = true;
            }
            if ui.selectable_label(c.italic, RichText::new("I").italics()).on_hover_text("Italic").clicked() {
                c.italic = !c.italic;
                changed = true;
            }
            if ui.selectable_label(c.bullet, "•").on_hover_text("Start every typed line with a bullet").clicked() {
                c.bullet = !c.bullet;
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            let mut header = c.header.is_some();
            if ui.checkbox(&mut header, "Header").on_hover_text("A title over the comments").changed() {
                c.header = header.then(shotcraft_engine::doc::model::commentary_header);
                changed = true;
            }
            if let Some(text) = &mut c.header {
                let width = (ui.available_width() - 4.0).max(80.0);
                changed |= ui.add(egui::TextEdit::singleline(text).hint_text("Comments").desired_width(width)).changed();
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Side");
            egui::ComboBox::from_id_salt("commentary-side").selected_text(c.side.label()).width(74.0).show_ui(ui, |ui| {
                for side in CommentarySide::ALL {
                    if ui.selectable_label(c.side == side, side.label()).clicked() {
                        c.side = side;
                        changed = true;
                    }
                }
            });
            if !c.side.horizontal() {
                ui.label("Width");
                changed |= ui
                    .add(egui::DragValue::new(&mut c.width).range(0.0..=4000.0).suffix(" px").speed(4.0))
                    .on_hover_text("How much room the comments get; 0 fits them to the image's height")
                    .changed();
            }
        });
        ui.horizontal(|ui| {
            changed |= ui.color_edit_button_srgba(&mut c.background).on_hover_text("Band colour").changed();
            ui.label("Text");
            changed |= ui.add(egui::DragValue::new(&mut c.size).range(8.0..=64.0).suffix(" pt")).changed();
        });
        ui.horizontal(|ui| {
            ui.label("Space");
            changed |= ui
                .add(egui::DragValue::new(&mut c.spacing).range(0.0..=120.0).suffix(" px"))
                .on_hover_text("Gap between the header and each comment")
                .changed();
            ui.label("Padding");
            changed |= ui.add(egui::DragValue::new(&mut c.padding).range(0.0..=120.0).suffix(" px")).changed();
        });
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut c.match_counter, "Match step").on_hover_text("Ring each step in its counter's colour").changed();
            if !c.match_counter {
                changed |= ui.color_edit_button_srgba(&mut c.badge).on_hover_text("Badge colour").changed();
            }
        });
        ui.separator();

        if counters.is_empty() {
            ui.label(RichText::new("Place a numbered step to describe it here").color(ui.visuals().weak_text_color()));
        }
        // One box per step: as tall as its text, up to ten lines, then it scrolls.
        let ten_lines = ui.text_style_height(&egui::TextStyle::Body) * 10.0 + 10.0;
        for (n, label) in &counters {
            ui.horizontal(|ui| {
                let badge = egui::Button::new(RichText::new(label).strong()).min_size(vec2(26.0, 0.0));
                if ui.add(badge).on_hover_text("Click to adjust this step").clicked() {
                    picked = Some(*n);
                }
                ui.label(RichText::new(format!("Step {label}")).weak());
            });
            egui::ScrollArea::vertical().id_salt(("comment-scroll", *n)).max_height(ten_lines).auto_shrink([false, true]).show(ui, |ui| {
                let width = ui.available_width();
                if ui.add(egui::TextEdit::multiline(c.entry_mut(*n)).hint_text("comment").desired_width(width).desired_rows(1)).changed() {
                    changed = true;
                }
            });
            ui.add_space(2.0);
        }
        if changed {
            self.doc.commentary = Some(c);
            self.touch();
        }
        if let Some(n) = picked {
            self.select_step(n);
        }
    }

    /// Select the numbered step `n` on the image, so the counter row edits its look.
    fn select_step(&mut self, n: u32) {
        if let Some(i) = self.doc.items.iter().position(|it| matches!(&it.shape, Shape::Counter { n: c, .. } if *c == n)) {
            self.selected = Some(i);
            self.tool = Tool::Counter;
        }
    }

    fn set_fill(&mut self, fill: BgFill) {
        self.checkpoint();
        let mut bg = self.doc.background.clone().unwrap_or_default();
        bg.fill = fill;
        self.doc.background = Some(bg);
        self.touch();
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let canvas = ui.max_rect();
        let checker = ui.visuals().extreme_bg_color;
        ui.painter().rect_filled(canvas, 0, checker);
        self.render_if_needed(&ctx, canvas);
        let resp = ui.allocate_rect(canvas, Sense::click_and_drag());
        let view = self.view;
        if let Some(tex) = &self.tex {
            let shadow = egui::epaint::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(50) };
            ui.painter().add(shadow.as_shape(view.img_rect, CornerRadius::ZERO));
            ui.painter().image(tex.id(), view.img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            // Keeps dark screenshots distinguishable from the dark canvas.
            ui.painter().rect_stroke(view.img_rect, 0, ui.visuals().widgets.noninteractive.bg_stroke, StrokeKind::Outside);
        }

        // The wheel zooms smoothly around the pointer; Shift + wheel moves the canvas instead.
        if resp.hovered() {
            let (zoom_delta, scroll, shift) = ctx.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers.shift));
            let pointer = resp.hover_pos();
            if zoom_delta != 1.0 {
                self.zoom_to(self.zoom * zoom_delta, pointer);
            } else if scroll != Vec2::ZERO {
                if shift {
                    self.pan += scroll;
                } else {
                    // A wheel notch is about 50 points, which is a little under 8 %.
                    self.zoom_to(self.zoom * (1.0 + scroll.y * 0.0015).clamp(0.8, 1.25), pointer);
                }
            }
        }
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            let r = image::open(f.path()).map_err(anyhow::Error::from);
            if let Some(img) = self.report(r) {
                self.add_image_item(img.into_rgba8());
            }
        }

        if self.crop.is_some() {
            self.crop_interaction(ui, &resp);
            return;
        }
        let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let tol = 6.0 / view.ppb();
        let shift = ctx.input(|i| i.modifiers.shift);
        let middle = ctx.input(|i| i.pointer.middle_down());

        if self.picking_color {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            if resp.clicked()
                && let Some(p) = pointer
            {
                let b = view.to_base(p);
                if b.x >= 0.0 && b.y >= 0.0 && (b.x as u32) < self.doc.base.width() && (b.y as u32) < self.doc.base.height() {
                    let [r, g, bb, _] = self.doc.base.get_pixel(b.x as u32, b.y as u32).0;
                    self.color = Color32::from_rgb(r, g, bb);
                    self.restyle_selected(|it| it.color = Color32::from_rgb(r, g, bb));
                }
                self.picking_color = false;
            }
            return;
        }

        if resp.drag_started() {
            self.commit_text();
            let start = ctx.input(|i| i.pointer.press_origin()).unwrap_or_default();
            let b = view.to_base(start);
            let canvas_point = view.to_canvas(start);
            let on_watermark = self.watermark_hit(canvas_point).is_some();
            let handle = self.selected.and_then(|i| {
                let it = self.doc.items.get(i)?;
                it.handles().iter().position(|h| h.distance(b) <= tol * 1.5).map(|h| (i, h))
            });
            self.drag = if middle || self.hand() {
                Some(EditDrag::Pan { last: start })
            } else if on_watermark {
                // The watermark sits on top of the annotations, so it gets the drag first.
                self.checkpoint();
                Some(EditDrag::Watermark { last: canvas_point })
            } else if let Some((index, handle)) = handle {
                self.checkpoint();
                Some(EditDrag::Handle { index, handle })
            } else if self.tool == Tool::Select {
                let hit = self.doc.items.iter().rposition(|it| it.hit(b, tol));
                self.selected = hit;
                hit.map(|index| {
                    self.checkpoint();
                    EditDrag::Move { index, last: b }
                })
            } else if let Some(shape) = self.new_shape(b) {
                self.checkpoint();
                let mut item = self.new_item(shape);
                if self.tool == Tool::Redact && self.redact_mode == RedactMode::Solid {
                    item.color = Color32::BLACK;
                }
                if self.tool == Tool::Highlighter && self.color == Color32::from_rgb(255, 59, 48) {
                    item.color = Color32::from_rgb(255, 214, 10);
                }
                self.doc.items.push(item);
                self.selected = None;
                Some(EditDrag::Create { index: self.doc.items.len() - 1, start: b })
            } else {
                None
            };
        }

        if let (Some(drag), Some(p)) = (self.drag, resp.interact_pointer_pos()) {
            let b = view.to_base(p);
            match drag {
                EditDrag::Pan { last } => {
                    self.pan += p - last;
                    self.drag = Some(EditDrag::Pan { last: p });
                }
                EditDrag::Move { index, last } => {
                    if let Some(it) = self.doc.items.get_mut(index) {
                        it.translate(b - last);
                        self.drag = Some(EditDrag::Move { index, last: b });
                        self.touch();
                    }
                }
                EditDrag::Watermark { last } => {
                    let c = view.to_canvas(p);
                    if let Some(k) = self.watermark_scale()
                        && let Some(watermark) = self.doc.background.as_mut().and_then(|bg| bg.watermark.as_mut())
                    {
                        watermark.offset += (c - last) * k;
                        self.drag = Some(EditDrag::Watermark { last: c });
                        self.touch();
                    }
                }
                EditDrag::Handle { index, handle } => {
                    if let Some(it) = self.doc.items.get_mut(index) {
                        it.set_handle(handle, b, shift);
                        self.touch();
                    }
                }
                EditDrag::Create { index, start } => {
                    if let Some(it) = self.doc.items.get_mut(index) {
                        update_created(it, start, b, shift);
                        self.touch();
                    }
                }
            }
        }

        if resp.drag_stopped() {
            if let Some(EditDrag::Create { index, .. }) = self.drag
                && let Some(it) = self.doc.items.get_mut(index)
            {
                if let Shape::Pen { pts } = &mut it.shape {
                    *pts = smooth(pts);
                }
                let r = it.bounds();
                let tiny = match &it.shape {
                    Shape::Pen { .. } | Shape::Highlighter { .. } => false,
                    _ => r.width().max(r.height()) < 4.0 + it.width,
                };
                if tiny {
                    self.doc.items.remove(index);
                    self.undo.pop();
                } else {
                    self.selected = Some(index);
                }
                self.touch();
            }
            self.drag = None;
        }

        if resp.clicked()
            && let Some(p) = pointer
        {
            let b = view.to_base(p);
            let hit = self.doc.items.iter().rposition(|it| it.hit(b, tol));
            match self.tool {
                Tool::Text => {
                    self.commit_text();
                    match hit.filter(|&i| matches!(self.doc.items[i].shape, Shape::Text { .. })) {
                        Some(i) => self.start_text_edit(i),
                        None => {
                            self.checkpoint();
                            let size = self.text_size;
                            let pos = b - vec2(0.0, size * 0.6);
                            // A paragraph wraps at the image's right edge, so typing never runs off it.
                            let wrap = self.text_paragraph.then(|| (self.doc.base.width() as f32 - pos.x).max(size * 4.0));
                            let item = self.new_item(Shape::Text { pos, text: String::new(), size, style: self.text_style, wrap });
                            self.doc.items.push(item);
                            self.start_text_edit(self.doc.items.len() - 1);
                        }
                    }
                }
                Tool::Counter => {
                    // Clicking a step that is already there adjusts it; a click on empty space adds the next one.
                    match hit.filter(|&i| matches!(self.doc.items[i].shape, Shape::Counter { .. })) {
                        Some(i) => self.selected = Some(i),
                        None => {
                            self.checkpoint();
                            let n = self.doc.next_counter();
                            let item = self.new_item(Shape::Counter { center: b, n, style: self.counter_style.clone() });
                            self.doc.items.push(item);
                            self.selected = Some(self.doc.items.len() - 1);
                            self.touch();
                        }
                    }
                }
                Tool::Pan => {}
                _ => {
                    self.commit_text();
                    self.selected = hit;
                }
            }
        }
        if resp.double_clicked()
            && let Some(p) = pointer
        {
            let b = view.to_base(p);
            let hit = self.doc.items.iter().rposition(|it| it.hit(b, tol));
            match hit.map(|i| (i, &self.doc.items[i].shape)) {
                Some((i, Shape::Text { .. })) => self.start_text_edit(i),
                // A double click on a step brings its look back into the options row.
                Some((i, Shape::Counter { .. })) => {
                    self.selected = Some(i);
                    self.tool = Tool::Counter;
                }
                _ => {}
            }
        }

        // Hover cursor feedback.
        if let Some(p) = resp.hover_pos() {
            let b = view.to_base(p);
            let over_handle = self.selected.and_then(|i| self.doc.items.get(i)).is_some_and(|it| it.handles().iter().any(|h| h.distance(b) <= tol * 1.5));
            let icon = if over_handle {
                egui::CursorIcon::Grab
            } else if self.hand() {
                if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab }
            } else if self.tool == Tool::Select {
                if self.doc.items.iter().any(|it| it.hit(b, tol)) { egui::CursorIcon::Move } else { egui::CursorIcon::Default }
            } else if self.tool == Tool::Text {
                egui::CursorIcon::Text
            } else {
                egui::CursorIcon::Crosshair
            };
            ctx.set_cursor_icon(icon);
        }

        self.draw_selection(ui);
        self.text_editor(ui);
    }

    fn draw_selection(&self, ui: &egui::Ui) {
        let Some(it) = self.selected.and_then(|i| self.doc.items.get(i)) else { return };
        if self.editing.is_some() {
            return;
        }
        let view = self.view;
        let b = it.bounds();
        let r = Rect::from_min_max(view.to_screen(b.min), view.to_screen(b.max)).expand(3.0);
        let p = ui.painter();
        p.rect_stroke(r, 4, Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.8)), StrokeKind::Outside);
        for h in it.handles() {
            let s = view.to_screen(h);
            p.circle_filled(s, 5.0, Color32::WHITE);
            p.circle_stroke(s, 5.0, Stroke::new(1.5, theme::ACCENT));
        }
    }

    fn text_editor(&mut self, ui: &mut egui::Ui) {
        let Some((i, mut text)) = self.editing.clone() else { return };
        let Some(Item { shape: Shape::Text { pos, size, wrap, .. }, color, .. }) = self.doc.items.get(i).cloned() else {
            self.editing = None;
            return;
        };
        let view = self.view;
        let at = view.to_screen(pos);
        let font = FontId::new(size * view.ppb(), FontFamily::Name(fonts::ANNOTATION_FAMILY.into()));
        let id = egui::Id::new("text-edit");
        // A paragraph types inside its wrapping width, so the wrapping the export does is visible
        // while the caret is there.
        let width = wrap.map_or(f32::INFINITY, |w| (w * view.ppb()).max(40.0));
        let out = egui::Area::new(egui::Id::new("text-area")).fixed_pos(at).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            let edit = egui::TextEdit::multiline(&mut text)
                .id(id)
                .font(font)
                .text_color(color)
                .frame(egui::Frame::NONE)
                .desired_rows(1)
                .desired_width(width)
                .margin(Vec2::ZERO);
            ui.add(edit)
        });
        let resp = out.inner;
        if self.focus_text {
            resp.request_focus();
            self.focus_text = false;
        }
        ui.painter().rect_stroke(resp.rect.expand(4.0), 4, Stroke::new(1.0, theme::ACCENT), StrokeKind::Outside);
        self.editing = Some((i, text));
        if resp.lost_focus() {
            self.commit_text();
        }
    }

    fn crop_interaction(&mut self, ui: &mut egui::Ui, resp: &egui::Response) {
        let ctx = ui.ctx().clone();
        let view = self.view;
        let full = self.doc.full_rect();
        let tol = 10.0 / view.ppb();
        let snap = 8.0 / view.ppb();
        // A double click inside the box is the quick way to confirm the crop.
        let double_clicked = resp.double_clicked()
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| self.crop.as_ref().is_some_and(|c| c.rect.contains(view.to_base(p))));
        if double_clicked {
            self.apply_crop();
            return;
        }
        let Some(crop) = &mut self.crop else { return };
        let snap1 = |v: f32, lo: f32, hi: f32| {
            let v = v.clamp(lo, hi);
            if (v - lo).abs() < snap {
                lo
            } else if (v - hi).abs() < snap {
                hi
            } else {
                v
            }
        };
        let snap_pt = |p: Pos2| pos2(snap1(p.x, full.min.x, full.max.x), snap1(p.y, full.min.y, full.max.y));
        let corners = |r: Rect| [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
        if resp.drag_started() {
            let b = view.to_base(ctx.input(|i| i.pointer.press_origin()).unwrap_or_default());
            crop.drag = Some(if let Some(i) = corners(crop.rect).iter().position(|c| c.distance(b) < tol) {
                CropDrag::Corner(i)
            } else if crop.rect.contains(b) && crop.rect != full {
                // A full-image crop can't move; dragging inside it draws a new one instead.
                CropDrag::Move(b)
            } else {
                CropDrag::New(snap_pt(b))
            });
        }
        if let (Some(d), Some(p)) = (crop.drag, resp.interact_pointer_pos()) {
            let b = view.to_base(p);
            let ratio = crop.aspect.map(|(w, h)| w as f32 / h as f32);
            let fix = |anchor: Pos2, q: Pos2| ratio.map_or(q, |r| constrain_aspect(anchor, q, r));
            match d {
                CropDrag::New(a) => crop.rect = Rect::from_two_pos(a, fix(a, snap_pt(b))).intersect(full),
                CropDrag::Corner(i) => {
                    let anchor = corners(crop.rect)[(i + 2) % 4];
                    crop.rect = Rect::from_two_pos(anchor, fix(anchor, snap_pt(b))).intersect(full);
                    crop.drag = Some(CropDrag::Corner(match corners(crop.rect).iter().position(|c| *c == anchor) {
                        Some(ai) => (ai + 2) % 4,
                        None => i,
                    }));
                }
                CropDrag::Move(last) => {
                    let size = crop.rect.size();
                    let max_min = (full.max - size).max(full.min);
                    let min = (crop.rect.min + (b - last)).clamp(full.min, max_min);
                    crop.rect = Rect::from_min_size(snap_pt(min), size);
                    crop.drag = Some(CropDrag::Move(b));
                }
            }
        }
        if resp.drag_stopped() {
            crop.drag = None;
        }
        let r = Rect::from_min_max(view.to_screen(crop.rect.min), view.to_screen(crop.rect.max));
        let p = ui.painter();
        let outer = view.img_rect;
        let dim = Color32::from_black_alpha(120);
        p.rect_filled(Rect::from_min_max(outer.min, pos2(outer.max.x, r.min.y)), 0, dim);
        p.rect_filled(Rect::from_min_max(pos2(outer.min.x, r.max.y), outer.max), 0, dim);
        p.rect_filled(Rect::from_min_max(pos2(outer.min.x, r.min.y), pos2(r.min.x, r.max.y)), 0, dim);
        p.rect_filled(Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(outer.max.x, r.max.y)), 0, dim);
        p.rect_stroke(r, 0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
        for t in [1.0 / 3.0, 2.0 / 3.0] {
            let thin = Stroke::new(0.5, Color32::from_white_alpha(120));
            p.line_segment([pos2(r.left() + r.width() * t, r.top()), pos2(r.left() + r.width() * t, r.bottom())], thin);
            p.line_segment([pos2(r.left(), r.top() + r.height() * t), pos2(r.right(), r.top() + r.height() * t)], thin);
        }
        for c in corners(r) {
            p.circle_filled(c, 6.0, Color32::WHITE);
            p.circle_stroke(c, 6.0, Stroke::new(1.5, theme::ACCENT));
        }
        let label = format!("{} × {}", crop.rect.width().round() as u32, crop.rect.height().round() as u32);
        p.text(r.center_top() - vec2(0.0, 12.0), Align2::CENTER_CENTER, label, FontId::proportional(12.0), Color32::WHITE);
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    fn crop_bar(&mut self, ui: &mut egui::Ui) {
        let full = self.doc.full_rect();
        let Some(aspect) = self.crop.as_ref().map(|c| c.aspect) else { return };
        let mut action = None;
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new(format!("{}  Crop", icon::CROP)).strong());
            let presets: [(Option<(u32, u32)>, &str); 6] =
                [(None, "Freeform"), (Some((1, 1)), "1:1"), (Some((4, 3)), "4:3"), (Some((3, 2)), "3:2"), (Some((16, 9)), "16:9"), (Some((9, 16)), "9:16")];
            for (a, l) in presets {
                if ui.selectable_label(aspect == a, l).clicked() {
                    action = Some(CropAction::Aspect(a));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Done").color(Color32::WHITE)).fill(theme::ACCENT)).clicked() {
                    action = Some(CropAction::Done);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(CropAction::Cancel);
                }
                if ui.button("Reset").clicked() {
                    action = Some(CropAction::Reset);
                }
            });
        });
        match action {
            Some(CropAction::Done) => self.apply_crop(),
            Some(CropAction::Cancel) => {
                self.crop = None;
                self.touch();
            }
            Some(CropAction::Reset) => {
                if let Some(c) = &mut self.crop {
                    c.rect = full;
                }
            }
            Some(CropAction::Aspect(a)) => {
                if let Some(c) = &mut self.crop {
                    c.aspect = a;
                    if let Some((w, h)) = a {
                        let end = constrain_aspect(c.rect.min, c.rect.max, w as f32 / h as f32);
                        c.rect = Rect::from_two_pos(c.rect.min, end).intersect(full);
                    }
                }
            }
            None => {}
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let size = render::output_size(&self.doc, &RenderOpts::default());
            let k = self.export_percent as f32 / 100.0;
            ui.label(RichText::new(format!("{} × {} px", (size.x * k).round(), (size.y * k).round())).weak());
            ui.separator();
            ui.label(RichText::new(format!("{:.0}%", self.zoom * 100.0)).weak())
                .on_hover_text("Wheel to zoom, Shift+wheel or the hand tool to move, Ctrl/Cmd+0 to fit");
            if let Some((label, _)) = &self.task {
                ui.separator();
                ui.spinner();
                ui.label(*label);
            }
            if let Some((msg, at)) = &self.status {
                if at.elapsed() < Duration::from_secs(5) {
                    ui.separator();
                    ui.label(msg);
                } else {
                    self.status = None;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut fmt = self.settings.export_format;
                egui::ComboBox::from_id_salt("format").selected_text(fmt.label()).width(70.0).show_ui(ui, |ui| {
                    for f in ExportFormat::ALL {
                        ui.selectable_value(&mut fmt, f, f.label());
                    }
                });
                if fmt != self.settings.export_format {
                    self.update_settings(|s| s.export_format = fmt);
                }
                ui.label("Format");
                egui::ComboBox::from_id_salt("scale").selected_text(format!("{}%", self.export_percent)).width(70.0).show_ui(ui, |ui| {
                    for p in [25, 50, 75, 100, 150, 200] {
                        ui.selectable_value(&mut self.export_percent, p, format!("{p}%"));
                    }
                });
                ui.label("Export size");
            });
        });
    }
}

fn rand_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64)
}

/// Update a shape being drawn from `start` to `b`; Shift squares boxes and snaps lines to 45°.
fn update_created(it: &mut Item, start: Pos2, b: Pos2, shift: bool) {
    let boxed = if shift { constrain_aspect(start, b, 1.0) } else { b };
    let snapped = if shift { snap45(start, b) } else { b };
    match &mut it.shape {
        Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Redact { rect, .. } | Shape::Spotlight { rect, .. } => {
            *rect = Rect::from_two_pos(start, boxed)
        }
        Shape::Line { b: end, .. } | Shape::Arrow { b: end, .. } => *end = snapped,
        Shape::Pen { pts } | Shape::Highlighter { pts } => {
            if shift && !pts.is_empty() {
                // Straight stroke from the first point.
                pts.truncate(1);
                pts.push(snap45(pts[0], b));
            } else if pts.last().is_none_or(|l| l.distance(b) > 1.0) {
                pts.push(b);
            }
        }
        _ => {}
    }
}

/// Pan that keeps the point `anchor` fixed when the zoom grows by `factor`; `centre` is the middle
/// of the visible canvas.
fn zoomed_pan(pan: Vec2, centre: Pos2, anchor: Pos2, factor: f32) -> Vec2 {
    (1.0 - factor) * (anchor - centre) + pan * factor
}

/// A small square icon button for the zoom cluster.
fn tiny_button(ui: &mut egui::Ui, glyph: &str, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    let colour = ui.visuals().widgets.inactive.fg_stroke.color;
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(11.5), colour);
    resp.on_hover_text(tip).clicked()
}

/// A titled block of the background panel: padding, a one-pixel rule, then the title.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    let (rule, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rule, 0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    ui.add_space(8.0);
    ui.label(RichText::new(title).strong());
    ui.add_space(2.0);
}

/// A small dial for a shadow direction: click or drag inside it to point the shadow.
fn direction_dial(ui: &mut egui::Ui, angle: &mut f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click_and_drag());
    let center = rect.center();
    let r = 10.0;
    let mut changed = false;
    if let Some(p) = resp.interact_pointer_pos() {
        let v = p - center;
        if v.length() > 1.5 {
            *angle = v.y.atan2(v.x).to_degrees().rem_euclid(360.0);
            changed = true;
        }
    }
    let painter = ui.painter().clone();
    painter.circle_stroke(center, r, Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color));
    let a = angle.to_radians();
    painter.line_segment([center, center + Vec2::angled(a) * r], Stroke::new(2.0, theme::ACCENT));
    painter.circle_filled(center + Vec2::angled(a) * r * 0.55, 2.5, theme::ACCENT);
    resp.on_hover_text("Shadow direction").changed() || changed
}

fn snap45(a: Pos2, b: Pos2) -> Pos2 {
    let d = b - a;
    let angle = (d.y.atan2(d.x) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    a + Vec2::angled(angle) * d.length()
}

/// Two rounds of Chaikin corner cutting: pleasant curves from shaky input.
fn smooth(pts: &[Pos2]) -> Vec<Pos2> {
    let mut cur = pts.to_vec();
    for _ in 0..2 {
        if cur.len() < 3 {
            break;
        }
        let mut next = vec![cur[0]];
        for w in cur.windows(2) {
            next.push(w[0] + (w[1] - w[0]) * 0.25);
            next.push(w[0] + (w[1] - w[0]) * 0.75);
        }
        next.extend(cur.last().copied());
        cur = next;
    }
    cur
}

/// A gradient tile: the four corners take the colour the gradient has at their own position, which
/// is exact for the two-stop presets and close enough for a 44 px tile anywhere else.
fn paint_stops(painter: &egui::Painter, r: Rect, angle: f32, sample: impl Fn(f32) -> Color32) {
    let (sin, cos) = angle.to_radians().sin_cos();
    let half = (r.width() * cos.abs() + r.height() * sin.abs()) / 2.0;
    let t_at = |p: egui::Pos2| {
        let d = p - r.center();
        (0.5 + (d.x * cos + d.y * sin) / (2.0 * half.max(1.0))).clamp(0.0, 1.0)
    };
    let mut mesh = Mesh::default();
    for p in [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()] {
        mesh.colored_vertex(p, sample(t_at(p)));
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(EShape::mesh(mesh));
    painter.rect_stroke(r, 10, Stroke::new(1.0, Color32::from_black_alpha(25)), StrokeKind::Inside);
}

/// The colour at `t` of a built-in gradient, whose stops are evenly spaced.
fn sampled(stops: &[[u8; 3]], t: f32) -> Color32 {
    let Some(first) = stops.first() else { return Color32::BLACK };
    if stops.len() == 1 {
        let [r, g, b] = *first;
        return Color32::from_rgb(r, g, b);
    }
    let x = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (x.floor() as usize).min(stops.len() - 2);
    let k = x - i as f32;
    let (a, b) = (stops[i], stops[i + 1]);
    let mix = |c: usize| (a[c] as f32 + (b[c] as f32 - a[c] as f32) * k).round() as u8;
    Color32::from_rgb(mix(0), mix(1), mix(2))
}

impl eframe::App for Editor {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        if let Some((_, rx)) = &self.task
            && let Ok(result) = rx.try_recv()
        {
            self.task = None;
            match result {
                Ok(msg) => self.flash(msg),
                Err(e) => self.flash(format!("{e:#}")),
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            self.commit_text();
            if self.revision != self.saved_revision && self.history_id.is_some() {
                // History captures autosave; if that fails, keep the window and offer the save dialog.
                if !self.sync_history() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.confirm_close = true;
                }
            } else if self.revision != self.saved_revision && self.project_path.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_close = true;
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::V))
            && self.editing.is_none()
            && let Ok(img) = share::clipboard_image()
        {
            self.add_image_item(img);
        }
        self.handle_shortcuts(ctx);

        egui::Panel::top("toolbar").exact_size(48.0).show(root, |ui| if self.crop.is_some() { self.crop_bar(ui) } else { self.toolbar(ui) });
        if self.crop.is_none() {
            // Per-tool options get their own row so the toolbar never overflows into the actions.
            egui::Panel::top("options").exact_size(36.0).show(root, |ui| {
                ui.horizontal_centered(|ui| self.style_controls(ui));
            });
        }
        egui::Panel::bottom("status").exact_size(32.0).show(root, |ui| self.status_bar(ui));
        if self.show_commentary && self.crop.is_none() {
            if self.commentary_collapsed {
                // A rail with one way back out. Its own id keeps the open panel's width intact.
                egui::Panel::right("commentary-rail").exact_size(RAIL_WIDTH).show(root, |ui| {
                    ui.add_space(4.0);
                    if tiny_button(ui, icon::CARET_LEFT, "Open the commentary panel") {
                        self.commentary_collapsed = false;
                        State::update(|s| s.commentary_collapsed = Some(false));
                    }
                });
            } else {
                egui::Panel::right("commentary").exact_size(self.commentary_width).show(root, |ui| {
                    // Dragging the panel's inner left edge resizes it; the width is remembered.
                    let edge = Rect::from_min_max(ui.max_rect().left_top(), pos2(ui.max_rect().left() + 6.0, ui.max_rect().bottom()));
                    let grip = ui.interact(edge, ui.id().with("commentary-grip"), Sense::drag());
                    if grip.hovered() || grip.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    if grip.dragged() {
                        self.commentary_width = (self.commentary_width - grip.drag_delta().x).clamp(220.0, 640.0);
                        self.panels_dirty = true;
                    }
                    ui.horizontal(|ui| {
                        if tiny_button(ui, icon::CARET_RIGHT, "Collapse this panel") {
                            self.commentary_collapsed = true;
                            State::update(|s| s.commentary_collapsed = Some(true));
                        }
                        ui.label(RichText::new("Commentary").strong());
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| self.commentary_panel(ui));
                });
            }
        }
        if self.show_background && self.crop.is_none() {
            if self.background_collapsed {
                // A rail with one way back out. Its own id keeps the open panel's width intact.
                egui::Panel::left("background-rail").exact_size(RAIL_WIDTH).show(root, |ui| {
                    ui.add_space(4.0);
                    if tiny_button(ui, icon::CARET_RIGHT, "Open the background tools (B)") {
                        self.background_collapsed = false;
                        State::update(|s| s.background_collapsed = Some(false));
                    }
                });
            } else {
                let shown = egui::Panel::left("background").resizable(true).default_size(self.background_width).size_range(200.0..=560.0).show(root, |ui| {
                    ui.add_space(2.0);
                    if tiny_button(ui, icon::CARET_LEFT, "Collapse this panel") {
                        self.background_collapsed = true;
                        State::update(|s| s.background_collapsed = Some(true));
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| self.background_panel(ui));
                });
                let width = shown.response.rect.width();
                if (width - self.background_width).abs() > 1.5 {
                    self.background_width = width;
                    self.panels_dirty = true;
                }
            }
        }
        // Panel sizes are written once the drag is over, not on every frame of it.
        if self.panels_dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.panels_dirty = false;
            let (commentary, width) = (self.commentary_width, self.background_width);
            State::update(|s| {
                s.commentary_width = Some(commentary);
                s.background_width = Some(width);
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| self.canvas(ui));

        self.gradient_window(ctx);
        self.colour_window(ctx);

        if self.confirm_close {
            egui::Modal::new(egui::Id::new("confirm-close")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("Save changes?");
                ui.label("Your edits to this project are not saved.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Save").color(Color32::WHITE)).fill(theme::ACCENT)).clicked() {
                        self.save();
                        // A failed save has already shown its error; keep the window and the edits.
                        if self.saved_revision == self.revision {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                    if ui.button("Don't Save").clicked() {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_close = false;
                    }
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_mapping_round_trips() {
        let v = View {
            img_rect: Rect::from_min_size(pos2(100.0, 50.0), vec2(400.0, 300.0)),
            region: Rect::from_min_size(pos2(20.0, 10.0), vec2(800.0, 600.0)),
            offset: vec2(0.0, 0.0),
            out_scale: 0.5,
            out_size: vec2(400.0, 300.0),
        };
        let b = pos2(321.0, 123.0);
        assert!(v.to_base(v.to_screen(b)).distance(b) < 1e-3);
        assert!((v.ppb() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn shift_snaps_lines_to_45_degrees() {
        let p = snap45(pos2(0.0, 0.0), pos2(10.0, 9.0));
        assert!((p.x - p.y).abs() < 1e-3);
    }

    #[test]
    fn smoothing_keeps_endpoints() {
        let pts = vec![pos2(0.0, 0.0), pos2(10.0, 0.0), pos2(10.0, 10.0), pos2(20.0, 10.0)];
        let s = smooth(&pts);
        assert_eq!(s.first(), pts.first());
        assert_eq!(s.last(), pts.last());
        assert!(s.len() > pts.len());
    }

    /// Zooming must keep the base pixel under the pointer where it is, which is what makes a wheel
    /// zoom feel like it follows the mouse.
    #[test]
    fn zooming_holds_the_point_under_the_pointer() {
        let canvas = Rect::from_min_size(pos2(100.0, 60.0), vec2(900.0, 700.0));
        let region = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        let view = |zoom: f32, pan: Vec2| View {
            img_rect: Rect::from_center_size(canvas.center() + pan, region.size() * zoom),
            region,
            offset: Vec2::ZERO,
            out_scale: 1.0,
            out_size: region.size(),
        };
        let pointer = pos2(320.0, 240.0);
        let start = view(1.0, Vec2::ZERO);
        for factor in [1.3, 0.6, 4.0] {
            let pan = zoomed_pan(Vec2::ZERO, canvas.center(), pointer, factor);
            let zoomed = view(factor, pan);
            let before = start.to_base(pointer);
            let after = zoomed.to_base(pointer);
            assert!(before.distance(after) < 0.01, "×{factor}: {before:?} became {after:?}");
        }
    }
}
