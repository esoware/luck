//! The files of the client's folders, and what each holds for the others.
//!
//! The workspace's files are the Lua files `luck lint` takes in the folders:
//! each under the nearest `luck.json` above it, whose `include` and
//! `exclude` decide, past what `.gitignore` and `.luckignore` hide.
//!
//! The first job that needs the files walks the folders, and the main loop
//! starts a new walk once a file is created or deleted, the folders change,
//! or the settings do. Each file that isn't open is read once into a record,
//! which outlives the arena it was parsed in, until the client's watcher says
//! it changed. A cross-file request reads every open document as it stood
//! when the request came, and every other file's record.

use alloc::sync::Arc;
use core::mem;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ignore::{WalkBuilder, WalkState};
use ls_types::{Position, Uri};
use luck_resolver::{FileSystem as _, FileSystemOs, Resolution, ResolveError};
use luck_semantic::SemanticBuilder;
use luck_token::LuaVersion;
use parking_lot::{Mutex, RwLock};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::analyzed::Arena;
use crate::document::{Document, DocumentState};
use crate::line_index::LineIndex;
use crate::module_record::{Member, ModuleRecord, Require, Table};
use crate::session::Session;
use crate::settings::LoadedProject;

#[derive(Debug, Default)]
pub(crate) struct Workspace {
    walk: RwLock<Arc<Walk>>,
    /// By canonical path.
    files: RwLock<FxHashMap<PathBuf, Arc<DiskFile>>>,
    /// Each file the watcher reported changed since a job last read the
    /// files, as the client named it.
    changed: Mutex<Vec<PathBuf>>,
}

/// One walk of the folders, which the first job that needs its files runs.
#[derive(Debug, Default)]
pub(crate) struct Walk {
    folders: Arc<[PathBuf]>,
    /// Canonical and sorted.
    files: OnceLock<Vec<PathBuf>>,
}

/// A file as it is on disk, read the first time a job needs it.
#[derive(Debug)]
pub(crate) struct DiskFile {
    path: PathBuf,
    /// `None` when the file can't be read.
    record: OnceLock<Option<FileRecord>>,
}

#[derive(Debug)]
struct FileRecord {
    project: Arc<LoadedProject>,
    version: LuaVersion,
    record: ModuleRecord,
}

impl Workspace {
    pub(crate) fn walk(&self) -> Arc<Walk> {
        Arc::clone(&self.walk.read())
    }

    pub(crate) fn is_current(&self, walk: &Arc<Walk>) -> bool {
        Arc::ptr_eq(&self.walk.read(), walk)
    }

    /// Makes `folders` the workspace, which a new walk reads.
    pub(crate) fn set_folders(&self, folders: Vec<PathBuf>) {
        *self.walk.write() = Arc::new(Walk { folders: folders.into(), files: OnceLock::new() });
    }

    /// Starts a new walk of the same folders, as a file created or deleted
    /// needs.
    pub(crate) fn walk_again(&self) {
        let mut walk = self.walk.write();
        *walk = Arc::new(Walk { folders: Arc::clone(&walk.folders), files: OnceLock::new() });
    }

    /// Makes the next job that needs the file at `path` read it again.
    pub(crate) fn forget(&self, path: PathBuf) {
        self.changed.lock().push(path);
    }

    /// Forgets every file read and starts a new walk, as new settings need,
    /// since they decide the files and each one's dialect.
    pub(crate) fn clear(&self) {
        self.files.write().clear();
        self.walk_again();
    }

    /// The file at `path`, a canonical path.
    pub(crate) fn file(&self, path: &Path) -> Arc<DiskFile> {
        let mut changed = self.changed.lock();
        if !changed.is_empty() {
            let mut files = self.files.write();
            for reported in mem::take(&mut *changed) {
                files.remove(&FileSystemOs.canonicalize_existing(&reported));
            }
        }
        drop(changed);
        if let Some(file) = self.files.read().get(path) {
            return Arc::clone(file);
        }
        let read = Arc::new(DiskFile { path: path.to_path_buf(), record: OnceLock::new() });
        Arc::clone(self.files.write().entry(path.to_path_buf()).or_insert(read))
    }

    /// Whether the file at `path` was read since it last changed.
    pub(crate) fn is_read(&self, path: &Path) -> bool {
        self.files.read().get(path).is_some_and(|file| file.record.get().is_some())
    }
}

impl Walk {
    pub(crate) fn has_folders(&self) -> bool {
        !self.folders.is_empty()
    }

    /// Walks the folders the first time a job asks, on that job's thread.
    pub(crate) fn files(&self, session: &Session) -> &[PathBuf] {
        self.files.get_or_init(|| walk(&self.folders, session))
    }
}

impl DiskFile {
    /// Reads the file the first time a job asks, on that job's thread.
    pub(crate) fn source(&self, session: &Session) -> Option<Source<'_>> {
        let read = self.record.get_or_init(|| read(session, &self.path)).as_ref()?;
        Some(Source {
            uri: Uri::from_file_path(&self.path)?,
            path: &self.path,
            record: &read.record,
            project: &read.project,
            version: read.version,
        })
    }
}

/// Every Lua file under `folders` that its project takes, canonical and
/// sorted. A directory that can't be read is passed over.
fn walk(folders: &[PathBuf], session: &Session) -> Vec<PathBuf> {
    // Each folder's canonical path, beside the client's spelling of it.
    let roots: Vec<(PathBuf, &Path)> = folders
        .iter()
        .filter_map(|folder| Some((FileSystemOs.canonicalize(folder).ok()?, folder.as_path())))
        .collect();
    let Some(((first, _), others)) = roots.split_first() else {
        return Vec::new();
    };
    let mut builder = WalkBuilder::new(first);
    for (root, _) in others {
        builder.add(root);
    }
    let found = Mutex::new(Vec::new());
    // Measured in `luck lint`: one thread of the parallel walker reads a tree
    // in half the time the serial walker takes.
    builder.add_custom_ignore_filename(".luckignore").threads(1).build_parallel().run(|| {
        Box::new(|walked| {
            let Ok(entry) = walked else {
                return WalkState::Continue;
            };
            if session.is_stopping() {
                return WalkState::Quit;
            }
            let path = entry.path();
            let Some(file_type) = entry.file_type() else {
                return WalkState::Continue;
            };
            // A directory is in the project its parent is in, and a file in
            // its directory's.
            let spelled = || {
                roots
                    .iter()
                    .find_map(|(root, folder)| Some(folder.join(path.strip_prefix(root).ok()?)))
            };
            let project = || session.project_for(Some(path), spelled().as_deref());
            if file_type.is_dir() {
                let is_excluded =
                    entry.depth() > 0 && project().project().filter().excludes_canonical_tree(path);
                return if is_excluded { WalkState::Skip } else { WalkState::Continue };
            }
            if !path.extension().is_some_and(|extension| extension == "lua" || extension == "luau")
            {
                return WalkState::Continue;
            }
            // The walk follows no symlink, so only a symlink needs the disk to
            // tell where it leads.
            let canonical = if entry.path_is_symlink() {
                FileSystemOs.canonicalize(path).ok().filter(|target| target.is_file())
            } else {
                #[expect(
                    clippy::filetype_is_file,
                    reason = "a pipe or device named like a source file is no file to read"
                )]
                let is_file = file_type.is_file();
                is_file.then(|| path.to_path_buf())
            };
            if let Some(file) = canonical
                && project().project().filter().includes_canonical(&file)
            {
                found.lock().push(file);
            }
            WalkState::Continue
        })
    });
    let mut files = found.into_inner();
    files.sort_unstable();
    files.dedup();
    files
}

/// The file at `path`, parsed in its project's dialect, or `None` when it
/// can't be read. A file that isn't UTF-8 reads as an editor would show it.
fn read(session: &Session, path: &Path) -> Option<FileRecord> {
    let project = session.project_for(Some(path), None);
    let version = project.project().target_for(Some(path)).version();
    let bytes = FileSystemOs.read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let allocator = session.allocators().get();
    let parsed = luck_parser::parse(&allocator, text.as_bytes(), version);
    let chunk = allocator.alloc(parsed.chunk);
    let semantic =
        SemanticBuilder::new().with_counts(parsed.counts).with_requires(true).build(chunk).semantic;
    let line_index = LineIndex::new(&text, session.encoding());
    let mut positions = line_index.positions(&text);
    let record = ModuleRecord::new(chunk, &semantic, |span| positions.range(span));
    Some(FileRecord { project, version, record })
}

/// A file's record, with what resolves its requires.
#[derive(Debug)]
pub(crate) struct Source<'file> {
    pub uri: Uri,
    /// Canonical.
    pub path: &'file Path,
    pub record: &'file ModuleRecord,
    pub project: &'file LoadedProject,
    pub version: LuaVersion,
}

impl<'file> Source<'file> {
    /// The source of `document`, the open document at `uri`, or `None` when
    /// it has no file.
    pub(crate) fn of_document(uri: &Uri, document: &'file DocumentState) -> Option<Self> {
        let settings = document.settings();
        Some(Self {
            uri: uri.clone(),
            path: settings.path.as_deref()?,
            record: document.record(),
            project: &settings.project,
            version: document.version(),
        })
    }

    /// # Errors
    ///
    /// Returns why `require` resolves to no module, or to more than one.
    pub(crate) fn resolve(&self, require: &Require) -> Result<Resolution, ResolveError> {
        self.project.resolver(self.version).resolve(self.path, &require.module)
    }

    /// The module whose table `table` is, or `None` when its require doesn't
    /// resolve.
    pub(crate) fn module_of(&self, table: Table) -> Option<PathBuf> {
        match table {
            Table::Returned | Table::Exported => Some(self.path.to_path_buf()),
            Table::Required(i) => Some(self.resolve(self.record.requires().get(i)?).ok()?.path),
        }
    }

    /// Whether `table` is the table of the module at `module`.
    pub(crate) fn is_table_of(&self, table: Table, module: &Path) -> bool {
        match table {
            Table::Returned | Table::Exported => self.path == module,
            Table::Required(_) => self.module_of(table).is_some_and(|path| path == module),
        }
    }

    /// The member named at `position`, and the module whose table holds it.
    pub(crate) fn member_at(&self, position: Position) -> Option<(&'file Member, PathBuf)> {
        let member = self.record.member_at(position)?;
        Some((member, self.module_of(member.table)?))
    }
}

/// The workspace as one request reads it.
pub(crate) struct WorkspaceView<'job> {
    session: &'job Session,
    /// The open documents as they stood when the request came.
    documents: &'job [(Uri, Arc<Document>)],
    is_cancelled: &'job (dyn Fn() -> bool + Sync),
}

impl<'job> WorkspaceView<'job> {
    pub(crate) const fn new(
        session: &'job Session,
        documents: &'job [(Uri, Arc<Document>)],
        is_cancelled: &'job (dyn Fn() -> bool + Sync),
    ) -> Self {
        Self { session, documents, is_cancelled }
    }

    /// The open document at `path`, a canonical path.
    pub(crate) fn document_at(&self, path: &Path) -> Option<(&'job Uri, &'job DocumentState)> {
        let (uri, document) = self
            .documents
            .iter()
            .find(|(_, document)| document.path(self.session) == Some(path))?;
        Some((uri, document.state(self.session)))
    }

    /// The file at `path` analyzed as an open document would be, from the
    /// open document there, or else from disk, or `None` when it can't be
    /// read.
    pub(crate) fn with_document<R>(
        &self,
        path: &Path,
        read: impl FnOnce(&DocumentState) -> R,
    ) -> Option<R> {
        if let Some((_, document)) = self.document_at(path) {
            return Some(read(document));
        }
        let bytes = FileSystemOs.read(path).ok()?;
        let text: Arc<str> = String::from_utf8_lossy(&bytes).into();
        let project = self.session.project_for(Some(path), None);
        let settings = self.session.settings().document_settings(project, Some(path.to_path_buf()));
        let line_index = LineIndex::new(&text, self.session.encoding());
        let document = DocumentState::new(
            text,
            Arc::new(settings),
            Arc::new(line_index),
            Arena::from_spare(self.session.spare_arena()),
        );
        Some(read(&document))
    }

    /// What `read` gives for the source of the file at `path`: the open
    /// document there, or else the file on disk.
    pub(crate) fn with_source<R>(
        &self,
        path: &Path,
        read: impl FnOnce(&Source<'_>) -> R,
    ) -> Option<R> {
        if let Some((uri, document)) = self.document_at(path) {
            return Source::of_document(uri, document).as_ref().map(read);
        }
        let file = self.session.workspace().file(path);
        file.source(self.session).as_ref().map(read)
    }

    /// Hands `visit` the source of every open document with a file, and
    /// then of every workspace file that isn't open, or stops with `None`
    /// once the request is cancelled.
    pub(crate) fn for_each_source(&self, mut visit: impl FnMut(&Source<'_>)) -> Option<()> {
        let mut open = FxHashSet::default();
        for (uri, document) in self.documents {
            if (self.is_cancelled)() {
                return None;
            }
            let state = document.state(self.session);
            if let Some(source) = Source::of_document(uri, state) {
                open.insert(source.path);
                visit(&source);
            }
        }
        let workspace = self.session.workspace();
        let walk = workspace.walk();
        for path in walk.files(self.session) {
            if (self.is_cancelled)() {
                return None;
            }
            if open.contains(path.as_path()) {
                continue;
            }
            if let Some(source) = workspace.file(path).source(self.session) {
                visit(&source);
            }
        }
        Some(())
    }

    /// Whether the file at `path` is one of the workspace's, which every
    /// cross-file answer reads.
    pub(crate) fn is_workspace_file(&self, path: &Path) -> bool {
        let walk = self.session.workspace().walk();
        walk.files(self.session).binary_search_by(|file| file.as_path().cmp(path)).is_ok()
    }
}
