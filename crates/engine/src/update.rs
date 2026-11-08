//! New versions from GitHub: ask the releases API, download the file for this system and start its
//! installer.
//!
//! Nothing here blocks on the UI: every call is one request or one download, and the windows run
//! them off the frame thread.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::Deserialize;

use crate::config::{self, State};
use crate::{Error, Result};

/// Where releases are published.
pub const REPO: &str = "jamesbuckley-source/KeptCraft";
/// The releases page, for a system whose package needs a step of its own.
pub const RELEASES_PAGE: &str = "https://github.com/jamesbuckley-source/KeptCraft/releases";

const API: &str = "https://api.github.com/repos/jamesbuckley-source/KeptCraft/releases";
/// How long an automatic check waits before asking again.
pub const CHECK_EVERY: i64 = 24 * 60 * 60;
/// An installer is large but not unbounded; refuse anything past this so a hostile response cannot
/// fill the disk.
const MAX_ASSET_BYTES: u64 = 512 << 20;
const MAX_JSON_BYTES: u64 = 4 << 20;
const USER_AGENT: &str = concat!("KeptCraft-updater/", env!("CARGO_PKG_VERSION"));

/// This build's version.
pub fn current() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| Error::Other(format!("this build has an odd version number: {e}")))
}

pub fn current_label() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A file on the release, ready to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// A newer release than this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release notes, as written on GitHub.
    pub notes: String,
    /// The release's page, which every platform can open.
    pub page: String,
    /// The file to install on this system, when the release has one.
    pub asset: Option<Asset>,
}

impl Release {
    pub fn version_label(&self) -> String {
        self.version.to_string()
    }

    /// What to call the download while it runs.
    pub fn asset_name(&self) -> Option<&str> {
        self.asset.as_ref().map(|a| a.name.as_str())
    }

    /// Whether the user asked not to be told about this version again.
    pub fn is_skipped(&self) -> bool {
        State::load().update_skipped.as_deref().is_some_and(|skipped| skipped == self.version.to_string())
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

/// Seconds since the Unix epoch, or 0 for a clock set before it.
pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Whether an automatic check is due: no check yet, or the last one was a day ago.
pub fn due() -> bool {
    match State::load().update_checked_at {
        Some(at) => now().saturating_sub(at) >= CHECK_EVERY,
        None => true,
    }
}

/// Note that a check happened, so the next automatic one waits a day.
pub fn record_check() {
    State::update(|s| s.update_checked_at = Some(now()));
}

/// Stop offering `version`.
pub fn skip(version: &str) {
    State::update(|s| s.update_skipped = Some(version.to_owned()));
}

/// The newest published release, if it is newer than this build.
///
/// A build that is itself a pre-release is offered pre-releases too, since that is the channel it
/// was installed from; a stable build is only offered stable releases.
pub fn fetch() -> Result<Option<Release>> {
    let current = current()?;
    let mut bytes = Vec::new();
    ureq::get(API)
        .config()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| Error::Other(format!("could not reach GitHub: {e}")))?
        .into_body()
        .into_reader()
        .take(MAX_JSON_BYTES)
        .read_to_end(&mut bytes)?;
    let releases: Vec<ApiRelease> = serde_json::from_slice(&bytes)?;
    Ok(newest(&releases, &current))
}

/// The highest version in `releases` that is newer than `current`, in the channel this build came
/// from.
fn newest(releases: &[ApiRelease], current: &Version) -> Option<Release> {
    let allow_pre = !current.pre.is_empty();
    releases
        .iter()
        .filter(|r| !r.draft && (allow_pre || !r.prerelease))
        .filter_map(|r| {
            let version = Version::parse(r.tag_name.trim().trim_start_matches(['v', 'V'])).ok()?;
            (version > *current).then_some((version, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(version, r)| Release {
            version,
            notes: r.body.clone().unwrap_or_default(),
            page: r.html_url.clone(),
            asset: r.assets.iter().find(|a| asset_fits(&a.name)).map(|a| Asset { name: a.name.clone(), url: a.browser_download_url.clone() }),
        })
}

/// Whether an asset is the one this system installs. The packaging scripts name files
/// `keptcraft-<version>-<os>-<arch>.<ext>`, so the name carries the answer.
fn asset_fits(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if cfg!(target_os = "windows") {
        let arch = if cfg!(target_arch = "aarch64") { "windows-arm64" } else { "windows-x64" };
        name.ends_with(".msi") && name.contains(arch)
    } else if cfg!(target_os = "macos") {
        name.ends_with(".dmg")
    } else {
        let arch = if cfg!(target_arch = "aarch64") { "linux-aarch64" } else { "linux-x86_64" };
        name.ends_with(".appimage") && name.contains(arch)
    }
}

/// Download the release's installer into the update folder; the file is written in one step, so an
/// interrupted download is never mistaken for a complete one.
pub fn download(release: &Release) -> Result<PathBuf> {
    let Some(asset) = &release.asset else {
        return Err(Error::Other(format!("KeptCraft {} has no installer for this system", release.version_label())));
    };
    let dir = config::data_dir().join("updates");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&asset.name);
    let mut bytes = Vec::new();
    ureq::get(&asset.url)
        .config()
        .timeout_global(Some(Duration::from_secs(1800)))
        .build()
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| Error::Other(format!("could not download {}: {e}", asset.name)))?
        .into_body()
        .into_reader()
        .take(MAX_ASSET_BYTES)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() {
        return Err(Error::Other(format!("{} downloaded empty", asset.name)));
    }
    crate::doc::write_atomic(&path, &bytes)?;
    Ok(path)
}

/// Download the release and start its installer. On Linux the downloaded package's folder opens
/// instead: a `.deb`, `.rpm` or AppImage takes the user's own install step.
pub fn install(release: &Release) -> Result<()> {
    let path = download(release)?;
    start(&path)
}

/// Open a link in whatever this system uses for it: the release page, when the user wants the files
/// by hand.
pub fn open_page(url: &str) -> Result<()> {
    #[cfg(windows)]
    // Not `cmd /c start`: the shell would read `&` in a URL as its own operator.
    return spawn(std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]));
    #[cfg(target_os = "macos")]
    return spawn(std::process::Command::new("open").arg(url));
    #[cfg(all(unix, not(target_os = "macos")))]
    return spawn(std::process::Command::new("xdg-open").arg(url));
    #[cfg(not(any(windows, unix)))]
    Err(Error::Other("this system has nothing to open a link with".into()))
}

/// Hand a downloaded file to the system's installer.
fn start(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        // A per-machine MSI needs elevation: Start-Process asks for it, and /passive leaves the
        // installer's own progress bar as the only window.
        let file = path.display().to_string().replace('\'', "''");
        let script = format!("Start-Process -FilePath msiexec.exe -ArgumentList @('/i','{file}','/passive') -Verb RunAs");
        spawn(std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]))
    }
    #[cfg(target_os = "macos")]
    return spawn(std::process::Command::new("open").arg(path));
    #[cfg(all(unix, not(target_os = "macos")))]
    // The folder, so the package is one double click away wherever the file manager is.
    return spawn(std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)));
    #[cfg(not(any(windows, unix)))]
    Err(Error::Other("this system has no installer to start".into()))
}

/// Start a program that outlives this one, without a console window, and reap it in the background.
fn spawn(cmd: &mut std::process::Command) -> Result<()> {
    use std::process::Stdio;
    // The installer carries its own messages; nothing it prints can be shown from here.
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flash when the window closes under the updater.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| Error::Other(format!("could not start the installer: {e}")))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn releases(json: &str) -> Vec<ApiRelease> {
        serde_json::from_str(json).unwrap()
    }

    const FIXTURE: &str = r#"[
      {
        "tag_name": "v0.3.0",
        "body": "Numbers, shadows, a faster canvas.",
        "html_url": "https://github.com/jamesbuckley-source/KeptCraft/releases/tag/v0.3.0",
        "assets": [
          {"name": "keptcraft-0.3.0-windows-x64.msi", "browser_download_url": "https://example.invalid/x64.msi"},
          {"name": "keptcraft-0.3.0-windows-arm64.msi", "browser_download_url": "https://example.invalid/arm64.msi"},
          {"name": "keptcraft-0.3.0-linux-x86_64.AppImage", "browser_download_url": "https://example.invalid/x86_64.AppImage"},
          {"name": "keptcraft-0.3.0-macos-universal.dmg", "browser_download_url": "https://example.invalid/universal.dmg"}
        ]
      },
      {"tag_name": "v0.2.0", "html_url": "https://example.invalid/v0.2.0", "assets": []},
      {"tag_name": "v0.4.0", "draft": true, "html_url": "https://example.invalid/v0.4.0", "assets": []}
    ]"#;

    #[test]
    fn the_newest_published_release_wins_and_drafts_are_ignored() {
        let found = newest(&releases(FIXTURE), &Version::parse("0.1.0").unwrap()).unwrap();
        assert_eq!(found.version.to_string(), "0.3.0", "a draft is not published yet");
        assert_eq!(found.notes, "Numbers, shadows, a faster canvas.");
    }

    #[test]
    fn a_release_that_is_not_newer_is_no_news() {
        assert!(newest(&releases(FIXTURE), &Version::parse("0.3.0").unwrap()).is_none());
        assert!(newest(&releases(FIXTURE), &Version::parse("0.3.1").unwrap()).is_none());
    }

    #[test]
    fn a_stable_build_is_not_sent_to_a_pre_release_but_a_pre_release_is_kept_current() {
        let channel = r#"[{"tag_name": "v1.0.0-beta.1", "prerelease": true, "html_url": "https://example.invalid/beta", "assets": []}]"#;
        assert!(newest(&releases(channel), &Version::parse("0.9.0").unwrap()).is_none(), "1.0.0-beta.1 is not for a stable build");
        let beta = newest(&releases(channel), &Version::parse("1.0.0-alpha.1").unwrap()).unwrap();
        assert_eq!(beta.version.to_string(), "1.0.0-beta.1");
    }

    #[test]
    fn the_installer_is_the_one_this_system_runs() {
        let found = newest(&releases(FIXTURE), &Version::parse("0.1.0").unwrap()).unwrap();
        let name = found.asset_name().unwrap_or_default();
        let expected = if cfg!(target_os = "windows") {
            if cfg!(target_arch = "aarch64") { "keptcraft-0.3.0-windows-arm64.msi" } else { "keptcraft-0.3.0-windows-x64.msi" }
        } else if cfg!(target_os = "macos") {
            "keptcraft-0.3.0-macos-universal.dmg"
        } else if cfg!(target_arch = "aarch64") {
            "keptcraft-0.3.0-linux-aarch64.AppImage"
        } else {
            "keptcraft-0.3.0-linux-x86_64.AppImage"
        };
        if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
            assert_eq!(name, "", "the fixture has no aarch64 Linux build");
        } else {
            assert_eq!(name, expected);
        }
    }

    #[test]
    fn this_build_knows_its_own_version() {
        assert_eq!(current().unwrap().to_string(), current_label());
    }
}
