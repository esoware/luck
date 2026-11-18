use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use std::fs;
use std::path::PathBuf;

use crossbeam_channel::Sender;
use ls_types::notification::{Notification as _, PublishDiagnostics, ShowMessage};
use ls_types::{MessageType, PublishDiagnosticsParams, ShowMessageParams, Uri};
use lsp_server::Notification;
use luck_config::Project;

use crate::analyzed::SpareArena;
use crate::line_index::PositionEncoding;
use crate::providers::diagnostics;
use crate::server::Task;
use crate::settings::{ConfigError, SettingsCache};

/// What the main loop shares with every job.
#[derive(Debug)]
pub(crate) struct Session {
    encoding: PositionEncoding,
    settings: SettingsCache,
    spare_arena: Arc<SpareArena>,
    tasks: Sender<Task>,
    is_stopping: AtomicBool,
}

impl Session {
    pub(crate) fn new(encoding: PositionEncoding, tasks: Sender<Task>) -> Self {
        Self {
            encoding,
            settings: SettingsCache::default(),
            spare_arena: Arc::default(),
            tasks,
            is_stopping: AtomicBool::new(false),
        }
    }

    pub(crate) const fn encoding(&self) -> PositionEncoding {
        self.encoding
    }

    pub(crate) const fn settings(&self) -> &SettingsCache {
        &self.settings
    }

    pub(crate) const fn spare_arena(&self) -> &Arc<SpareArena> {
        &self.spare_arena
    }

    /// Hands `task` to the main loop.
    pub(crate) fn send(&self, task: Task) {
        // Only a main loop that has stopped stops listening.
        let _stopped = self.tasks.send(task);
    }

    /// Tells every job still queued to return without doing its work.
    pub(crate) fn stop(&self) {
        self.is_stopping.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_stopping(&self) -> bool {
        self.is_stopping.load(Ordering::Relaxed)
    }

    /// Shows why the `luck.json` that `defaults` stand in for doesn't load,
    /// and publishes the problem on the file at fault.
    pub(crate) fn report_config_error(&self, defaults: &Project, error: ConfigError) {
        let ConfigError { diag, generation } = error;
        let message = format!(
            "luck: the `luck.json` in `{}` doesn't load, so its files use the defaults: {}",
            defaults.root().display(),
            diag.message
        );
        let shown = ShowMessageParams { typ: MessageType::ERROR, message };
        let mut notifications = vec![Notification::new(ShowMessage::METHOD.to_owned(), shown)];
        let path = PathBuf::from(&diag.file_path);
        if let Some(uri) = Uri::from_file_path(&path) {
            // A file that can't be read has no text to place the problem in.
            let text = fs::read_to_string(&path).unwrap_or_default();
            let published = PublishDiagnosticsParams {
                uri,
                diagnostics: vec![diagnostics::config_to_lsp(&diag, &text, self.encoding)],
                version: None,
            };
            notifications.push(Notification::new(PublishDiagnostics::METHOD.to_owned(), published));
        }
        self.send(Task::ConfigError { generation, notifications });
    }
}
