//! Luau's own type checker over the open documents and the files they
//! require.
//!
//! The session keeps one checker for each security level and list of
//! definitions its projects load, as `luck check` does, behind a lock. A job
//! that needs types holds the lock while it brings the checker up to the open
//! documents as they stood when the job was spawned, checks what it asks
//! about, and reads the answer. Analysis keeps every module it checked, so a
//! check reads only the modules whose text changed and the modules that
//! require them.
//!
//! Every check of a generation, the open documents and the files they read as
//! they stand between two changes, takes its token, which the main loop
//! cancels once the next change starts a new one. So a job never brings a
//! checker back to older text, and a check of text that changed since stops.

use std::borrow::Cow;
use std::cell::RefCell;
use std::iter;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ls_types::notification::{Notification as _, ShowMessage};
use ls_types::{Location, MessageType, ShowMessageParams, Uri};
use lsp_server::Notification;
use luck_config::{DefinitionSource, Project};
use luck_diagnostics::Diagnostic;
use luck_resolver::{FileSystem as _, FileSystemOs, Resolver, to_slash_lossy};
use luck_stdlib::{SecurityLevel, luau_definitions, roblox_definitions};
use luck_type_checker::{
    CancellationToken, CheckError, Checker, Completion, Config, Declaration, Description, Host,
};
use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::definitions::{Package, luau_uri, roblox_uri};
use crate::document::{Document, DocumentState};
use crate::line_index::PositionEncoding;
use crate::server::Task;
use crate::session::Session;

/// The open documents' texts by module name, which a check reads in place of
/// the disk.
pub(crate) type OpenModules = FxHashMap<String, Arc<str>>;

/// The checkers of the session's projects.
#[derive(Debug, Default)]
pub(crate) struct TypeChecks {
    checks: Mutex<Vec<Arc<TypeCheck>>>,
}

impl TypeChecks {
    /// The checker of the files of `project`.
    pub(crate) fn of(&self, project: &Project) -> Arc<TypeCheck> {
        let (security, definitions) = (project.security(), project.definitions());
        let mut checks = self.checks.lock();
        let found = checks
            .iter()
            .find(|check| check.security == security && check.definitions == definitions);
        if let Some(check) = found {
            return Arc::clone(check);
        }
        let check = Arc::new(TypeCheck::new(security, definitions.to_vec()));
        checks.push(Arc::clone(&check));
        check
    }

    /// Makes every checker read the module at `path`, as the client spells
    /// it, again, or every module, after a change that can change how they
    /// resolve.
    pub(crate) fn changed(&self, path: Option<&Path>) {
        for check in self.checks.lock().iter() {
            let mut changed = check.changed.lock();
            match path {
                Some(module) => changed.modules.push(module.to_path_buf()),
                None => changed.is_everything = true,
            }
        }
    }

    /// Drops every checker, as new settings need, since they decide the
    /// definitions each project loads.
    pub(crate) fn clear(&self) {
        self.checks.lock().clear();
    }
}

/// One checker and the definitions it loads.
#[derive(Debug)]
pub(crate) struct TypeCheck {
    security: SecurityLevel,
    definitions: Vec<DefinitionSource>,
    /// Luau's own definitions, then the project's, in the order it loads
    /// them.
    packages: Vec<Package>,
    /// Loaded the first time a job needs it, since that takes a while.
    checker: Mutex<Option<Checker<ModuleHost>>>,
    /// What changed on disk since the last check.
    changed: Mutex<Changed>,
}

#[derive(Debug, Default)]
struct Changed {
    /// As the client spells them.
    modules: Vec<PathBuf>,
    /// A module was created or deleted, or a `.luaurc` changed, so any
    /// module may resolve a require or take its mode differently.
    is_everything: bool,
}

impl TypeCheck {
    fn new(security: SecurityLevel, definitions: Vec<DefinitionSource>) -> Self {
        let luau =
            Package::new("@luau".to_owned(), Some(luau_uri()), Cow::Borrowed(luau_definitions()));
        let packages = iter::once(luau)
            .chain(definitions.iter().map(|source| match source {
                DefinitionSource::Roblox => Package::new(
                    "@roblox".to_owned(),
                    Some(roblox_uri(security)),
                    roblox_definitions(security),
                ),
                DefinitionSource::File { path, text } => Package::new(
                    module_name(path),
                    Uri::from_file_path(path),
                    Cow::Owned(text.clone()),
                ),
            }))
            .collect();
        Self {
            security,
            definitions,
            packages,
            checker: Mutex::default(),
            changed: Mutex::default(),
        }
    }

    /// A checker with the project's definitions loaded, which shows why one
    /// doesn't load.
    fn load(&self, session: &Session) -> Checker<ModuleHost> {
        let mut checker = Checker::new(ModuleHost::default());
        checker.retain_type_graphs();
        for package in &self.packages[1..] {
            if let Err(diags) = checker.load_definitions(package.name(), package.text()) {
                let problems: Vec<&str> = diags.iter().map(|diag| &*diag.message).collect();
                let message = format!(
                    "luck: Luau can't load the definition file `{}`, so its declarations are left out: {}",
                    package.name(),
                    problems.join("; ")
                );
                let shown = ShowMessageParams { typ: MessageType::ERROR, message };
                session
                    .send(Task::Notify(Notification::new(ShowMessage::METHOD.to_owned(), shown)));
            }
        }
        if self.definitions.contains(&DefinitionSource::Roblox) {
            // What Roblox Studio types past its definitions: the class a
            // string names, and the `Enum.Material` its definitions call
            // `EnumMaterial`.
            checker.return_named_type("Instance", "new");
            checker.return_named_type("ServiceProvider", "GetService");
            checker.return_named_type("ServiceProvider", "FindService");
            checker.bind_type_namespace("Enum", "EnumItem");
        }
        checker
    }

    /// What `read` gives once `modules` are checked as `open` holds the open
    /// documents, with each one's diagnostics, or `None` once `cancellation`
    /// is cancelled.
    fn with_checked<R>(
        &self,
        session: &Session,
        open: OpenModules,
        modules: &[String],
        cancellation: &CancellationToken,
        read: impl FnOnce(&mut Checker<ModuleHost>, Vec<Vec<Diagnostic>>) -> R,
    ) -> Option<R> {
        let mut guard = self.checker.lock();
        if cancellation.is_cancelled() {
            return None;
        }
        let checker = guard.get_or_insert_with(|| self.load(session));
        let changed = mem::take(&mut *self.changed.lock());
        let changed_modules: FxHashSet<String> = changed
            .modules
            .iter()
            .map(|path| module_name(&FileSystemOs.canonicalize_existing(path)))
            .collect();
        sync(checker, open, &changed_modules, changed.is_everything);
        let diags = match checker.check(modules, session.threads(), cancellation) {
            Ok(diags) => diags,
            Err(CheckError::Cancelled) => return None,
            Err(CheckError::Internal { module, diag }) => modules
                .iter()
                .map(|name| if *name == module { vec![diag.clone()] } else { Vec::new() })
                .collect(),
        };
        Some(read(checker, diags))
    }
}

/// What a request reads of the types of the module it's about.
pub(crate) struct Types<'check> {
    checker: &'check mut Checker<ModuleHost>,
    packages: &'check [Package],
    module: &'check str,
    encoding: PositionEncoding,
}

impl Types<'_> {
    /// What's at `offset` in the module.
    pub(crate) fn describe(&mut self, offset: u32) -> Option<Description> {
        self.checker.describe(self.module, offset)
    }

    /// Luau's autocomplete at `offset` in the module.
    pub(crate) fn autocomplete(&mut self, offset: u32) -> Vec<Completion> {
        self.checker.autocomplete(self.module, offset)
    }

    /// The comment above the declaration in a definition file that
    /// `declaration` refers to.
    pub(crate) fn documentation(&self, declaration: &Declaration) -> Option<String> {
        Package::find(self.packages, declaration, self.encoding)?.documentation()
    }

    /// Where a definition file declares what `declaration` refers to.
    pub(crate) fn location(&self, declaration: &Declaration) -> Option<Location> {
        Package::find(self.packages, declaration, self.encoding)?.location()
    }
}

/// What `read` gives for the types of `document`, the version of the open
/// document at `uri` that a job reads, once Luau has checked it with the
/// other open documents as `open` holds them, or `None` once `cancellation`
/// is cancelled.
pub(crate) fn with_types<R>(
    session: &Session,
    uri: &Uri,
    document: &DocumentState,
    mut open: OpenModules,
    cancellation: &CancellationToken,
    read: impl FnOnce(&mut Types<'_>) -> R,
) -> Option<R> {
    let settings = document.settings();
    let module = document_module(uri, settings.path.as_deref());
    open.insert(module.clone(), Arc::clone(document.shared_text()));
    let check = session.type_checks().of(settings.project.project());
    let modules = [module];
    check.with_checked(session, open, &modules, cancellation, |checker, _| {
        let (packages, encoding) = (&check.packages, session.encoding());
        read(&mut Types { checker, packages, module: &modules[0], encoding })
    })
}

/// The type errors of each of `documents`, the open documents a job reads,
/// in order, once Luau has checked them together, or `None` once
/// `cancellation` is cancelled. A document the project leaves out has none.
pub(crate) fn type_errors(
    session: &Session,
    documents: &[(Uri, Arc<Document>)],
    cancellation: &CancellationToken,
) -> Option<Vec<Vec<Diagnostic>>> {
    let open = open_modules(session, documents);
    let mut groups: Vec<(Arc<TypeCheck>, Vec<usize>, Vec<String>)> = Vec::new();
    for (index, (uri, document)) in documents.iter().enumerate() {
        let settings = document.state(session).settings();
        if !settings.is_included {
            continue;
        }
        let check = session.type_checks().of(settings.project.project());
        let module = document_module(uri, settings.path.as_deref());
        match groups.iter_mut().find(|(known, ..)| Arc::ptr_eq(known, &check)) {
            Some((_, indices, modules)) => {
                indices.push(index);
                modules.push(module);
            }
            None => groups.push((check, vec![index], vec![module])),
        }
    }
    let mut errors = vec![Vec::new(); documents.len()];
    for (check, indices, modules) in groups {
        let checked =
            check.with_checked(session, open.clone(), &modules, cancellation, |_, diags| diags)?;
        for (index, diags) in indices.into_iter().zip(checked) {
            errors[index] = diags;
        }
    }
    Some(errors)
}

/// The text of each of `documents` by its module's name.
pub(crate) fn open_modules(session: &Session, documents: &[(Uri, Arc<Document>)]) -> OpenModules {
    documents
        .iter()
        .map(|(uri, document)| {
            (document_module(uri, document.path(session)), Arc::clone(document.text()))
        })
        .collect()
}

/// The name a checker gives the module at `path`, a canonical path.
fn module_name(path: &Path) -> String {
    to_slash_lossy(path).into_owned()
}

/// The module of the open document at `uri`, whose file is at `path`, a
/// canonical path, or which goes by its URI when it has none.
fn document_module(uri: &Uri, path: Option<&Path>) -> String {
    path.map_or_else(|| uri.as_str().to_owned(), module_name)
}

/// Brings `checker` to `open` and to the modules `changed` on disk, or to
/// every file when `is_everything`, invalidating each module it last read
/// otherwise.
fn sync(
    checker: &mut Checker<ModuleHost>,
    open: OpenModules,
    changed: &FxHashSet<String>,
    is_everything: bool,
) {
    let host = checker.host_mut();
    if is_everything {
        host.resolver.clear_cache();
        host.luaurcs.get_mut().clear();
    }
    let stale: Vec<String> = host
        .read
        .get_mut()
        .iter()
        .filter(|(module, read)| {
            let is_same = match (read, open.get(*module)) {
                (Some(last), Some(now)) => Arc::ptr_eq(last, now),
                (None, None) => !changed.contains(*module),
                (Some(_), None) | (None, Some(_)) => false,
            };
            is_everything || !is_same
        })
        .map(|(module, _)| module.clone())
        .collect();
    host.open = open;
    for module in stale {
        checker.invalidate(&module);
    }
}

/// The open documents and the files on disk, as a checker reads them, with
/// `require` resolved as `luck build` resolves it.
#[derive(Debug, Default)]
struct ModuleHost {
    open: OpenModules,
    /// The text each module was last read with: an open document's, or
    /// `None` for the file's.
    read: RefCell<FxHashMap<String, Option<Arc<str>>>>,
    resolver: Resolver,
    /// Each directory's `.luaurc`, if it has one, as read the first time a
    /// module below it asked.
    luaurcs: RefCell<FxHashMap<PathBuf, Option<String>>>,
}

impl Host for ModuleHost {
    fn source(&self, module: &str) -> Option<Vec<u8>> {
        if let Some(text) = self.open.get(module) {
            self.read.borrow_mut().insert(module.to_owned(), Some(Arc::clone(text)));
            return Some(text.as_bytes().to_vec());
        }
        let source = FileSystemOs.read(Path::new(module)).ok()?;
        self.read.borrow_mut().insert(module.to_owned(), None);
        Some(source)
    }

    fn resolve(&self, requirer: &str, path: &str) -> Option<String> {
        let requirer = Path::new(requirer);
        if !requirer.is_absolute() {
            return None;
        }
        Some(module_name(&self.resolver.resolve(requirer, path).ok()?.path))
    }

    fn config(&self, module: &str, config: &mut Config<'_>) {
        let path = Path::new(module);
        if !path.is_absolute() {
            return;
        }
        let directories: Vec<&Path> = path.ancestors().skip(1).collect();
        let mut luaurcs = self.luaurcs.borrow_mut();
        for &directory in directories.iter().rev() {
            let luaurc = luaurcs.entry(directory.to_path_buf()).or_insert_with(|| {
                String::from_utf8(FileSystemOs.read(&directory.join(".luaurc")).ok()?).ok()
            });
            // Luau applies the keys before one it can't read.
            if let Some(text) = luaurc {
                let _ = config.apply_luaurc(text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{document, session, uri};

    #[test]
    fn a_check_a_change_cancelled_reads_nothing() {
        let session = session();
        let document = document("local count: number = \"one\"\n");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let read =
            with_types(&session, &uri(), &document, OpenModules::default(), &cancellation, |_| ());

        assert_eq!(read, None);
    }
}
