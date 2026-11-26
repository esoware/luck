//! The files of the client's folders, and what each holds for the others.
//!
//! The workspace's files are the Luau files `luck lint` takes in the folders:
//! each under the nearest `luck.json` above it, whose `include` and
//! `exclude` decide, past what `.gitignore` and `.luckignore` hide. Once the
//! client gives a config, they're the files it holds in the folders, which
//! that config's `include` and `exclude` take.
//!
//! The first job that needs the files walks the folders, and the main loop
//! starts a new walk once a file is created or deleted, the folders change,
//! or the settings do. Each file that isn't open is read once into a record,
//! which outlives the arena it was parsed in, until the client's watcher says
//! it changed. A cross-file request reads every open document as it stood
//! when the request came, and every other file's record.

use std::convert::Infallible;
use std::mem;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use ls_types::{Position, Uri};
use luck_config::{ProjectFilter, WalkEntry, WalkError, WalkRoot, WalkVisitor, walk_source_files};
use luck_resolver::{FileSystem as _, FileSystemOs, is_definition_file};
use luck_semantic::SemanticBuilder;
use parking_lot::{Mutex, RwLock};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::analyzed::Arena;
use crate::document::{Document, DocumentState};
use crate::files::Files;
use crate::line_index::LineIndex;
use crate::module_record::{Member, ModuleRecord, Table};
use crate::session::Session;
use crate::settings::LoadedProject;

#[derive(Debug)]
pub(crate) struct Workspace {
    files_read: Files,
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
    uri: Uri,
    project: Arc<LoadedProject>,
    record: ModuleRecord,
}

impl Workspace {
    /// A workspace of no folders, whose files are read through `files`.
    pub(crate) fn new(files: Files) -> Self {
        Self {
            files_read: files,
            walk: RwLock::default(),
            files: RwLock::default(),
            changed: Mutex::default(),
        }
    }

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
    /// since they decide the files and each one's project.
    pub(crate) fn clear(&self) {
        self.files.write().clear();
        self.walk_again();
    }

    /// The file at `path`, a canonical path.
    pub(crate) fn file(&self, path: &Path) -> Arc<DiskFile> {
        self.forget_changed();
        if let Some(file) = self.files.read().get(path) {
            return Arc::clone(file);
        }
        let read = Arc::new(DiskFile { path: path.to_path_buf(), record: OnceLock::new() });
        Arc::clone(self.files.write().entry(path.to_path_buf()).or_insert(read))
    }

    /// Whether the file at `path`, a canonical path, was read since it last
    /// changed.
    pub(crate) fn is_read(&self, path: &Path) -> bool {
        self.forget_changed();
        self.files.read().get(path).is_some_and(|file| file.record.get().is_some())
    }

    /// Drops the record of each file the watcher reported changed.
    fn forget_changed(&self) {
        let mut changed = self.changed.lock();
        if !changed.is_empty() {
            let mut files = self.files.write();
            for reported in mem::take(&mut *changed) {
                files.remove(&self.files_read.canonicalize_existing(&reported));
            }
        }
    }
}

impl Walk {
    pub(crate) fn has_folders(&self) -> bool {
        !self.folders.is_empty()
    }

    /// Walks the folders the first time a job asks, on that job's thread.
    pub(crate) fn files(&self, session: &Session) -> &[PathBuf] {
        self.files.get_or_init(|| files_under(&self.folders, session))
    }
}

impl DiskFile {
    /// Reads the file the first time a job asks, on that job's thread.
    pub(crate) fn source(&self, session: &Session) -> Option<Source<'_>> {
        let read = self.record.get_or_init(|| read_record(session, &self.path)).as_ref()?;
        Some(Source {
            uri: &read.uri,
            path: &self.path,
            record: &read.record,
            project: &read.project,
        })
    }
}

/// Every Luau file under `folders` that its project takes, canonical and
/// sorted. A directory that can't be read is passed over.
fn files_under(folders: &[PathBuf], session: &Session) -> Vec<PathBuf> {
    if let Some(client) = session.files().client() {
        let mut files = client.paths();
        files.retain(|path| {
            folders.iter().any(|folder| path.starts_with(folder))
                && session.project_for(Some(path), None).project().filter().includes_located(path)
        });
        return files;
    }
    let visitor = FolderVisitor {
        session,
        folders: folders
            .iter()
            .filter_map(|folder| Some((FileSystemOs.canonicalize(folder).ok()?, folder.as_path())))
            .collect(),
    };
    let roots: Vec<WalkRoot<'_>> = visitor
        .folders
        .iter()
        .map(|(canonical, _)| WalkRoot { path: canonical, canonical })
        .collect();
    let Ok(walked) = walk_source_files(&roots, None, &visitor);
    let mut files: Vec<PathBuf> = walked.into_iter().map(|file| file.canonical).collect();
    files.sort_unstable();
    files.dedup();
    files
}

/// The client's folders, walked from their canonical paths, with each
/// directory below one in the project its parent is in, and each file in its
/// directory's.
struct FolderVisitor<'folders> {
    session: &'folders Session,
    /// Each folder's canonical path, beside the client's spelling of it.
    folders: Vec<(PathBuf, &'folders Path)>,
}

impl FolderVisitor<'_> {
    fn filter_of(&self, entry: &WalkEntry<'_>) -> ProjectFilterOf {
        let spelled = self.folders.iter().find_map(|(canonical, folder)| {
            Some(folder.join(entry.path.strip_prefix(canonical).ok()?))
        });
        ProjectFilterOf(self.session.project_for(Some(entry.path), spelled.as_deref()))
    }
}

impl WalkVisitor for FolderVisitor<'_> {
    type Error = Infallible;

    fn directory_filter(
        &self,
        directory: &WalkEntry<'_>,
    ) -> Option<impl Deref<Target = ProjectFilter>> {
        (directory.depth > 0).then(|| self.filter_of(directory))
    }

    fn file_filter(&self, file: &WalkEntry<'_>) -> impl Deref<Target = ProjectFilter> {
        self.filter_of(file)
    }

    fn is_stopping(&self) -> bool {
        self.session.is_stopping()
    }

    fn read_error(&self, _: WalkError) -> Result<(), Infallible> {
        Ok(())
    }
}

struct ProjectFilterOf(Arc<LoadedProject>);

impl Deref for ProjectFilterOf {
    type Target = ProjectFilter;

    fn deref(&self) -> &ProjectFilter {
        self.0.project().filter()
    }
}

/// The file at `path`, parsed, as a definition file when its name says it's
/// one, or `None` when it can't be read or no URI names it. A file that isn't
/// UTF-8 reads as an editor would show it.
fn read_record(session: &Session, path: &Path) -> Option<FileRecord> {
    let project = session.project_for(Some(path), None);
    let bytes = session.files().read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let allocator = session.allocators().get();
    let parsed = if is_definition_file(path) {
        luck_parser::parse_definitions(&allocator, text.as_bytes())
    } else {
        luck_parser::parse(&allocator, text.as_bytes())
    };
    let chunk = allocator.alloc(parsed.chunk);
    let semantic =
        SemanticBuilder::new().with_counts(parsed.counts).with_requires(true).build(chunk).semantic;
    let line_index = LineIndex::new(&text, session.encoding());
    let mut positions = line_index.positions(&text);
    let record = ModuleRecord::new(chunk, &semantic, |span| positions.range(span));
    Some(FileRecord { uri: session.files().uri(path)?, project, record })
}

/// A file's record, with what resolves its requires.
#[derive(Debug)]
pub(crate) struct Source<'file> {
    pub uri: &'file Uri,
    /// Canonical.
    path: &'file Path,
    pub record: &'file ModuleRecord,
    project: &'file LoadedProject,
}

impl<'file> Source<'file> {
    /// The source of `document`, the open document at `uri`, or `None` when
    /// it has no file.
    pub(crate) fn of_document(uri: &'file Uri, document: &'file DocumentState) -> Option<Self> {
        let settings = document.settings();
        Some(Self {
            uri,
            path: settings.path.as_deref()?,
            record: document.record(),
            project: &settings.project,
        })
    }

    /// The module whose table `table` is, or `None` when its require doesn't
    /// resolve.
    fn module_of(&self, table: Table) -> Option<PathBuf> {
        match table {
            Table::Returned | Table::Exported => Some(self.path.to_path_buf()),
            Table::Required(i) => {
                let resolver = self.project.resolver();
                Some(resolver.resolve(self.path, &self.record.requires()[i].module).ok()?.path)
            }
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
    is_cancelled: &'job dyn Fn() -> bool,
}

impl<'job> WorkspaceView<'job> {
    pub(crate) const fn new(
        session: &'job Session,
        documents: &'job [(Uri, Arc<Document>)],
        is_cancelled: &'job dyn Fn() -> bool,
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
    /// open document there, or else from its file, or `None` when it can't be
    /// read.
    pub(crate) fn with_document<R>(
        &self,
        path: &Path,
        read: impl FnOnce(&DocumentState) -> R,
    ) -> Option<R> {
        if let Some((_, document)) = self.document_at(path) {
            return Some(read(document));
        }
        let bytes = self.session.files().read(path).ok()?;
        let text: Arc<str> = String::from_utf8_lossy(&bytes).into();
        let settings = self.session.document_settings(Some(path));
        let line_index = LineIndex::new(&text, self.session.encoding());
        let stdlib = self
            .session
            .type_checks()
            .stdlib(settings.project.project(), self.session.settings().generation());
        let document = DocumentState::new(
            text,
            Arc::new(settings),
            Arc::new(line_index),
            Arena::from_spare(self.session.spare_arena()),
            stdlib,
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::fixtures::session;

    #[test]
    fn a_file_the_watcher_reported_changed_is_not_read() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
        let spelled = directory.path().join("a.luau");
        fs::write(&spelled, "return {}\n").expect("write a.luau");
        let path = FileSystemOs.canonicalize(&spelled).expect("canonicalize");
        let session = session();
        let workspace = session.workspace();
        assert!(!workspace.is_read(&path));
        assert!(workspace.file(&path).source(&session).is_some());
        assert!(workspace.is_read(&path));

        workspace.forget(spelled);
        assert!(!workspace.is_read(&path), "its record is stale");
    }
}
