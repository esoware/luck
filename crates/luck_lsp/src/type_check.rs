//! Luau's own type checker over the open documents and the files they
//! require.
//!
//! The session keeps one checker for each security level and list of
//! definitions, texts included, that its projects load, as `luck check` does,
//! which every project that loads them shares, behind a lock. A job that
//! needs types holds the lock while it brings the checker up to the open
//! documents as they stood when the job was spawned, checks what it asks
//! about, and reads the answer. Analysis keeps every module it checked, so a
//! check reads only the modules whose text changed and the modules that
//! require them.
//!
//! Every check of a generation, the open documents and the files they read as
//! they stand between two changes, takes its token, which the main loop
//! cancels once the next change starts a new one. So a job never brings a
//! checker back to older text, and a check of text that changed since stops.
//!
//! A definition file is no module, so it's never checked as one. A project
//! reads an open one as its document holds it, so an edit to it gives the
//! projects that list it other definitions, and so another checker, since
//! Luau can't unload a definition. A checker loads its definitions the first
//! time a job needs it, one at a time, so a job a change stops leaves the
//! ones it loaded to the next, and one that no project holds is freed by the
//! next check of every open document, on its worker.

use std::borrow::Cow;
use std::cell::RefCell;
use std::mem;
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::Arc;

use ls_types::{Location, Uri};
use luck_config::{DefinitionSource, Project};
use luck_diagnostics::Diagnostic;
use luck_resolver::{FileSystem as _, ResolverGeneric, to_slash_lossy};
use luck_stdlib::SecurityLevel;
use luck_type_checker::{
    CancellationToken, CheckError, Checker, Completion, Config, Declaration, Description,
    DirectoryConfigs, Host,
};
use parking_lot::{MappedMutexGuard, Mutex, MutexGuard};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::definitions::{Package, luau_package, roblox_package};
use crate::document::{Document, DocumentState};
use crate::files::Files;
use crate::line_index::PositionEncoding;
use crate::providers::diagnostics;
use crate::server::Problem;
use crate::session::Session;

/// The open documents' texts by module name, which a check reads in place of
/// the files.
type OpenModules = FxHashMap<String, Arc<str>>;

/// The checkers of the session's projects.
#[derive(Debug, Default)]
pub(crate) struct TypeChecks {
    checks: Mutex<Vec<Arc<TypeCheck>>>,
}

impl TypeChecks {
    /// The checker of the files of `project`, which another project that
    /// loads the same definitions shares.
    pub(crate) fn of(&self, project: &Project) -> Arc<TypeCheck> {
        let (security, definitions) = (project.security(), project.definitions());
        let mut checks = self.checks.lock();
        let found = checks
            .iter()
            .find(|check| check.security == security && check.definitions == definitions);
        if let Some(check) = found {
            return Arc::clone(check);
        }
        let check = Arc::new(TypeCheck {
            security,
            definitions: definitions.to_vec(),
            loaded: Mutex::default(),
            changed: Mutex::default(),
        });
        checks.push(Arc::clone(&check));
        check
    }

    /// Makes every checker read again what `changed` says changed in the
    /// files.
    pub(crate) fn changed(&self, changed: &FileChanges) {
        for check in self.checks.lock().iter() {
            let mut pending = check.changed.lock();
            pending.modules.extend(changed.modules.iter().cloned());
            pending.is_module_added_or_removed |= changed.is_module_added_or_removed;
            pending.is_config_changed |= changed.is_config_changed;
        }
    }

    /// Frees, on this thread, each checker no project holds.
    pub(crate) fn drop_unused(&self) {
        let unused: Vec<Arc<TypeCheck>> =
            self.checks.lock().extract_if(.., |check| Arc::strong_count(check) == 1).collect();
        drop(unused);
    }
}

/// What changed in the files, which every checker reads again before its
/// next check.
#[derive(Debug, Default)]
pub(crate) struct FileChanges {
    /// Each module whose file changed, as the client spells it.
    pub modules: Vec<PathBuf>,
    /// Whether a module was created or deleted, which can change what a
    /// require resolves to.
    pub is_module_added_or_removed: bool,
    /// Whether a `.luaurc` or `.config.luau` changed, which can change any
    /// module's mode and what any require resolves to.
    pub is_config_changed: bool,
}

/// One checker and the definitions it loads.
#[derive(Debug)]
pub(crate) struct TypeCheck {
    security: SecurityLevel,
    definitions: Vec<DefinitionSource>,
    /// Made the first time a job needs it, since loading Roblox's API takes
    /// a while.
    loaded: Mutex<Option<Loaded>>,
    /// What changed in the files since the last check.
    changed: Mutex<FileChanges>,
}

#[derive(Debug)]
struct Loaded {
    checker: Checker<ModuleHost>,
    /// Luau's own definitions, then each of the checker's it has loaded so
    /// far, in order.
    packages: Vec<Arc<Package>>,
    /// What Luau found wrong with each definition file it couldn't load.
    rejected: Vec<(Arc<Package>, Vec<Diagnostic>)>,
}

impl TypeCheck {
    /// The checker, with every definition loaded, or `None` once
    /// `cancellation` is cancelled before they are.
    fn loaded(
        &self,
        session: &Session,
        cancellation: &CancellationToken,
    ) -> Option<MappedMutexGuard<'_, Loaded>> {
        let mut guard = self.loaded.lock();
        let loaded = guard.get_or_insert_with(|| {
            let host = ModuleHost::new(session.files().clone(), Arc::clone(session.resolver()));
            let mut checker = Checker::new(host);
            checker.retain_type_graphs();
            Loaded { checker, packages: vec![luau_package()], rejected: Vec::new() }
        });
        for source in &self.definitions[loaded.packages.len() - 1..] {
            if cancellation.is_cancelled() {
                return None;
            }
            let package = match source {
                DefinitionSource::Roblox => roblox_package(self.security),
                DefinitionSource::File { path, text } => Arc::new(Package::new(
                    module_name(path),
                    session.files().uri(path),
                    Cow::Owned(text.clone()),
                )),
            };
            if let Err(diags) = loaded.checker.load_definitions(package.name(), package.text()) {
                loaded.rejected.push((Arc::clone(&package), diags));
            }
            loaded.packages.push(package);
            if loaded.packages.len() > self.definitions.len()
                && self.definitions.contains(&DefinitionSource::Roblox)
            {
                loaded.checker.type_as_roblox_studio();
            }
        }
        MutexGuard::try_map(guard, Option::as_mut).ok()
    }

    /// What `read` gives once the checker checks `modules` as `open` holds
    /// the open documents, with each one's diagnostics, or `None` once
    /// `cancellation` is cancelled.
    fn with_checked<R>(
        &self,
        session: &Session,
        open: OpenModules,
        modules: &[String],
        cancellation: &CancellationToken,
        read: impl FnOnce(&mut Loaded, Vec<Vec<Diagnostic>>) -> R,
    ) -> Option<R> {
        let mut loaded = self.loaded(session, cancellation)?;
        if cancellation.is_cancelled() {
            return None;
        }
        let changed = mem::take(&mut *self.changed.lock());
        let changed_modules: FxHashSet<String> = changed
            .modules
            .iter()
            .map(|path| module_name(&session.files().canonicalize_existing(path)))
            .collect();
        sync(&mut loaded.checker, open, &changed_modules, &changed);
        let diags = match loaded.checker.check(modules, session.threads(), cancellation) {
            Ok(diags) => diags,
            Err(CheckError::Cancelled) => return None,
            Err(CheckError::Internal { module, diag }) => modules
                .iter()
                .map(|name| if *name == module { vec![diag.clone()] } else { Vec::new() })
                .collect(),
        };
        Some(read(&mut loaded, diags))
    }
}

/// What a request reads of the types of the module it's about.
pub(crate) struct Types<'check> {
    checker: &'check mut Checker<ModuleHost>,
    packages: &'check [Arc<Package>],
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
        Package::find(self.packages, declaration)?.documentation()
    }

    /// Where a definition file declares what `declaration` refers to.
    pub(crate) fn location(&self, declaration: &Declaration) -> Option<Location> {
        Package::find(self.packages, declaration)?.location(self.encoding)
    }
}

/// What `read` gives for the types of `document`, the version of the open
/// document at `uri` that a job reads, once Luau has checked it with the
/// other open `documents`, or `None` once `cancellation` is cancelled. A
/// definition file, which is no module, has no types to read.
pub(crate) fn with_types<R>(
    session: &Session,
    uri: &Uri,
    document: &DocumentState,
    documents: &[(Uri, Arc<Document>)],
    cancellation: &CancellationToken,
    read: impl FnOnce(&mut Types<'_>) -> R,
) -> Option<R> {
    let settings = document.settings();
    if settings.is_definitions {
        return None;
    }
    let module = document_module(uri, settings.path.as_deref());
    let mut open = open_modules(session, documents);
    open.insert(module.clone(), Arc::clone(document.shared_text()));
    let check = settings.project.type_check(session.type_checks());
    let modules = [module];
    check.with_checked(session, open, &modules, cancellation, |loaded, _| {
        read(&mut Types {
            checker: &mut loaded.checker,
            packages: &loaded.packages,
            module: &modules[0],
            encoding: session.encoding(),
        })
    })
}

/// What Luau finds checking every open document together.
#[derive(Debug)]
pub(crate) struct TypeErrors {
    /// Each document's, in order: a module's type errors, what Luau found
    /// wrong with a definition file as it loaded it, or the E014 of a
    /// `.luaurc` or `.config.luau` it couldn't read. A document the project
    /// leaves out has no type errors.
    pub documents: Vec<Vec<Diagnostic>>,
    /// Why Luau can't load each definition file, or read each config, that
    /// no document holds open.
    pub problems: Vec<Problem>,
}

/// What Luau finds checking `documents`, the open documents a job reads,
/// together, or `None` once `cancellation` is cancelled.
pub(crate) fn type_errors(
    session: &Session,
    documents: &[(Uri, Arc<Document>)],
    cancellation: &CancellationToken,
) -> Option<TypeErrors> {
    let open = open_modules(session, documents);
    let mut groups: Vec<CheckGroup> = Vec::new();
    let mut definition_files = Vec::new();
    for (index, (uri, document)) in documents.iter().enumerate() {
        let settings = document.settings(session);
        if !settings.is_included {
            continue;
        }
        let check = settings.project.type_check(session.type_checks());
        let found = groups.iter().position(|group| Arc::ptr_eq(&group.check, check));
        let group = found.unwrap_or_else(|| {
            groups.push(CheckGroup { check: Arc::clone(check), modules: Vec::new() });
            groups.len() - 1
        });
        let module = document_module(uri, settings.path.as_deref());
        if settings.is_definitions {
            definition_files.push((index, module));
        } else {
            groups[group].modules.push((index, module));
        }
    }
    let mut errors = vec![Vec::new(); documents.len()];
    let mut rejected: Vec<(Arc<Package>, Vec<Diagnostic>)> = Vec::new();
    let mut config_errors: Vec<(PathBuf, Diagnostic)> = Vec::new();
    for CheckGroup { check, modules } in groups {
        let (indices, names): (Vec<usize>, Vec<String>) = modules.into_iter().unzip();
        let checked =
            check.with_checked(session, open.clone(), &names, cancellation, |loaded, diags| {
                for (package, problems) in &loaded.rejected {
                    if rejected.iter().all(|(known, _)| known.name() != package.name()) {
                        rejected.push((Arc::clone(package), problems.clone()));
                    }
                }
                for (path, diag) in loaded.checker.host_mut().config_errors.get_mut().iter() {
                    if config_errors.iter().all(|(known, _)| known != path) {
                        config_errors.push((path.clone(), diag.clone()));
                    }
                }
                diags
            })?;
        for (index, diags) in indices.into_iter().zip(checked) {
            errors[index] = diags;
        }
    }
    let mut problems = Vec::new();
    for (package, diags) in rejected {
        if let Some((index, _)) = definition_files.iter().find(|(_, name)| name == package.name()) {
            errors[*index] = diags;
        } else if !open.contains_key(package.name()) {
            let messages: Vec<&str> = diags.iter().map(|diag| &*diag.message).collect();
            let message = format!(
                "luck: Luau can't load the definition file `{}`, so its declarations are left \
                 out: {}",
                package.name(),
                messages.join("; ")
            );
            let published = package.uri().map(|uri| {
                let text = package.text();
                (uri.clone(), diagnostics::config_to_lsp(uri, &diags, text, session.encoding()))
            });
            problems.push(Problem { message, published });
        }
    }
    for (path, diag) in config_errors {
        let open_config =
            documents.iter().position(|(_, document)| document.path(session) == Some(&*path));
        if let Some(index) = open_config {
            errors[index].push(diag);
            continue;
        }
        let message = format!(
            "luck: Luau can't read `{}`, so it checks the modules below it without all of its \
             settings: {}",
            module_name(&path),
            diag.message
        );
        let published = session.files().uri(&path).map(|uri| {
            let bytes = session.files().read(&path).unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes);
            let diags =
                diagnostics::config_to_lsp(&uri, slice::from_ref(&diag), &text, session.encoding());
            (uri, diags)
        });
        problems.push(Problem { message, published });
    }
    Some(TypeErrors { documents: errors, problems })
}

/// The open documents one checker checks together.
struct CheckGroup {
    check: Arc<TypeCheck>,
    /// Each module's document, by its index, and the module's name.
    modules: Vec<(usize, String)>,
}

/// The text of each of `documents` by its module's name.
fn open_modules(session: &Session, documents: &[(Uri, Arc<Document>)]) -> OpenModules {
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

/// Brings `checker` to `open` and to `changed`, whose modules are
/// `changed_modules` by name, invalidating each module that reads otherwise
/// now: one whose text changed, or every one after a config changed, or one
/// with a require that resolves otherwise after a module was created or
/// deleted.
fn sync(
    checker: &mut Checker<ModuleHost>,
    open: OpenModules,
    changed_modules: &FxHashSet<String>,
    changed: &FileChanges,
) {
    let host = checker.host_mut();
    if changed.is_config_changed {
        host.configs.clear();
        host.config_errors.get_mut().clear();
    }
    let mut stale: Vec<String> = host
        .read
        .get_mut()
        .iter()
        .filter(|(module, read)| {
            let is_same = match (read, open.get(*module)) {
                (Some(last), Some(now)) => Arc::ptr_eq(last, now),
                (None, None) => !changed_modules.contains(*module),
                (Some(_), None) | (None, Some(_)) => false,
            };
            changed.is_config_changed || !is_same
        })
        .map(|(module, _)| module.clone())
        .collect();
    if changed.is_module_added_or_removed && !changed.is_config_changed {
        let resolver = &host.resolver;
        let resolves_otherwise = host
            .resolved
            .get_mut()
            .iter()
            .filter(|((requirer, path), last)| resolve(resolver, requirer, path) != **last);
        stale.extend(resolves_otherwise.map(|((requirer, _), _)| requirer.clone()));
    }
    host.open = open;
    for module in stale {
        checker.invalidate(&module);
    }
}

/// The open documents and the files, as a checker reads them, with `require`
/// resolved as `luck build` resolves it.
#[derive(Debug)]
struct ModuleHost {
    files: Files,
    resolver: Arc<ResolverGeneric<Files>>,
    open: OpenModules,
    /// The text each module was last read with: an open document's, or
    /// `None` for the file's.
    read: RefCell<FxHashMap<String, Option<Arc<str>>>>,
    /// What each require a check resolved named, by the module that requires
    /// it and its path.
    resolved: RefCell<FxHashMap<(String, String), Option<String>>>,
    configs: DirectoryConfigs,
    /// The E014 of each config Luau couldn't read, by its path, which
    /// `configs` gives only the first time it reads one.
    config_errors: RefCell<Vec<(PathBuf, Diagnostic)>>,
}

impl ModuleHost {
    fn new(files: Files, resolver: Arc<ResolverGeneric<Files>>) -> Self {
        Self {
            files,
            resolver,
            open: OpenModules::default(),
            read: RefCell::default(),
            resolved: RefCell::default(),
            configs: DirectoryConfigs::default(),
            config_errors: RefCell::default(),
        }
    }
}

impl Host for ModuleHost {
    fn source(&self, module: &str) -> Option<Cow<'_, [u8]>> {
        if let Some(text) = self.open.get(module) {
            self.read.borrow_mut().insert(module.to_owned(), Some(Arc::clone(text)));
            return Some(Cow::Borrowed(text.as_bytes()));
        }
        let source = self.files.read(Path::new(module)).ok()?;
        self.read.borrow_mut().insert(module.to_owned(), None);
        Some(Cow::Owned(source))
    }

    fn resolve(&self, requirer: &str, path: &str) -> Option<String> {
        let resolved = resolve(&self.resolver, requirer, path);
        let key = (requirer.to_owned(), path.to_owned());
        self.resolved.borrow_mut().insert(key, resolved.clone());
        resolved
    }

    fn config(&self, module: &str, config: &mut Config<'_>) {
        let path = Path::new(module);
        if !path.has_root() {
            return;
        }
        let read = |file: &Path| String::from_utf8(self.files.read(file).ok()?).ok();
        let errors = self.configs.apply(path, config, read);
        let mut known = self.config_errors.borrow_mut();
        for (config_path, diag) in errors {
            if known.iter().all(|(known_path, _)| *known_path != config_path) {
                known.push((config_path, diag));
            }
        }
    }
}

/// The module that `require(path)` in the module `requirer` names, which a
/// document with no file can't require from.
fn resolve(resolver: &ResolverGeneric<Files>, requirer: &str, path: &str) -> Option<String> {
    let requirer = Path::new(requirer);
    if !requirer.has_root() {
        return None;
    }
    Some(module_name(&resolver.resolve(requirer, path).ok()?.path))
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;
    use std::str::FromStr as _;
    use std::time::{Duration, Instant};

    use luck_resolver::{FileSystemOs, lexically_normalized};
    use luck_type_checker::luau_config_aliases;

    use super::*;
    use crate::files::ClientFiles;
    use crate::fixtures::{document, session, uri};

    /// A checker of the files `client` holds, and what pushes a file there,
    /// which gives the module's name.
    fn client_checker() -> (Checker<ModuleHost>, impl Fn(&str, Option<&str>) -> String) {
        let client = Arc::new(ClientFiles::default());
        let files = Files::new(Some(Arc::clone(&client)));
        let resolver = ResolverGeneric::new_with_file_system(files.clone())
            .with_luau_config(luau_config_aliases);
        let checker = Checker::new(ModuleHost::new(files, Arc::new(resolver)));
        let push = move |name: &str, text: Option<&str>| {
            let path = lexically_normalized(Path::new(&format!("/inmemory/model/{name}")));
            let uri = Uri::from_str(&format!("inmemory://model/{name}")).expect("a valid URI");
            client.push(&path, uri, text.map(Arc::from));
            module_name(&path)
        };
        (checker, push)
    }

    #[test]
    fn a_config_luau_runs_once_for_every_module_below_it_until_the_files_change() {
        let (mut checker, push) = client_checker();
        let slow_nocheck = "\
local started = os.clock()
while os.clock() - started < 0.5 do end
return { luau = { languagemode = \"nocheck\" } }
";
        push(".config.luau", Some(slow_nocheck));
        let mismatch = "\
local count: number = \"one\"
return count
";
        let modules = [
            push("a.luau", Some(mismatch)),
            push("src/b.luau", Some(mismatch)),
            push("src/c.luau", Some(mismatch)),
        ];
        let (threads, cancellation) = (NonZero::<usize>::MIN, CancellationToken::new());

        let started = Instant::now();
        let first = checker.check(&modules, threads, &cancellation).expect("nothing cancels it");
        for module in &modules {
            checker.invalidate(module);
        }
        let second = checker.check(&modules, threads, &cancellation).expect("nothing cancels it");
        let elapsed = started.elapsed();

        // Analysis asks for each module's config twice a check, so running
        // the config each time would take half a second twelve times.
        assert!(elapsed < Duration::from_secs(2), "it took {elapsed:?}");
        assert!(first.iter().chain(&second).all(Vec::is_empty), "{first:?} {second:?}");

        push(".config.luau", Some("return { luau = { languagemode = \"strict\" } }\n"));
        let changed = FileChanges { is_config_changed: true, ..FileChanges::default() };
        sync(&mut checker, OpenModules::default(), &FxHashSet::default(), &changed);
        let strict = checker.check(&modules, threads, &cancellation).expect("nothing cancels it");

        assert!(strict.iter().all(|diags| diags.len() == 1), "{strict:?}");
    }

    #[test]
    fn a_created_module_checks_again_only_what_requires_it() {
        let (mut checker, push) = client_checker();
        let requirer = push(
            "a.luau",
            Some(
                "\
--!strict
local late: number = require(\"./late\")
return late
",
            ),
        );
        let bystander = push("b.luau", Some("return 1\n"));
        let modules = [requirer, bystander];
        let (threads, cancellation) = (NonZero::<usize>::MIN, CancellationToken::new());
        let before = checker.check(&modules, threads, &cancellation).expect("nothing cancels it");
        assert_eq!(before[0].len(), 1, "`./late` doesn't resolve yet: {before:?}");

        push("late.luau", Some("return 1\n"));
        let mismatch = "\
--!strict
local count: number = \"one\"
return count
";
        // Unless it's checked again, the bystander keeps what it last read.
        push("b.luau", Some(mismatch));
        checker.host_mut().resolver.clear_cache();
        let changed = FileChanges { is_module_added_or_removed: true, ..FileChanges::default() };
        sync(&mut checker, OpenModules::default(), &FxHashSet::default(), &changed);
        let after = checker.check(&modules, threads, &cancellation).expect("nothing cancels it");

        assert!(after.iter().all(Vec::is_empty), "{after:?}");
    }

    #[test]
    fn projects_that_load_the_same_definitions_share_a_checker_while_one_holds_it() {
        let checks = TypeChecks::default();
        let project = || Project::defaults(PathBuf::from("."), FileSystemOs);
        let held = checks.of(&project());
        assert!(Arc::ptr_eq(&held, &checks.of(&project())), "a reloaded project's is the same");

        checks.drop_unused();
        assert!(Arc::ptr_eq(&held, &checks.of(&project())), "one is held");

        drop(held);
        checks.drop_unused();
        assert!(checks.checks.lock().is_empty());
    }

    #[test]
    fn a_check_a_change_cancelled_reads_nothing() {
        let session = session();
        let document = document("local count: number = \"one\"\n");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let read = with_types(&session, &uri(), &document, &[], &cancellation, |_| ());

        assert_eq!(read, None);
    }
}
