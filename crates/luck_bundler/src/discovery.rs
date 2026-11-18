use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::any::Any;
use core::panic::AssertUnwindSafe;
use core::slice::IterMut;
use std::panic;
use std::path::{Path, PathBuf};
use std::thread::{self, Scope};

use luck_allocator::Allocator;
use luck_ast::MIN_STACK_BYTES;
use luck_core::Diagnostic;
use luck_resolver::{
    FileSystem as _, FileSystemOs, Resolution, ResolveError, Resolver, to_slash_lossy,
};
use luck_token::{CommentKind, LuaVersion, Span, leading_hot_comments};
use parking_lot::{Condvar, Mutex, MutexGuard};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dynamic_require::DynamicRequire;
use crate::graph::{BundleOptions, MAX_MODULE_COUNT};
use crate::loader::PREFIX_STEM;
use crate::module::ModuleInfo;
use crate::relative_path::make_relative;
use crate::require_extraction::{ExtractedRequires, RequireSite, extract_requires};

const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

/// The most requires one task resolves, so a module that makes more hands
/// them to every thread that frees up meanwhile.
const MAX_RESOLVE_BATCH: usize = 16;

/// Everything one module's discovery finds, which depends on no other
/// module, with its diagnostics in the order the graph reports them.
pub(crate) struct Discovered<'ast> {
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    /// `None` when the file can't be read or is too large.
    pub module: Option<ResolvedModuleInfo<'ast>>,
}

pub(crate) struct ResolvedModuleInfo<'ast> {
    /// Every field but the dependencies, which need module ids.
    pub info: ModuleInfo<'ast>,
    /// Each require and the path it resolves to, in source order. `None`
    /// when it doesn't resolve.
    pub requires: Vec<(RequireSite<'ast>, Option<PathBuf>)>,
}

/// A module parsed and analyzed, before its requires resolve.
pub(crate) struct ParsedModule<'ast> {
    /// Every field but the dependencies.
    pub info: ModuleInfo<'ast>,
    /// `info.path`, for the requests that resolve its requires.
    pub path: Arc<Path>,
    /// In source order.
    pub sites: Vec<RequireSite<'ast>>,
    /// Parse errors, then the analysis's.
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

impl<'ast> ParsedModule<'ast> {
    /// The module with each of its requires resolved, in source order.
    fn resolved(
        self,
        resolutions: impl IntoIterator<Item = Result<Resolution, ResolveError>>,
    ) -> Discovered<'ast> {
        let Self { info, path, sites, mut errors, mut warnings } = self;
        let requires = sites
            .into_iter()
            .zip(resolutions)
            .map(|(site, resolution)| match resolution {
                Ok(Resolution { path: target, is_self_alias_shadowed }) => {
                    if is_self_alias_shadowed {
                        warnings.push(Diagnostic::w004(&to_slash_lossy(&path), site.call_span));
                    }
                    (site, Some(target))
                }
                Err(error) => {
                    errors.push(resolve_error_diagnostic(&to_slash_lossy(&path), &site, &error));
                    (site, None)
                }
            })
            .collect();
        Discovered { errors, warnings, module: Some(ResolvedModuleInfo { info, requires }) }
    }
}

/// What reports `error`, which stopped `site`, a require in the module at
/// `file_path`, from resolving.
fn resolve_error_diagnostic(
    file_path: &str,
    site: &RequireSite<'_>,
    error: &ResolveError,
) -> Diagnostic {
    let (span, module) = (site.call_span, site.require_string);
    match error {
        ResolveError::NotFound(searched) => {
            let tried_paths: Vec<String> =
                searched.iter().map(|path| to_slash_lossy(path).into_owned()).collect();
            Diagnostic::e004(file_path, span, module, &tried_paths)
        }
        ResolveError::MissingPrefix => Diagnostic::e004_missing_prefix(file_path, span, module),
        ResolveError::BareSelfAlias => Diagnostic::e004_bare_self_alias(file_path, span),
        ResolveError::UnknownAlias(alias) => {
            Diagnostic::e004_unknown_alias(file_path, span, module, alias)
        }
        ResolveError::MalformedLuaurc(luaurc) => {
            Diagnostic::e004_malformed_luaurc(file_path, span, module, &to_slash_lossy(luaurc))
        }
        ResolveError::AliasCycle(cycle) => {
            Diagnostic::e004_alias_cycle(file_path, span, module, cycle)
        }
        ResolveError::BothExtensions { luau, lua } => Diagnostic::e007_both_extensions(
            file_path,
            span,
            &to_slash_lossy(luau),
            &to_slash_lossy(lua),
        ),
        ResolveError::FileAndDirectory { file, init } => Diagnostic::e007_file_and_directory(
            file_path,
            span,
            &to_slash_lossy(file),
            &to_slash_lossy(init),
        ),
        ResolveError::FileBesideDirectory { file, directory } => {
            Diagnostic::e007_file_beside_directory(
                file_path,
                span,
                &to_slash_lossy(file),
                &to_slash_lossy(directory),
            )
        }
    }
}

/// Parses `source`, the module at `path`, into `allocator` and analyzes its
/// tree.
pub(crate) fn parse_module<'ast>(
    allocator: &'ast Allocator,
    path: PathBuf,
    relative_path: String,
    source: &'ast str,
    version: LuaVersion,
    dynamic_require: DynamicRequire,
) -> ParsedModule<'ast> {
    let parsed = luck_parser::parse(allocator, source, version);
    let file_path = to_slash_lossy(&path);
    let mut errors: Vec<Diagnostic> =
        parsed.errors.iter().map(|error| Diagnostic::e008(&file_path, error)).collect();
    let mut warnings = Vec::new();
    let ExtractedRequires {
        sites,
        dynamic_callees,
        diags,
        has_vararg_read,
        has_global_arg_read,
        has_env_rebinding,
        max_function_upvalues,
        exports,
    } = extract_requires(
        allocator,
        &parsed.block,
        parsed.counts,
        source,
        &file_path,
        version,
        dynamic_require,
    );
    drop(file_path);
    for diag in diags {
        if diag.is_error() {
            errors.push(diag);
        } else {
            warnings.push(diag);
        }
    }
    let hot_comments = if version.is_luau() {
        leading_hot_comments(&parsed.comments, source).map(|comment| comment.span).collect()
    } else {
        Vec::new()
    };
    let shebang = parsed
        .comments
        .first()
        .filter(|comment| comment.kind == CommentKind::Shebang)
        .map(|comment| comment.span);
    let shared_path = Arc::from(path.as_path());
    let info = ModuleInfo {
        path,
        relative_path,
        source,
        block: parsed.block,
        nesting_depth: parsed.nesting_depth,
        dependencies: Vec::new(),
        dynamic_callees,
        hot_comments,
        shebang,
        has_vararg_read,
        has_global_arg_read,
        has_env_rebinding,
        max_function_upvalues,
        has_prefix_stem: source.contains(PREFIX_STEM),
        exports,
    };
    ParsedModule { info, path: shared_path, sites, errors, warnings }
}

/// What reading and resolving modules takes, which every thread shares.
#[derive(Clone, Copy)]
pub(crate) struct Reader<'input> {
    version: LuaVersion,
    dynamic_require: DynamicRequire,
    /// From [`absolute_project_root`](crate::relative_path::absolute_project_root).
    project_root: &'input Path,
    resolver: &'input Resolver,
}

/// Reading a module either parses it or fails with E010 or E012.
type Read<'ast> = Result<Box<ParsedModule<'ast>>, Diagnostic>;

impl<'input> Reader<'input> {
    pub(crate) const fn new(
        options: &BundleOptions<'_>,
        project_root: &'input Path,
        resolver: &'input Resolver,
    ) -> Self {
        Self {
            version: options.version,
            dynamic_require: options.dynamic_require,
            project_root,
            resolver,
        }
    }

    /// Reads and parses the module at `path` into `allocator`.
    ///
    /// # Errors
    ///
    /// Returns E010 when the file can't be read and E012 when it's too
    /// large.
    fn read<'ast>(&self, allocator: &'ast Allocator, path: &Path) -> Read<'ast> {
        let file = FileSystemOs.read_to_string(path).map_err(|io_error| {
            Diagnostic::e010(&to_slash_lossy(path), Span::default(), &io_error.to_string())
        })?;
        if file.len() > MAX_FILE_SIZE {
            return Err(Diagnostic::e012(
                &to_slash_lossy(path),
                Span::default(),
                file.len(),
                MAX_FILE_SIZE,
            ));
        }
        let source = allocator.alloc_str(&file);
        drop(file);
        Ok(Box::new(parse_module(
            allocator,
            path.to_path_buf(),
            make_relative(path, self.project_root),
            source,
            self.version,
            self.dynamic_require,
        )))
    }

    /// # Errors
    ///
    /// Returns why `require` names no module, or more than one.
    fn resolve(
        &self,
        require: &RequireSite<'_>,
        from_file: &Path,
    ) -> Result<Resolution, ResolveError> {
        self.resolver.resolve(from_file, require.require_string)
    }

    /// Reads the module at `path` and resolves its requires on this thread.
    pub(crate) fn discover<'ast>(
        &self,
        allocator: &'ast Allocator,
        path: &Path,
    ) -> Discovered<'ast> {
        match self.read(allocator, path) {
            Ok(module) => self.resolve_all(*module),
            Err(error) => Discovered { errors: vec![error], warnings: Vec::new(), module: None },
        }
    }

    fn resolve_all<'ast>(&self, module: ParsedModule<'ast>) -> Discovered<'ast> {
        let resolutions: Vec<_> =
            module.sites.iter().map(|site| self.resolve(site, &module.path)).collect();
        module.resolved(resolutions)
    }
}

/// The require at `site` in the module waiting on it in `slot`, which is in
/// `from_file`.
struct Request<'ast> {
    slot: usize,
    site: usize,
    require: RequireSite<'ast>,
    from_file: Arc<Path>,
}

enum Task<'ast> {
    Read(PathBuf),
    Resolve(Vec<Request<'ast>>),
}

enum Done<'ast> {
    Discovered(PathBuf, Box<Discovered<'ast>>),
    /// A module that makes more requires than one task resolves.
    Parsed(Box<ParsedModule<'ast>>),
    Resolved(Vec<(Request<'ast>, Result<Resolution, ResolveError>)>),
}

impl<'ast> Task<'ast> {
    fn run(self, reader: Reader<'_>, allocator: &'ast Allocator) -> Done<'ast> {
        match self {
            Self::Read(path) => match reader.read(allocator, &path) {
                Ok(module) if module.sites.len() > MAX_RESOLVE_BATCH => Done::Parsed(module),
                Ok(module) => Done::Discovered(path, Box::new(reader.resolve_all(*module))),
                Err(error) => {
                    let unreadable =
                        Discovered { errors: vec![error], warnings: Vec::new(), module: None };
                    Done::Discovered(path, Box::new(unreadable))
                }
            },
            Self::Resolve(requests) => Done::Resolved(
                requests
                    .into_iter()
                    .map(|request| {
                        let resolution = reader.resolve(&request.require, &request.from_file);
                        (request, resolution)
                    })
                    .collect(),
            ),
        }
    }
}

/// A module whose requires are resolving.
struct Resolving<'ast> {
    module: ParsedModule<'ast>,
    resolutions: Vec<Option<Result<Resolution, ResolveError>>>,
    remaining: usize,
}

/// Every module the entry reaches, read and resolved as separate tasks by one
/// thread per arena, which share this behind one lock.
struct Discovery<'ast> {
    found: FxHashMap<PathBuf, Discovered<'ast>>,
    /// Every path found, being read, or queued to be, which stops growing once
    /// more are known than a graph may hold.
    seen: FxHashSet<PathBuf>,
    reads: VecDeque<PathBuf>,
    requests: VecDeque<Request<'ast>>,
    /// By slot, `None` once resolved.
    resolving: Vec<Option<Resolving<'ast>>>,
    /// The arenas no thread has taken.
    spare_arenas: IterMut<'ast, Allocator>,
    /// Tasks a thread is running.
    running: usize,
    /// Threads waiting for a task that nothing has woken yet.
    idle: usize,
    /// What the first task to panic panicked with, which stops discovery.
    panic: Option<Box<dyn Any + Send>>,
}

struct Shared<'ast> {
    discovery: Mutex<Discovery<'ast>>,
    task_queued: Condvar,
}

/// Discovers every module `entry` reaches, by canonical path.
///
/// This thread reads the entry into `allocator`, and another thread starts
/// with one of `spare_arenas` whenever more tasks are queued than the running
/// threads can take.
pub(crate) fn discover_reachable<'ast>(
    allocator: &'ast Allocator,
    spare_arenas: &'ast mut [Allocator],
    reader: Reader<'_>,
    entry: &Path,
) -> FxHashMap<PathBuf, Discovered<'ast>> {
    let shared = Shared {
        discovery: Mutex::new(Discovery {
            found: FxHashMap::default(),
            seen: FxHashSet::from_iter([entry.to_path_buf()]),
            reads: VecDeque::from([entry.to_path_buf()]),
            requests: VecDeque::new(),
            resolving: Vec::new(),
            spare_arenas: spare_arenas.iter_mut(),
            running: 0,
            idle: 0,
            panic: None,
        }),
        task_queued: Condvar::new(),
    };
    thread::scope(|scope| work(&shared, scope, reader, allocator));
    let Discovery { found, panic: first_panic, .. } = shared.discovery.into_inner();
    if let Some(payload) = first_panic {
        panic::resume_unwind(payload);
    }
    found
}

/// Runs queued tasks, reading into `allocator`, until none is left or one
/// panics. Each task's queued successors wake waiting threads, and start a
/// thread for each one left over while an arena is spare.
#[expect(
    clippy::significant_drop_tightening,
    reason = "the lock is let go only while a task runs or a thread starts"
)]
fn work<'scope, 'ast>(
    shared: &'scope Shared<'ast>,
    scope: &'scope Scope<'scope, '_>,
    reader: Reader<'scope>,
    allocator: &'ast Allocator,
) where
    'ast: 'scope,
{
    let mut discovery = shared.discovery.lock();
    while discovery.panic.is_none() {
        let Some(task) = discovery.next_task() else {
            if discovery.running == 0 {
                shared.task_queued.notify_all();
                return;
            }
            discovery.idle = discovery.idle.saturating_add(1);
            shared.task_queued.wait(&mut discovery);
            continue;
        };
        discovery.running = discovery.running.saturating_add(1);
        let outcome = MutexGuard::unlocked(&mut discovery, || {
            panic::catch_unwind(AssertUnwindSafe(|| task.run(reader, allocator)))
        });
        discovery.running = discovery.running.saturating_sub(1);
        match outcome {
            Ok(done) => discovery.finish(done),
            Err(payload) => {
                discovery.panic = Some(payload);
                shared.task_queued.notify_all();
                return;
            }
        }
        let batches = discovery.requests.len().div_ceil(MAX_RESOLVE_BATCH);
        // This thread takes the next task itself.
        let unclaimed = discovery.reads.len().saturating_add(batches).saturating_sub(1);
        let woken = unclaimed.min(discovery.idle);
        discovery.idle = discovery.idle.saturating_sub(woken);
        for _ in 0..woken {
            shared.task_queued.notify_one();
        }
        let arenas: Vec<&'ast mut Allocator> =
            discovery.spare_arenas.by_ref().take(unclaimed.saturating_sub(woken)).collect();
        if !arenas.is_empty() {
            MutexGuard::unlocked(&mut discovery, || {
                for arena in arenas {
                    // One that can't start leaves its tasks to the threads
                    // already running.
                    let _unused = thread::Builder::new()
                        .stack_size(MIN_STACK_BYTES)
                        .spawn_scoped(scope, move || work(shared, scope, reader, arena));
                }
            });
        }
    }
}

impl<'ast> Discovery<'ast> {
    fn next_task(&mut self) -> Option<Task<'ast>> {
        if !self.requests.is_empty() {
            let count = self.requests.len().min(MAX_RESOLVE_BATCH);
            return Some(Task::Resolve(self.requests.drain(..count).collect()));
        }
        self.reads.pop_front().map(Task::Read)
    }

    fn finish(&mut self, done: Done<'ast>) {
        match done {
            Done::Discovered(path, discovered) => {
                for target in discovered
                    .module
                    .iter()
                    .flat_map(|info| &info.requires)
                    .filter_map(|(_, target)| target.as_ref())
                {
                    self.queue_read(target);
                }
                self.found.insert(path, *discovered);
            }
            Done::Parsed(module) => {
                let slot = self.resolving.len();
                self.requests.extend(module.sites.iter().enumerate().map(|(site, &require)| {
                    Request { slot, site, require, from_file: Arc::clone(&module.path) }
                }));
                let resolutions = module.sites.iter().map(|_| None).collect();
                let remaining = module.sites.len();
                self.resolving.push(Some(Resolving { module: *module, resolutions, remaining }));
            }
            // Each target is read as soon as its require resolves, not once the
            // module's last one does.
            Done::Resolved(resolved) => {
                for (Request { slot, site, .. }, resolution) in resolved {
                    if let Ok(target) = &resolution {
                        self.queue_read(&target.path);
                    }
                    let Some(Some(resolving)) = self.resolving.get_mut(slot) else {
                        continue;
                    };
                    if let Some(entry) = resolving.resolutions.get_mut(site) {
                        *entry = Some(resolution);
                    }
                    resolving.remaining = resolving.remaining.saturating_sub(1);
                    if resolving.remaining == 0
                        && let Some(Resolving { module, resolutions, .. }) =
                            self.resolving.get_mut(slot).and_then(Option::take)
                    {
                        let path = module.info.path.clone();
                        self.found.insert(path, module.resolved(resolutions.into_iter().flatten()));
                    }
                }
            }
        }
    }

    /// Queues `target` unless it's been seen.
    fn queue_read(&mut self, target: &Path) {
        if self.seen.len() <= MAX_MODULE_COUNT && !self.seen.contains(target) {
            self.seen.insert(target.to_path_buf());
            self.reads.push_back(target.to_path_buf());
        }
    }
}
