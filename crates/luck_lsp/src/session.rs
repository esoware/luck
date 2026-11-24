use alloc::sync::Arc;
use core::num::NonZero;
use core::sync::atomic::{AtomicBool, Ordering};
use std::fs;
use std::path::Path;

use crossbeam_channel::Sender;
use ls_types::notification::{Notification as _, PublishDiagnostics, ShowMessage};
use ls_types::{MessageType, PublishDiagnosticsParams, ShowMessageParams, Uri};
use lsp_server::Notification;
use luck_allocator::AllocatorPool;
use luck_config::Project;

use crate::analyzed::SpareArena;
use crate::line_index::PositionEncoding;
use crate::providers::diagnostics;
use crate::server::Task;
use crate::settings::{ConfigError, LoadedProject, SettingsCache};
use crate::workspace::Workspace;

/// What the main loop shares with every job.
#[derive(Debug)]
pub(crate) struct Session {
    encoding: PositionEncoding,
    settings: SettingsCache,
    workspace: Workspace,
    spare_arena: Arc<SpareArena>,
    /// The arenas files read from disk parse in, one at a time per worker.
    allocators: AllocatorPool,
    tasks: Sender<Task>,
    is_stopping: AtomicBool,
}

impl Session {
    pub(crate) fn new(
        encoding: PositionEncoding,
        tasks: Sender<Task>,
        threads: NonZero<usize>,
    ) -> Self {
        Self {
            encoding,
            settings: SettingsCache::default(),
            workspace: Workspace::default(),
            spare_arena: Arc::default(),
            allocators: AllocatorPool::new(threads.get()),
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

    pub(crate) const fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub(crate) const fn spare_arena(&self) -> &Arc<SpareArena> {
        &self.spare_arena
    }

    pub(crate) const fn allocators(&self) -> &AllocatorPool {
        &self.allocators
    }

    /// The project of the file at `path`, a canonical path, or of an
    /// untitled document, which reports why its `luck.json` doesn't load if
    /// this is the first lookup to find it, under the directories of
    /// `spelled`, the path as the client spells it.
    pub(crate) fn project_for(
        &self,
        path: Option<&Path>,
        spelled: Option<&Path>,
    ) -> Arc<LoadedProject> {
        let (project, error) = self.settings.project_for(path, spelled);
        if let Some(config_error) = error {
            self.report_config_error(project.project(), config_error);
        }
        project
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
    fn report_config_error(&self, defaults: &Project, error: ConfigError) {
        let ConfigError { file, generation } = error;
        let problems: Vec<&str> = file.diags.iter().map(|diag| &*diag.message).collect();
        let message = format!(
            "luck: the `luck.json` in `{}` doesn't load, so its files use the defaults: {}",
            defaults.root().display(),
            problems.join("; ")
        );
        let shown = ShowMessageParams { typ: MessageType::ERROR, message };
        let mut notifications = vec![Notification::new(ShowMessage::METHOD.to_owned(), shown)];
        if let Some(uri) = Uri::from_file_path(&file.path) {
            // A file that can't be read has no text to place the problem in.
            let text = fs::read_to_string(&file.path).unwrap_or_default();
            let diagnostics = diagnostics::config_to_lsp(&uri, &file.diags, &text, self.encoding);
            let published = PublishDiagnosticsParams { uri, diagnostics, version: None };
            notifications.push(Notification::new(PublishDiagnostics::METHOD.to_owned(), published));
        }
        self.send(Task::ConfigError { generation, notifications });
    }
}
