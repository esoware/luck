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
//!
//! A definition file is no module, so it's never checked as one. A checker
//! loads each one its project lists as its open document holds it, if it has
//! one, and Luau can't unload a definition, so once that text changes, the
//! generation's job loads a new checker in place of the old one, and layers
//! the same texts over Luau's library for the documents of its projects to be
//! analyzed against, which every other job reads the old ones of until the
//! new ones are ready, and stops between definition files once the next
//! generation starts. What Luau finds wrong with a definition file as it
//! loads it is published on it, open or not.

use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ls_types::notification::{Notification as _, PublishDiagnostics, ShowMessage};
use ls_types::{Location, MessageType, PublishDiagnosticsParams, ShowMessageParams, Uri};
use lsp_server::Notification;
use luck_config::{DefinitionSource, Project};
use luck_diagnostics::Diagnostic;
use luck_resolver::{FileSystem as _, ResolverGeneric, to_slash_lossy};
use luck_stdlib::{SecurityLevel, Stdlib, luau_definitions, roblox_definitions};
use luck_type_checker::{
    CancellationToken, CheckError, Checker, Completion, Config, Declaration, Description, Host,
    SavedConfig, luau_config_aliases,
};
use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::definitions::{Package, luau_uri, roblox_uri};
use crate::document::{Document, DocumentState};
use crate::files::Files;
use crate::line_index::PositionEncoding;
use crate::providers::diagnostics;
use crate::server::Task;
use crate::session::Session;

/// The open documents' texts by module name, which a check reads in place of
/// the files.
pub(crate) type OpenModules = FxHashMap<String, Arc<str>>;

/// The checkers of the session's projects.
#[derive(Debug)]
pub(crate) struct TypeChecks {
    files: Files,
    checks: Mutex<Vec<Arc<TypeCheck>>>,
}

impl TypeChecks {
    /// Checkers that read every module through `files`.
    pub(crate) fn new(files: Files) -> Self {
        Self { files, checks: Mutex::default() }
    }

    /// The checker of the files of `project`, which the settings of
    /// `generation` gave.
    pub(crate) fn of(&self, project: &Project, generation: u64) -> Arc<TypeCheck> {
        let (security, definitions) = (project.security(), project.definitions());
        let mut checks = self.checks.lock();
        let found = checks
            .iter()
            .find(|check| check.security == security && check.definitions == definitions);
        if let Some(check) = found {
            return Arc::clone(check);
        }
        let check = Arc::new(TypeCheck::new(
            security,
            definitions.to_vec(),
            self.files.clone(),
            generation,
        ));
        checks.push(Arc::clone(&check));
        check
    }

    /// The library the documents of `project`, which the settings of
    /// `generation` gave, are analyzed against: the one its checker's
    /// definitions layer over Luau's, or the project's while they're the
    /// texts the project read.
    pub(crate) fn stdlib(&self, project: &Project, generation: u64) -> Stdlib {
        let edited = self.of(project, generation).stdlib.lock().clone();
        edited.unwrap_or_else(|| project.stdlib().clone())
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
    /// definitions each project loads, and returns each definition file Luau
    /// couldn't load, whose problems were published.
    pub(crate) fn clear(&self) -> Vec<PathBuf> {
        let checks = mem::take(&mut *self.checks.lock());
        checks
            .iter()
            .flat_map(|check| mem::take(&mut *check.rejected.lock()))
            .map(|(path, _)| path)
            .collect()
    }
}

/// One checker and the definitions it loads.
#[derive(Debug)]
pub(crate) struct TypeCheck {
    security: SecurityLevel,
    definitions: Vec<DefinitionSource>,
    files: Files,
    /// The settings' generation the project came from, which a problem of
    /// its definitions is published under.
    generation: u64,
    /// The checker every job reads, loaded the first time a job needs one,
    /// since that takes a while.
    loaded: Mutex<Option<Loaded>>,
    /// Held while a job loads a checker in place of the one in `loaded`, so
    /// only one job loads it.
    loading: Mutex<()>,
    /// What changed in the files since the last check.
    changed: Mutex<Changed>,
    /// What Luau found wrong with each definition file the checker in
    /// `loaded` couldn't load.
    rejected: Mutex<Vec<(PathBuf, Vec<Diagnostic>)>>,
    /// Luau's library with each definition layered over it as the checker in
    /// `loaded` loaded it, leaving out a file that doesn't parse, or `None`
    /// while those are the texts the project read.
    stdlib: Mutex<Option<Stdlib>>,
}

/// A checker and the definitions it loaded.
#[derive(Debug)]
struct Loaded {
    checker: Checker<ModuleHost>,
    /// Luau's own definitions, then the project's, in the order it loaded
    /// them, each open one as its document held it.
    packages: Vec<Package>,
}

#[derive(Debug, Default)]
struct Changed {
    /// As the client spells them.
    modules: Vec<PathBuf>,
    /// A module was created or deleted, or a config changed, so any module
    /// may resolve a require or take its mode differently.
    is_everything: bool,
}

impl TypeCheck {
    fn new(
        security: SecurityLevel,
        definitions: Vec<DefinitionSource>,
        files: Files,
        generation: u64,
    ) -> Self {
        Self {
            security,
            definitions,
            files,
            generation,
            loaded: Mutex::default(),
            loading: Mutex::default(),
            changed: Mutex::default(),
            rejected: Mutex::default(),
            stdlib: Mutex::default(),
        }
    }

    fn is_loaded(&self) -> bool {
        self.loaded.lock().is_some()
    }

    /// Loads a checker with the project's definitions, each open one as
    /// `open` holds it, and the library they layer, in place of the ones
    /// loaded, which every other job reads meanwhile, unless those loaded the
    /// same, or returns `None` once `cancellation` is cancelled. It publishes
    /// on each definition file why it doesn't load, and shows it, or clears
    /// that once it loads.
    fn reload(
        &self,
        session: &Session,
        open: &OpenModules,
        cancellation: &CancellationToken,
    ) -> Option<()> {
        let _loading = self.loading.lock();
        let is_current = self.loaded.lock().as_ref().is_some_and(|loaded| {
            loaded.packages[1..].iter().zip(&self.definitions).all(|(package, source)| match source
            {
                DefinitionSource::Roblox => true,
                DefinitionSource::File { path, text } => {
                    package.text() == definition_text(open, path, text)
                }
            })
        });
        if is_current {
            return Some(());
        }
        if cancellation.is_cancelled() {
            return None;
        }
        let mut checker = Checker::new(ModuleHost::new(self.files.clone()));
        checker.retain_type_graphs();
        let luau =
            Package::new("@luau".to_owned(), Some(luau_uri()), Cow::Borrowed(luau_definitions()));
        let mut packages = vec![luau];
        let mut rejected = Vec::new();
        let mut notifications = Vec::new();
        let is_edited = self.definitions.iter().any(|source| match source {
            DefinitionSource::Roblox => false,
            DefinitionSource::File { path, text } => definition_text(open, path, text) != text,
        });
        let mut stdlib = is_edited.then(Stdlib::luau);
        for source in &self.definitions {
            if cancellation.is_cancelled() {
                return None;
            }
            let package = match source {
                DefinitionSource::Roblox => Package::new(
                    "@roblox".to_owned(),
                    Some(roblox_uri(self.security)),
                    roblox_definitions(self.security),
                ),
                DefinitionSource::File { path, text } => Package::new(
                    module_name(path),
                    self.files.uri(path),
                    Cow::Owned(definition_text(open, path, text).to_owned()),
                ),
            };
            stdlib = stdlib.map(|below| match source {
                DefinitionSource::Roblox => below.with_roblox(self.security),
                DefinitionSource::File { .. } => below.layer(package.text()).unwrap_or(below),
            });
            if let Err(diags) = checker.load_definitions(package.name(), package.text()) {
                // An open definition file loads again with each edit, so its
                // problems show only on it, not in a message for each edit.
                if !open.contains_key(package.name()) {
                    let problems: Vec<&str> = diags.iter().map(|diag| &*diag.message).collect();
                    let message = format!(
                        "luck: Luau can't load the definition file `{}`, so its declarations are left out: {}",
                        package.name(),
                        problems.join("; ")
                    );
                    let shown = ShowMessageParams { typ: MessageType::ERROR, message };
                    notifications.push(Notification::new(ShowMessage::METHOD.to_owned(), shown));
                }
                if let DefinitionSource::File { path, .. } = source {
                    if let Some(uri) = package.uri() {
                        let diagnostics = diagnostics::config_to_lsp(
                            uri,
                            &diags,
                            package.text(),
                            session.encoding(),
                        );
                        let published = PublishDiagnosticsParams {
                            uri: uri.clone(),
                            diagnostics,
                            version: None,
                        };
                        let method = PublishDiagnostics::METHOD.to_owned();
                        notifications.push(Notification::new(method, published));
                    }
                    rejected.push((path.clone(), diags));
                }
            }
            packages.push(package);
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
        {
            let mut loaded = self.loaded.lock();
            if cancellation.is_cancelled() {
                return None;
            }
            *loaded = Some(Loaded { checker, packages });
            *self.stdlib.lock() = stdlib;
        }
        let loads_now: Vec<PathBuf> = {
            let mut kept = self.rejected.lock();
            let was_rejected = mem::replace(&mut *kept, rejected);
            let paths = was_rejected.into_iter().map(|(path, _)| path);
            paths
                .filter(|path| kept.iter().all(|(rejected_path, _)| rejected_path != path))
                .collect()
        };
        for uri in loads_now.iter().filter_map(|path| self.files.uri(path)) {
            let cleared = PublishDiagnosticsParams { uri, diagnostics: Vec::new(), version: None };
            notifications.push(Notification::new(PublishDiagnostics::METHOD.to_owned(), cleared));
        }
        if !notifications.is_empty() {
            session.send(Task::ConfigError { generation: self.generation, notifications });
        }
        Some(())
    }

    /// What Luau found wrong with the definition file at `path` as it loaded
    /// it, once it has.
    fn rejected(&self, path: &Path) -> Vec<Diagnostic> {
        let rejected = self.rejected.lock();
        let found = rejected.iter().find(|(rejected_path, _)| rejected_path == path);
        found.map(|(_, diags)| diags.clone()).unwrap_or_default()
    }

    /// What `read` gives once the checker loaded checks `modules` as `open`
    /// holds the open documents, with each one's diagnostics, or `None` once
    /// `cancellation` is cancelled, or while no checker is loaded.
    fn with_checked<R>(
        &self,
        session: &Session,
        open: OpenModules,
        modules: &[String],
        cancellation: &CancellationToken,
        read: impl FnOnce(&mut Loaded, Vec<Vec<Diagnostic>>) -> R,
    ) -> Option<R> {
        let mut guard = self.loaded.lock();
        if cancellation.is_cancelled() {
            return None;
        }
        let loaded = guard.as_mut()?;
        let changed = mem::take(&mut *self.changed.lock());
        let changed_modules: FxHashSet<String> = changed
            .modules
            .iter()
            .map(|path| module_name(&self.files.canonicalize_existing(path)))
            .collect();
        sync(&mut loaded.checker, open, &changed_modules, changed.is_everything);
        let diags = match loaded.checker.check(modules, session.threads(), cancellation) {
            Ok(diags) => diags,
            Err(CheckError::Cancelled) => return None,
            Err(CheckError::Internal { module, diag }) => modules
                .iter()
                .map(|name| if *name == module { vec![diag.clone()] } else { Vec::new() })
                .collect(),
        };
        Some(read(loaded, diags))
    }
}

/// The text of the definition file at `path`, which its project read as
/// `text`, as a checker loads it: its open document's, where `open` holds
/// one.
fn definition_text<'text>(open: &'text OpenModules, path: &Path, text: &'text str) -> &'text str {
    open.get(&module_name(path)).map_or(text, |open_text| open_text)
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
/// is cancelled. A definition file, which is no module, has no types to read.
///
/// It reads the checker loaded, whatever definitions it loaded, and loads
/// one only when none is, so a request never waits for a new one.
pub(crate) fn with_types<R>(
    session: &Session,
    uri: &Uri,
    document: &DocumentState,
    mut open: OpenModules,
    cancellation: &CancellationToken,
    read: impl FnOnce(&mut Types<'_>) -> R,
) -> Option<R> {
    let settings = document.settings();
    if settings.is_definitions {
        return None;
    }
    let module = document_module(uri, settings.path.as_deref());
    open.insert(module.clone(), Arc::clone(document.shared_text()));
    let check =
        session.type_checks().of(settings.project.project(), session.settings().generation());
    if !check.is_loaded() {
        check.reload(session, &open, cancellation)?;
    }
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

/// The type errors of each of `documents`, the open documents a job reads,
/// in order, once Luau has checked them together, or `None` once
/// `cancellation` is cancelled. A document the project leaves out has none,
/// and a definition file has what Luau found wrong with it as it loaded it.
///
/// A checker that loaded a definition file with other text than the open
/// documents now give it loads afresh first.
pub(crate) fn type_errors(
    session: &Session,
    documents: &[(Uri, Arc<Document>)],
    cancellation: &CancellationToken,
) -> Option<Vec<Vec<Diagnostic>>> {
    let open = open_modules(session, documents);
    let generation = session.settings().generation();
    let mut groups: Vec<CheckGroup> = Vec::new();
    for (index, (uri, document)) in documents.iter().enumerate() {
        let settings = document.state(session).settings();
        if !settings.is_included {
            continue;
        }
        let check = session.type_checks().of(settings.project.project(), generation);
        let found = groups.iter().position(|group| Arc::ptr_eq(&group.check, &check));
        let group = found.unwrap_or_else(|| {
            groups.push(CheckGroup { check, modules: Vec::new(), definitions: Vec::new() });
            groups.len() - 1
        });
        let group = &mut groups[group];
        match (&settings.path, settings.is_definitions) {
            (Some(path), true) => group.definitions.push((index, path.clone())),
            (_, true) => {}
            (path, false) => group.modules.push((index, document_module(uri, path.as_deref()))),
        }
    }
    let mut errors = vec![Vec::new(); documents.len()];
    for CheckGroup { check, modules, definitions } in groups {
        check.reload(session, &open, cancellation)?;
        let (indices, names): (Vec<usize>, Vec<String>) = modules.into_iter().unzip();
        let checked =
            check.with_checked(session, open.clone(), &names, cancellation, |_, diags| diags)?;
        for (index, diags) in indices.into_iter().zip(checked) {
            errors[index] = diags;
        }
        for (index, path) in definitions {
            errors[index] = check.rejected(&path);
        }
    }
    Some(errors)
}

/// The open documents one checker checks together.
struct CheckGroup {
    check: Arc<TypeCheck>,
    /// Each module's document, by its index, and the module's name.
    modules: Vec<(usize, String)>,
    /// Each definition file's document, by its index, and its path.
    definitions: Vec<(usize, PathBuf)>,
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

/// Brings `checker` to `open` and to the modules `changed` in the files, or
/// to every file when `is_everything`, invalidating each module it last read
/// otherwise.
fn sync(
    checker: &mut Checker<ModuleHost>,
    open: OpenModules,
    changed: &FxHashSet<String>,
    is_everything: bool,
) {
    let host = checker.host_mut();
    host.pushed = OnceCell::new();
    if is_everything {
        host.resolver.clear_cache();
        host.configs.get_mut().clear();
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

/// The open documents and the files, as a checker reads them, with `require`
/// resolved as `luck build` resolves it.
#[derive(Debug)]
struct ModuleHost {
    files: Files,
    open: OpenModules,
    /// What the client pushed of each file, taken the first time a check
    /// reads a module no open document holds, which the check lends Luau as
    /// it does a document's text.
    pushed: OnceCell<BTreeMap<PathBuf, Arc<str>>>,
    /// The text each module was last read with: an open document's, or
    /// `None` for the file's.
    read: RefCell<FxHashMap<String, Option<Arc<str>>>>,
    resolver: ResolverGeneric<Files>,
    /// The config each directory gives the modules in it, from its `.luaurc`
    /// or `.config.luau` and the ones above it, read the first time a module
    /// below it asks, as `luau-analyze` keeps them.
    configs: RefCell<FxHashMap<PathBuf, SavedConfig>>,
}

impl ModuleHost {
    fn new(files: Files) -> Self {
        Self {
            resolver: ResolverGeneric::new_with_file_system(files.clone())
                .with_luau_config(luau_config_aliases),
            files,
            open: OpenModules::default(),
            pushed: OnceCell::new(),
            read: RefCell::default(),
            configs: RefCell::default(),
        }
    }
}

impl Host for ModuleHost {
    fn source(&self, module: &str) -> Option<Cow<'_, [u8]>> {
        if let Some(text) = self.open.get(module) {
            self.read.borrow_mut().insert(module.to_owned(), Some(Arc::clone(text)));
            return Some(Cow::Borrowed(text.as_bytes()));
        }
        let path = Path::new(module);
        let pushed = self.files.client().map(|client| self.pushed.get_or_init(|| client.pushed()));
        let source = match pushed.and_then(|pushed| pushed.get(path)) {
            Some(text) => Cow::Borrowed(text.as_bytes()),
            None => Cow::Owned(self.files.read(path).ok()?),
        };
        self.read.borrow_mut().insert(module.to_owned(), None);
        Some(source)
    }

    fn resolve(&self, requirer: &str, path: &str) -> Option<String> {
        let requirer = Path::new(requirer);
        if !requirer.has_root() {
            return None;
        }
        Some(module_name(&self.resolver.resolve(requirer, path).ok()?.path))
    }

    /// As luau-analyze reads them: a directory with both a `.luaurc` and a
    /// `.config.luau` applies neither, and Luau applies the keys of a config
    /// before one it can't read.
    fn config(&self, module: &str, config: &mut Config<'_>) {
        let path = Path::new(module);
        if !path.has_root() {
            return;
        }
        let directories: Vec<&Path> = path.ancestors().skip(1).collect();
        let mut configs = self.configs.borrow_mut();
        let cached = directories.iter().position(|directory| configs.contains_key(*directory));
        if let Some(index) = cached {
            config.set(&configs[directories[index]]);
        }
        for &directory in directories[..cached.unwrap_or(directories.len())].iter().rev() {
            let read =
                |name: &str| String::from_utf8(self.files.read(&directory.join(name)).ok()?).ok();
            let _ = match (read(".luaurc"), read(".config.luau")) {
                (Some(text), None) => config.apply_luaurc(&text),
                (None, Some(source)) => config.apply_luau_config(&source),
                (Some(_), Some(_)) | (None, None) => Ok(()),
            };
            let Some(saved) = config.saved() else {
                return;
            };
            configs.insert(directory.to_path_buf(), saved);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;
    use std::str::FromStr as _;
    use std::time::{Duration, Instant};

    use luck_resolver::lexically_normalized;

    use super::*;
    use crate::files::ClientFiles;
    use crate::fixtures::{document, session, uri};

    #[test]
    fn a_config_luau_runs_once_for_every_module_below_it_until_the_files_change() {
        let client = Arc::new(ClientFiles::default());
        let push = |name: &str, text: &str| {
            let path = lexically_normalized(Path::new(&format!("/inmemory/model/{name}")));
            let uri = Uri::from_str(&format!("inmemory://model/{name}")).expect("a valid URI");
            client.push(&path, uri, Some(Arc::from(text)));
            module_name(&path)
        };
        let slow_nocheck = "\
local started = os.clock()
while os.clock() - started < 0.5 do end
return { luau = { languagemode = \"nocheck\" } }
";
        push(".config.luau", slow_nocheck);
        let mismatch = "\
local count: number = \"one\"
return count
";
        let modules =
            [push("a.luau", mismatch), push("src/b.luau", mismatch), push("src/c.luau", mismatch)];
        let mut checker = Checker::new(ModuleHost::new(Files::Client(Arc::clone(&client))));
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

        push(".config.luau", "return { luau = { languagemode = \"strict\" } }\n");
        sync(&mut checker, OpenModules::default(), &FxHashSet::default(), true);
        let strict = checker.check(&modules, threads, &cancellation).expect("nothing cancels it");

        assert!(strict.iter().all(|diags| diags.len() == 1), "{strict:?}");
    }

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
