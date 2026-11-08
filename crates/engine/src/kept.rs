//! Kept, the clipboard manager this fork is for: where it keeps its clips and how to point its
//! image editor at this binary.
//!
//! Kept stores a `copypaste.db` SQLite file in the folder its own `storage.json` names, with one
//! row per clip and the image bytes beside it in `media/`. This module reads only what the editor
//! needs (the newest images) and writes nothing but Kept's own `imageEditor` setting.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

/// Tauri's app identifier, which names Kept's configuration folder on every platform.
const KEPT_ID: &str = "ikept.it";

#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    /// The installed `kept.exe`, when there is one to find.
    pub exe: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub db: PathBuf,
    pub media: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub id: i64,
    pub path: PathBuf,
    pub thumb: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// Unix milliseconds, as Kept records it.
    pub created_ms: i64,
    pub source_app: Option<String>,
}

impl Clip {
    /// "2026-11-07 17:02 · 442×1056 · Screenshot", for a menu entry.
    pub fn label(&self) -> String {
        let when = chrono::DateTime::from_timestamp_millis(self.created_ms)
            .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| self.created_ms.to_string());
        let source = self.source_app.as_deref().unwrap_or("Kept");
        format!("{when}  {}×{}  {source}", self.width, self.height)
    }
}

#[derive(Deserialize)]
struct Storage {
    #[serde(rename = "dataDir")]
    data_dir: String,
}

/// Find Kept through its configuration, or `None` when it is not installed (or never started).
pub fn detect() -> Option<Kept> {
    detect_near(&dirs::config_dir()?, &dirs::data_local_dir().unwrap_or_default())
}

fn detect_near(config_dir: &Path, local_data: &Path) -> Option<Kept> {
    let text = std::fs::read_to_string(config_dir.join(KEPT_ID).join("storage.json")).ok()?;
    let storage: Storage = serde_json::from_str(&text).ok()?;
    let data_dir = PathBuf::from(storage.data_dir.trim());
    if data_dir.as_os_str().is_empty() {
        return None;
    }
    let exe = [local_data.join("Kept").join(exe_name()), PathBuf::from(r"C:\Program Files\Kept").join(exe_name())].into_iter().find(|p| p.is_file());
    let db = data_dir.join("copypaste.db");
    db.is_file().then(|| Kept { exe, data_dir: data_dir.clone(), db, media: data_dir.join("media") })
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "kept.exe" } else { "kept" }
}

/// The command line Kept runs for "edit this image", in the form Kept's setting holds: a whole
/// command line with `{file}` where the path goes.
pub fn editor_command(exe: &Path) -> String {
    format!("\"{}\" open-annotate --filepath {{file}} --overwrite", exe.display())
}

fn open(db: &Path, read_only: bool) -> Result<rusqlite::Connection> {
    let flags = if read_only { rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY } else { rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE };
    let conn = rusqlite::Connection::open_with_flags(db, flags).map_err(|e| Error::Other(format!("cannot open {}: {e}", db.display())))?;
    // Kept keeps the database open and writes often; wait for its lock instead of failing at once.
    let _ = conn.busy_timeout(std::time::Duration::from_millis(1500));
    Ok(conn)
}

/// The newest image clips, newest first.
pub fn recent_images(kept: &Kept, limit: u32) -> Result<Vec<Clip>> {
    let conn = open(&kept.db, true)?;
    let mut stmt = conn
        .prepare(
            "select id, image_path, thumb_path, width, height, created_at, source_app
             from clips
             where kind = 'image' and image_path is not null
             order by created_at desc
             limit ?1",
        )
        .map_err(sql)?;
    let rows = stmt
        .query_map([limit], |row| {
            let image: String = row.get(1)?;
            let thumb: Option<String> = row.get(2)?;
            Ok(Clip {
                id: row.get(0)?,
                path: kept.media.join(image),
                thumb: thumb.map(|t| kept.media.join(t)),
                width: row.get::<_, Option<i64>>(3)?.unwrap_or_default().max(0) as u32,
                height: row.get::<_, Option<i64>>(4)?.unwrap_or_default().max(0) as u32,
                created_ms: row.get(5)?,
                source_app: row.get(6)?,
            })
        })
        .map_err(sql)?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
}

/// Point Kept's image editor at `command`. Kept keeps settings as JSON text, so the value is encoded
/// the way it writes it, and Kept picks it up the next time it reads its settings.
pub fn set_image_editor(kept: &Kept, command: &str) -> Result<()> {
    let conn = open(&kept.db, false)?;
    let value = serde_json::to_string(command)?;
    conn.execute("insert into settings(key, value) values('imageEditor', ?1) on conflict(key) do update set value = excluded.value", [value]).map_err(sql)?;
    Ok(())
}

/// Kept's current image editor command, decoded, for showing what was (or will be) replaced.
pub fn image_editor(kept: &Kept) -> Result<Option<String>> {
    use rusqlite::OptionalExtension as _;
    let conn = open(&kept.db, true)?;
    let raw: Option<String> = conn.query_row("select value from settings where key = 'imageEditor'", [], |row| row.get(0)).optional().map_err(sql)?;
    Ok(raw.and_then(|v| serde_json::from_str::<String>(&v).ok()))
}

/// Whether Kept is running. It holds its settings in memory and writes them back, so a write from
/// outside is lost while it runs, and its database can refuse one.
pub fn running() -> bool {
    if cfg!(windows) {
        std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq kept.exe", "/NH"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_ascii_lowercase().contains("kept.exe"))
            .unwrap_or(false)
    } else {
        false
    }
}

fn sql(e: rusqlite::Error) -> Error {
    Error::Other(format!("Kept database: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_reads_kepts_own_configuration() {
        let root = std::env::temp_dir().join(format!("keptcraft-kept-{}", std::process::id()));
        let config = root.join("config");
        let local = root.join("local");
        let data = root.join("data");
        std::fs::create_dir_all(config.join(KEPT_ID)).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let storage = format!("{{\"dataDir\": \"{}\", \"previousDir\": null}}", data.display().to_string().replace('\\', "\\\\"));
        std::fs::write(config.join(KEPT_ID).join("storage.json"), storage).unwrap();
        // No database yet: Kept has never stored a clip, so there is nothing to integrate with.
        assert!(detect_near(&config, &local).is_none());
        std::fs::write(data.join("copypaste.db"), b"").unwrap();
        let kept = detect_near(&config, &local).unwrap();
        assert_eq!((kept.data_dir.clone(), kept.media.clone()), (data.clone(), data.join("media")));
        assert!(kept.exe.is_none(), "no kept.exe beside the config");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_editor_command_is_a_whole_command_line() {
        let command = editor_command(Path::new(r"C:\Program Files\KeptCraft\keptcraft.exe"));
        assert_eq!(command, "\"C:\\Program Files\\KeptCraft\\keptcraft.exe\" open-annotate --filepath {file} --overwrite");
    }

    #[test]
    fn reads_the_newest_images_and_writes_the_editor_setting() {
        let dir = std::env::temp_dir().join(format!("keptcraft-kept-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("media")).unwrap();
        let kept = Kept { exe: None, data_dir: dir.clone(), db: dir.join("copypaste.db"), media: dir.join("media") };
        {
            let conn = rusqlite::Connection::open(&kept.db).unwrap();
            conn.execute_batch(
                "create table clips (id integer primary key, kind text, image_path text, thumb_path text, width integer, height integer, created_at integer, source_app text);
                 create table settings (key text primary key, value text not null);
                 insert into clips values (1, 'image', 'a.png', 'a.thumb.png', 10, 20, 100, 'Screenshot');
                 insert into clips values (2, 'text', null, null, null, null, 300, null);
                 insert into clips values (3, 'image', 'b.png', null, 30, 40, 200, null);",
            )
            .unwrap();
        }
        let clips = recent_images(&kept, 20).unwrap();
        assert_eq!(clips.iter().map(|c| c.id).collect::<Vec<_>>(), [3, 1], "newest image first, text clips skipped");
        assert_eq!(clips[1].path, kept.media.join("a.png"));
        assert!(clips[0].label().contains("30×40") && clips[0].label().ends_with("Kept"), "{}", clips[0].label());
        set_image_editor(&kept, "cmd {file}").unwrap();
        assert_eq!(image_editor(&kept).unwrap().as_deref(), Some("cmd {file}"));
        set_image_editor(&kept, "next {file}").unwrap();
        assert_eq!(image_editor(&kept).unwrap().as_deref(), Some("next {file}"), "the setting is replaced, not duplicated");
        std::fs::remove_dir_all(&dir).ok();
    }
}
