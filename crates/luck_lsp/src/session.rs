use std::num::NonZero;
use std::path::Path;
use std::slice;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use luck_allocator::AllocatorPool;
use luck_config::{ConfigError, Project, located_path};
use luck_resolver::{FileSystem as _, ResolverGeneric};
use luck_type_checker::luau_config_aliases;

use crate::analyzed::SpareArena;
use crate::files::Files;
use crate::line_index::PositionEncoding;
use crate::providers::diagnostics;
use crate::server::{Problem, Task};
use crate::settings::{DocumentSettings, GivenConfig, LoadError, LoadedProject, SettingsCache};
use crate::type_check::TypeChecks;
use crate::workspace::Workspace;

/// What the main loop shares with every job.
#[derive(Debug)]
pub(crate) struct Session {
    encoding: PositionEncoding,
    files: Files,
    /// What every require resolves through, which the main loop clears once
    /// a file changes in a way that can change what one resolves to.
    resolver: Arc<ResolverGeneric<Files>>,
    settings: SettingsCache,
    workspace: Workspace,
    type_checks: TypeChecks,
    spare_arena: Arc<SpareArena>,
    /// The arenas files that aren't open parse in, one at a time per worker.
    allocators: AllocatorPool,
    /// How many workers the pool has, and each type check starts.
    threads: NonZero<usize>,
    tasks: Sender<Task>,
    is_stopping: AtomicBool,
}

impl Session {
    /// A session that reads every source through `files`, under `given`, the
    /// config the client gave, if it gave one.
    pub(crate) fn new(
        encoding: PositionEncoding,
        tasks: Sender<Task>,
        threads: NonZero<usize>,
        files: Files,
        given: Option<GivenConfig>,
    ) -> Self {
        let resolver = ResolverGeneric::new_with_file_system(files.clone())
            .with_luau_config(luau_config_aliases);
        let resolver = Arc::new(resolver);
        Self {
            encoding,
            settings: SettingsCache::new(files.clone(), Arc::clone(&resolver), given),
            resolver,
            workspace: Workspace::new(files.clone()),
            type_checks: TypeChecks::default(),
            files,
            spare_arena: Arc::default(),
            allocators: AllocatorPool::new(threads.get()),
            threads,
            tasks,
            is_stopping: AtomicBool::new(false),
        }
    }

    pub(crate) const fn encoding(&self) -> PositionEncoding {
        self.encoding
    }

    pub(crate) const fn files(&self) -> &Files {
        &self.files
    }

    pub(crate) const fn resolver(&self) -> &Arc<ResolverGeneric<Files>> {
        &self.resolver
    }

    pub(crate) const fn settings(&self) -> &SettingsCache {
        &self.settings
    }

    pub(crate) const fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub(crate) const fn type_checks(&self) -> &TypeChecks {
        &self.type_checks
    }

    pub(crate) const fn threads(&self) -> NonZero<usize> {
        self.threads
    }

    pub(crate) const fn spare_arena(&self) -> &Arc<SpareArena> {
        &self.spare_arena
    }

    pub(crate) const fn allocators(&self) -> &AllocatorPool {
        &self.allocators
    }

    /// The project of the file at `path`, whose directory is canonical, or
    /// of an untitled document, which reports why its config doesn't load if
    /// this is the first lookup to find it, under the directories of
    /// `spelled`, the path as the client spells it.
    pub(crate) fn project_for(
        &self,
        path: Option<&Path>,
        spelled: Option<&Path>,
    ) -> Arc<LoadedProject> {
        let (project, error) = self.settings.project_for(path, spelled);
        if let Some(load_error) = error {
            self.report_config_error(project.project(), load_error);
        }
        project
    }

    /// The settings of the document at `spelled`, its path as the client
    /// spells it, or of an untitled document, under the project
    /// [`Self::project_for`] finds.
    pub(crate) fn document_settings(&self, spelled: Option<&Path>) -> DocumentSettings {
        let file = spelled.map(|path| located_path(&self.files, path));
        let project = self.project_for(file.as_deref(), spelled);
        self.settings.document_settings(project, file.as_deref())
    }

    /// Hands `task` to the main loop.
    pub(crate) fn send(&self, task: Task) {
        // Only a main loop that has stopped stops listening.
        let _ = self.tasks.send(task);
    }

    /// Tells every job still queued to return without doing its work.
    pub(crate) fn stop(&self) {
        self.is_stopping.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_stopping(&self) -> bool {
        self.is_stopping.load(Ordering::Relaxed)
    }

    /// Shows why the config that `defaults` stand in for doesn't load, and
    /// publishes a problem in a file on that file.
    fn report_config_error(&self, defaults: &Project, error: LoadError) {
        let LoadError { error, generation } = error;
        let (diags, file) = match &error {
            ConfigError::Given(diag) => (slice::from_ref(diag), None),
            ConfigError::File(file) => (file.diags.as_slice(), Some(file)),
        };
        let problems: Vec<&str> = diags.iter().map(|diag| &*diag.message).collect();
        let config = if self.settings.is_given() {
            "the config the client gave".to_owned()
        } else {
            format!("the `luck.json` in `{}`", defaults.root().display())
        };
        let message = format!(
            "luck: {config} doesn't load, so its files use the defaults: {}",
            problems.join("; ")
        );
        let published = file.and_then(|file| {
            let uri = self.files.uri(&file.path)?;
            // A file that can't be read has no text to place the problem in.
            let bytes = self.files.read(&file.path).unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes);
            let lsp_diags = diagnostics::config_to_lsp(&uri, &file.diags, &text, self.encoding);
            Some((uri, lsp_diags))
        });
        self.send(Task::ConfigError { generation, problem: Problem { message, published } });
    }
}
