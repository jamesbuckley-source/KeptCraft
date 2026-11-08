//! The GitHub update check, its download and its installer, run off the frame thread.
//!
//! Both windows that offer an update (the editor's dialog and the settings page) drive one of
//! these: [`Updater::poll`] once a frame, then whatever their own surface shows for the stage.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use keptcraft_engine::update::{self, Release};

/// Where the updater has got to.
#[derive(Clone, Debug)]
pub enum Stage {
    /// Nothing to report.
    Idle,
    /// Asking GitHub.
    Checking,
    /// A newer release waits for the user's answer.
    Available(Box<Release>),
    /// Its installer is downloading; the name is the file.
    Downloading(String),
    /// The installer is running, and the app should close so it can be replaced.
    Started,
    /// The check found nothing newer.
    UpToDate,
    /// What went wrong, or what the user asked for.
    Message(String),
}

enum Msg {
    Checked(Result<Option<Release>, String>),
    Installed(Result<(), String>),
}

pub struct Updater {
    stage: Stage,
    task: Option<Receiver<Msg>>,
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

impl Updater {
    pub fn new() -> Self {
        Self { stage: Stage::Idle, task: None }
    }

    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The release waiting for an answer, if one is.
    pub fn offering(&self) -> Option<&Release> {
        match &self.stage {
            Stage::Available(release) => Some(release),
            _ => None,
        }
    }

    /// Whether a request is in flight.
    pub fn busy(&self) -> bool {
        matches!(self.stage, Stage::Checking | Stage::Downloading(_))
    }

    /// Check now if the preference allows it and the last check was a day ago.
    pub fn auto_check(&mut self, allowed: bool) {
        if allowed && matches!(self.stage, Stage::Idle) && update::due() {
            self.start_check();
        }
    }

    /// Check now, whatever the preference and the clock say. A failed check is worth retrying by
    /// hand, so it always runs.
    pub fn check_now(&mut self) {
        self.start_check();
    }

    /// Download the offered release and start its installer.
    pub fn install(&mut self) {
        let Some(release) = self.offering().cloned() else { return };
        let name = release.asset_name().map(str::to_owned).unwrap_or_else(|| release.version_label());
        self.stage = Stage::Downloading(name);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Installed(update::install(&release).map_err(|e| format!("{e:#}"))));
        });
        self.task = Some(rx);
    }

    /// Remember not to mention this version again.
    pub fn skip(&mut self) {
        if let Some(release) = self.offering().cloned() {
            update::skip(&release.version_label());
            self.stage = Stage::Message(format!("KeptCraft {} will not be mentioned again.", release.version_label()));
        }
    }

    /// The user said later: forget the offer until the next launch.
    pub fn dismiss(&mut self) {
        if self.offering().is_some() {
            self.stage = Stage::Idle;
        }
    }

    /// Pick up what the worker finished. Call once a frame; returns the new stage when one arrived.
    pub fn poll(&mut self) -> Option<Stage> {
        let Some(task) = &self.task else { return None };
        let message = match task.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                self.stage = Stage::Message("The update check stopped unexpectedly.".to_owned());
                self.task = None;
                return Some(self.stage.clone());
            }
        };
        self.task = None;
        self.stage = match message {
            Msg::Checked(Ok(None)) => Stage::UpToDate,
            Msg::Checked(Ok(Some(release))) if release.is_skipped() => Stage::Idle,
            Msg::Checked(Ok(Some(release))) => Stage::Available(Box::new(release)),
            Msg::Checked(Err(error)) => Stage::Message(error),
            Msg::Installed(Ok(())) => Stage::Started,
            Msg::Installed(Err(error)) => Stage::Message(error),
        };
        Some(self.stage.clone())
    }

    fn start_check(&mut self) {
        self.stage = Stage::Checking;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let found = update::fetch().map_err(|e| format!("{e:#}"));
            // Written here as well as on success: a failure is still a check, and a daily retry of
            // an offline machine is not worth a request every launch.
            update::record_check();
            let _ = tx.send(Msg::Checked(found));
        });
        self.task = Some(rx);
    }
}
