use alloc::collections::VecDeque;
use core::panic::AssertUnwindSafe;
use std::panic;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, Scope};

use luck_allocator::Allocator;
use luck_core::Diagnostic;
use luck_resolver::{ResolveRequest, ResolvedModule, Resolver, native_path, read_source_file};
use luck_token::{CommentKind, LuaVersion, Span, leading_hot_comments};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dynamic_require::DynamicRequire;
use crate::graph::{BundleOptions, MAX_MODULE_COUNT};
use crate::loader::PREFIX_STEM;
use crate::module::ModuleInfo;
use crate::relative_path::make_relative;
use crate::require_extraction::{ExtractedRequires, RequireSite, extract_requires};

const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

/// The most requires one thread is handed at once, so the threads that free up
/// meanwhile still find some left.
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
    pub requires: Vec<(RequireSite<'ast>, Option<String>)>,
}

/// A module parsed and analyzed, before its requires resolve.
pub(crate) struct ParsedModule<'ast> {
    /// Every field but the dependencies.
    pub info: ModuleInfo<'ast>,
    /// `info.path` in the arena, for the requests that resolve its requires.
    pub path: &'ast str,
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
        resolutions: impl IntoIterator<Item = Result<ResolvedModule, Diagnostic>>,
    ) -> Discovered<'ast> {
        let Self { info, sites, mut errors, mut warnings, .. } = self;
        let requires = sites
            .into_iter()
            .zip(resolutions)
            .map(|(site, resolution)| match resolution {
                Ok(ResolvedModule { path, warnings: resolve_warnings }) => {
                    warnings.extend(resolve_warnings);
                    (site, Some(path))
                }
                Err(diag) => {
                    errors.push(diag);
                    (site, None)
                }
            })
            .collect();
        Discovered { errors, warnings, module: Some(ResolvedModuleInfo { info, requires }) }
    }
}

/// Parses `source`, the module at `path`, into `allocator` and analyzes its
/// tree.
pub(crate) fn parse_module<'ast>(
    allocator: &'ast Allocator,
    path: String,
    relative_path: String,
    source: &'ast str,
    version: LuaVersion,
    dynamic_require: DynamicRequire,
) -> ParsedModule<'ast> {
    let parsed = luck_parser::parse(allocator, source, version);
    let mut errors: Vec<Diagnostic> =
        parsed.errors.iter().map(|error| Diagnostic::e008(&path, error)).collect();
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
    } = extract_requires(allocator, &parsed.block, source, &path, version, dynamic_require);
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
    let arena_path = allocator.alloc_str(&path);
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
    ParsedModule { info, path: arena_path, sites, errors, warnings }
}

/// What reading and resolving modules takes on one thread.
pub(crate) struct Reader<'input> {
    version: LuaVersion,
    dynamic_require: DynamicRequire,
    /// From [`absolute_project_root`](crate::relative_path::absolute_project_root).
    project_root: &'input str,
    resolver: Resolver<'input>,
}

/// Reading a module either parses it or fails with E010 or E012.
type Read<'ast> = Result<Box<ParsedModule<'ast>>, Diagnostic>;

impl<'input> Reader<'input> {
    pub(crate) fn new(options: &BundleOptions<'input>, project_root: &'input str) -> Self {
        Self {
            version: options.version,
            dynamic_require: options.dynamic_require,
            project_root,
            resolver: Resolver::new(options.version, options.search_paths, Path::new(project_root)),
        }
    }

    /// Reads and parses the module at `path` into `allocator`.
    ///
    /// # Errors
    ///
    /// Returns E010 when the file can't be read and E012 when it's too
    /// large.
    fn read<'ast>(&self, allocator: &'ast Allocator, path: &str) -> Read<'ast> {
        let file = read_source_file(native_path(path))
            .map_err(|io_error| Diagnostic::e010(path, Span::default(), &io_error.to_string()))?;
        if file.len() > MAX_FILE_SIZE {
            return Err(Diagnostic::e012(path, Span::default(), file.len(), MAX_FILE_SIZE));
        }
        let source = allocator.alloc_str(&file);
        drop(file);
        Ok(Box::new(parse_module(
            allocator,
            path.to_owned(),
            make_relative(path, self.project_root),
            source,
            self.version,
            self.dynamic_require,
        )))
    }

    /// # Errors
    ///
    /// Returns the resolver's diagnostic for a require that names no module,
    /// or more than one.
    fn resolve(
        &mut self,
        require: &RequireSite<'_>,
        from_file: &str,
    ) -> Result<ResolvedModule, Diagnostic> {
        self.resolver.resolve(&ResolveRequest {
            module: require.require_string,
            from_file,
            span: require.call_span,
        })
    }

    /// Reads the module at `path` and resolves its requires on this thread.
    pub(crate) fn discover<'ast>(
        &mut self,
        allocator: &'ast Allocator,
        path: &str,
    ) -> Discovered<'ast> {
        match self.read(allocator, path) {
            Ok(module) => {
                let resolutions: Vec<_> =
                    module.sites.iter().map(|site| self.resolve(site, module.path)).collect();
                module.resolved(resolutions)
            }
            Err(error) => Discovered { errors: vec![error], warnings: Vec::new(), module: None },
        }
    }
}

/// The require at `site` in the module waiting on it in `slot`, which is in
/// `from_file`.
#[derive(Clone, Copy)]
struct Request<'ast> {
    slot: usize,
    site: usize,
    require: RequireSite<'ast>,
    from_file: &'ast str,
    /// Lua: the first request for its require string, whose resolution the
    /// other sites making it share.
    is_shared: bool,
}

enum Task<'ast> {
    Read(String),
    Resolve(Vec<Request<'ast>>),
}

enum Done<'ast> {
    Read(String, Read<'ast>),
    Resolved(Vec<(Request<'ast>, Result<ResolvedModule, Diagnostic>)>),
}

impl<'ast> Task<'ast> {
    fn run(self, reader: &mut Reader<'_>, allocator: &'ast Allocator) -> Done<'ast> {
        match self {
            Self::Read(path) => {
                let module = reader.read(allocator, &path);
                Done::Read(path, module)
            }
            Self::Resolve(requests) => Done::Resolved(
                requests
                    .into_iter()
                    .map(|request| (request, reader.resolve(&request.require, request.from_file)))
                    .collect(),
            ),
        }
    }
}

/// What a thread that reads modules sends back: its index, and what its task
/// found or the panic that stopped it.
type WorkerResult<'ast> = (usize, thread::Result<Done<'ast>>);

/// Lua: where a require string stands, since only the string decides what it
/// names.
enum LuaPath {
    /// Its first request hasn't come back, and these sites wait on it.
    Resolving(Vec<(usize, usize)>),
    Resolved(String),
}

/// A module whose requires are resolving.
struct Resolving<'ast> {
    module: ParsedModule<'ast>,
    resolutions: Vec<Option<Result<ResolvedModule, Diagnostic>>>,
    remaining: usize,
}

/// Every module the entry reaches, read and resolved as separate tasks on as
/// many threads as there are arenas, with each Lua require string resolved
/// once however many modules make it.
struct Discovery<'ast> {
    version: LuaVersion,
    found: FxHashMap<String, Discovered<'ast>>,
    /// Every path found, being read, or queued to be, which stops growing once
    /// more are known than a graph may hold.
    seen: FxHashSet<String>,
    reads: VecDeque<String>,
    requests: VecDeque<Request<'ast>>,
    /// By slot, `None` once resolved.
    resolving: Vec<Option<Resolving<'ast>>>,
    lua_paths: FxHashMap<&'ast str, LuaPath>,
}

/// Discovers every module `entry` reaches, by canonical path, reading the
/// entry into `allocator` on this thread, as [`Discovery::run`] runs tasks.
pub(crate) fn discover_reachable<'ast>(
    allocator: &'ast Allocator,
    spare_arenas: &'ast mut [Allocator],
    reader: &mut Reader<'_>,
    entry: &str,
    options: &BundleOptions<'_>,
    project_root: &str,
) -> FxHashMap<String, Discovered<'ast>> {
    let mut discovery = Discovery {
        version: options.version,
        found: FxHashMap::default(),
        seen: FxHashSet::default(),
        reads: VecDeque::new(),
        requests: VecDeque::new(),
        resolving: Vec::new(),
        lua_paths: FxHashMap::default(),
    };
    discovery.seen.insert(entry.to_owned());
    discovery.reads.push_back(entry.to_owned());
    thread::scope(|scope| {
        discovery.run(scope, reader, allocator, spare_arenas, options, project_root);
    });
    discovery.found
}

impl<'ast> Discovery<'ast> {
    /// The next task, with at most `batch` requires to resolve.
    fn next_task(&mut self, batch: usize) -> Option<Task<'ast>> {
        if !self.requests.is_empty() {
            let count = batch.min(self.requests.len());
            return Some(Task::Resolve(self.requests.drain(..count).collect()));
        }
        self.reads.pop_front().map(Task::Read)
    }

    fn finish(&mut self, done: Done<'ast>) {
        match done {
            Done::Read(path, Err(error)) => {
                let unreadable =
                    Discovered { errors: vec![error], warnings: Vec::new(), module: None };
                self.found.insert(path, unreadable);
            }
            Done::Read(_, Ok(module)) => self.resolve(*module),
            Done::Resolved(resolved) => {
                for (request, resolution) in resolved {
                    self.settle(request, resolution);
                }
            }
        }
    }

    /// Queues a request for each of `module`'s requires that no other
    /// module's has asked for.
    fn resolve(&mut self, module: ParsedModule<'ast>) {
        let slot = self.resolving.len();
        let resolutions = module.sites.iter().map(|_| None).collect();
        let remaining = module.sites.len();
        let mut resolving = Resolving { module, resolutions, remaining };
        for (site, &require) in resolving.module.sites.iter().enumerate() {
            let from_file = resolving.module.path;
            let request = Request { slot, site, require, from_file, is_shared: false };
            if self.version.is_luau() {
                self.requests.push_back(request);
                continue;
            }
            match self.lua_paths.get_mut(require.require_string) {
                Some(LuaPath::Resolved(path)) => {
                    if let Some(resolution) = resolving.resolutions.get_mut(site) {
                        *resolution =
                            Some(Ok(ResolvedModule { path: path.clone(), warnings: Vec::new() }));
                    }
                    resolving.remaining = resolving.remaining.saturating_sub(1);
                }
                Some(LuaPath::Resolving(waiting)) => waiting.push((slot, site)),
                None => {
                    self.lua_paths.insert(require.require_string, LuaPath::Resolving(Vec::new()));
                    self.requests.push_back(Request { is_shared: true, ..request });
                }
            }
        }
        let is_resolved = resolving.remaining == 0;
        self.resolving.push(Some(resolving));
        if is_resolved {
            self.complete(slot);
        }
    }

    /// Records what `request` resolved to, for the sites waiting on its Lua
    /// require string too. Each of those asks again on its own after a
    /// failure, since the error names the site.
    fn settle(&mut self, request: Request<'ast>, resolution: Result<ResolvedModule, Diagnostic>) {
        if request.is_shared {
            let string = request.require.require_string;
            let previous = match &resolution {
                Ok(resolved) => {
                    self.lua_paths.insert(string, LuaPath::Resolved(resolved.path.clone()))
                }
                Err(_) => self.lua_paths.remove(string),
            };
            if let Some(LuaPath::Resolving(waiting)) = previous {
                for (slot, site) in waiting {
                    match &resolution {
                        Ok(resolved) => {
                            let shared = ResolvedModule {
                                path: resolved.path.clone(),
                                warnings: Vec::new(),
                            };
                            self.record(slot, site, Ok(shared));
                        }
                        Err(_) => self.requeue(slot, site),
                    }
                }
            }
        }
        self.record(request.slot, request.site, resolution);
    }

    fn requeue(&mut self, slot: usize, site: usize) {
        if let Some(Some(resolving)) = self.resolving.get(slot)
            && let Some(&require) = resolving.module.sites.get(site)
        {
            let from_file = resolving.module.path;
            self.requests.push_back(Request { slot, site, require, from_file, is_shared: false });
        }
    }

    fn record(&mut self, slot: usize, site: usize, resolution: Result<ResolvedModule, Diagnostic>) {
        let Some(Some(resolving)) = self.resolving.get_mut(slot) else {
            return;
        };
        if let Some(entry) = resolving.resolutions.get_mut(site) {
            *entry = Some(resolution);
        }
        resolving.remaining = resolving.remaining.saturating_sub(1);
        if resolving.remaining == 0 {
            self.complete(slot);
        }
    }

    /// Files the module in `slot`, every require resolved, and queues each
    /// module it reaches that no other has.
    fn complete(&mut self, slot: usize) {
        let Some(Resolving { module, resolutions, .. }) =
            self.resolving.get_mut(slot).and_then(Option::take)
        else {
            return;
        };
        let path = module.info.path.clone();
        let discovered = module.resolved(resolutions.into_iter().flatten());
        for target in discovered
            .module
            .iter()
            .flat_map(|info| &info.requires)
            .filter_map(|(_, target)| target.as_ref())
        {
            if self.seen.len() <= MAX_MODULE_COUNT && !self.seen.contains(target) {
                self.seen.insert(target.clone());
                self.reads.push_back(target.clone());
            }
        }
        self.found.insert(path, discovered);
    }

    /// Runs every task, each on a thread with one of `spare_arenas`, which
    /// starts only when every other is busy. A task runs on this thread, with
    /// `reader` and `allocator`, while no other thread is busy and it's the
    /// only one or no thread can take it.
    fn run<'scope>(
        &mut self,
        scope: &'scope Scope<'scope, '_>,
        reader: &mut Reader<'_>,
        allocator: &'ast Allocator,
        spare_arenas: &'ast mut [Allocator],
        options: &'scope BundleOptions<'_>,
        project_root: &'scope str,
    ) where
        'ast: 'scope,
    {
        let (result_sender, results) = mpsc::channel::<WorkerResult<'ast>>();
        let mut unused_arenas = spare_arenas.iter_mut();
        let mut workers: Vec<Sender<Task<'ast>>> = Vec::new();
        let mut idle_workers: Vec<usize> = Vec::new();
        let mut busy = 0usize;
        loop {
            let available = idle_workers.len().saturating_add(unused_arenas.len());
            if busy == 0
                && (available == 0 || self.reads.len().saturating_add(self.requests.len()) <= 1)
            {
                let Some(task) = self.next_task(usize::MAX) else {
                    return;
                };
                self.finish(task.run(reader, allocator));
                continue;
            }
            for _ in 0..available {
                let batch = self.requests.len().div_ceil(available).clamp(1, MAX_RESOLVE_BATCH);
                let Some(task) = self.next_task(batch) else {
                    break;
                };
                let worker = if let Some(worker) = idle_workers.pop() {
                    worker
                } else if let Some(arena) = unused_arenas.next() {
                    let index = workers.len();
                    let sender = result_sender.clone();
                    workers.push(spawn_worker(scope, arena, index, sender, options, project_root));
                    index
                } else {
                    break;
                };
                // A thread stops taking tasks only once it panics, which
                // stops discovery too.
                if workers.get(worker).is_some_and(|tasks| tasks.send(task).is_ok()) {
                    busy = busy.saturating_add(1);
                }
            }
            let Ok((worker, done)) = results.recv() else {
                return;
            };
            busy = busy.saturating_sub(1);
            idle_workers.push(worker);
            self.finish(done.unwrap_or_else(|payload| panic::resume_unwind(payload)));
        }
    }
}

/// Starts a thread that runs each task it's sent, reading into `arena`, and
/// sends back what it found.
fn spawn_worker<'scope, 'ast>(
    scope: &'scope Scope<'scope, '_>,
    arena: &'ast mut Allocator,
    index: usize,
    results: Sender<WorkerResult<'ast>>,
    options: &'scope BundleOptions<'_>,
    project_root: &'scope str,
) -> Sender<Task<'ast>>
where
    'ast: 'scope,
{
    let (task_sender, tasks) = mpsc::channel::<Task<'ast>>();
    scope.spawn(move || {
        let allocator: &'ast Allocator = arena;
        let mut reader = Reader::new(options, project_root);
        for task in tasks {
            let done = panic::catch_unwind(AssertUnwindSafe(|| task.run(&mut reader, allocator)));
            if results.send((index, done)).is_err() {
                return;
            }
        }
    });
    task_sender
}
