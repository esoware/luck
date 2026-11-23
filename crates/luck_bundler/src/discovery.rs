use core::panic::AssertUnwindSafe;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use luck_allocator::{Allocator, ThreadAllocators};
use luck_diagnostics::Diagnostic;
use luck_resolver::{
    FileSystem as _, FileSystemOs, Resolution, ResolveError, Resolver, to_slash_lossy,
};
use luck_token::{CommentKind, LuaVersion, leading_hot_comments};
use memchr::memmem;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dynamic_require::DynamicRequire;
use crate::graph::{BundleOptions, MAX_MODULE_COUNT};
use crate::loader::PREFIX_STEM;
use crate::module::ModuleInfo;
use crate::relative_path::make_relative;
use crate::require_extraction::{ExtractedRequires, RequireSite, extract_requires};

const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

/// Everything one module's discovery finds, which depends on no other
/// module, with the diagnostics in its file in the order the graph reports
/// them.
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
    /// In source order.
    pub sites: Vec<RequireSite<'ast>>,
    /// Parse errors, then the analysis's.
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

/// What reports `error`, which stopped `site` from resolving.
fn resolve_error_diagnostic(site: &RequireSite<'_>, error: &ResolveError) -> Diagnostic {
    let (span, module) = (site.call_span, site.require_string);
    match error {
        ResolveError::NotFound(searched) => {
            let tried_paths: Vec<String> =
                searched.iter().map(|path| to_slash_lossy(path).into_owned()).collect();
            Diagnostic::e004(span, module, &tried_paths)
        }
        ResolveError::MissingPrefix => Diagnostic::e004_missing_prefix(span, module),
        ResolveError::BareSelfAlias => Diagnostic::e004_bare_self_alias(span),
        ResolveError::UnknownAlias(alias) => Diagnostic::e004_unknown_alias(span, module, alias),
        ResolveError::MalformedLuaurc(luaurc) => {
            Diagnostic::e004_malformed_luaurc(span, module, &to_slash_lossy(luaurc))
        }
        ResolveError::AliasCycle(cycle) => Diagnostic::e004_alias_cycle(span, module, cycle),
        ResolveError::BothExtensions { luau, lua } => {
            Diagnostic::e007_both_extensions(span, &to_slash_lossy(luau), &to_slash_lossy(lua))
        }
        ResolveError::FileAndDirectory { file, init } => {
            Diagnostic::e007_file_and_directory(span, &to_slash_lossy(file), &to_slash_lossy(init))
        }
        ResolveError::FileBesideDirectory { file, directory } => {
            Diagnostic::e007_file_beside_directory(
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
    source: &'ast [u8],
    version: LuaVersion,
    dynamic_require: DynamicRequire,
) -> ParsedModule<'ast> {
    let parsed = luck_parser::parse(allocator, source, version);
    let mut errors = parsed.errors;
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
    } = extract_requires(allocator, &parsed.chunk, parsed.counts, source, dynamic_require);
    for diag in diags {
        if diag.is_error() {
            errors.push(diag);
        } else {
            warnings.push(diag);
        }
    }
    let hot_comments = leading_hot_comments(&parsed.comments, parsed.chunk.block.span.start)
        .map(|comment| comment.span)
        .collect();
    let shebang = parsed
        .comments
        .first()
        .filter(|comment| comment.kind == CommentKind::Shebang)
        .map(|comment| comment.span);
    let info = ModuleInfo {
        path,
        relative_path,
        source,
        block: parsed.chunk.block,
        nesting_depth: parsed.nesting_depth,
        dependencies: Vec::new(),
        dynamic_callees,
        hot_comments,
        shebang,
        has_vararg_read,
        has_global_arg_read,
        has_env_rebinding,
        max_function_upvalues,
        has_prefix_stem: memmem::find(source, PREFIX_STEM.as_bytes()).is_some(),
        exports,
    };
    ParsedModule { info, sites, errors, warnings }
}

/// What reading and resolving modules takes, which every task shares.
#[derive(Clone, Copy)]
pub(crate) struct Reader<'input> {
    version: LuaVersion,
    dynamic_require: DynamicRequire,
    /// From [`absolute_project_root`](crate::relative_path::absolute_project_root).
    project_root: &'input Path,
    resolver: &'input Resolver,
}

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
    fn read<'ast>(
        &self,
        allocator: &'ast Allocator,
        path: &Path,
    ) -> Result<ParsedModule<'ast>, Diagnostic> {
        let file = FileSystemOs
            .read(path)
            .map_err(|io_error| Diagnostic::e010(&to_slash_lossy(path), &io_error.to_string()))?;
        if file.len() > MAX_FILE_SIZE {
            return Err(Diagnostic::e012(file.len(), MAX_FILE_SIZE));
        }
        let source = allocator.alloc_slice_copy(&file);
        drop(file);
        Ok(parse_module(
            allocator,
            path.to_path_buf(),
            make_relative(path, self.project_root),
            source,
            self.version,
            self.dynamic_require,
        ))
    }

    /// Reads the module at `path` into `allocator` and resolves its requires.
    pub(crate) fn discover<'ast>(
        &self,
        allocator: &'ast Allocator,
        path: &Path,
    ) -> Discovered<'ast> {
        let ParsedModule { info, sites, mut errors, mut warnings } =
            match self.read(allocator, path) {
                Ok(module) => module,
                Err(error) => {
                    return Discovered { errors: vec![error], warnings: Vec::new(), module: None };
                }
            };
        let requires = sites
            .into_iter()
            .map(|site| match self.resolver.resolve(path, site.require_string) {
                Ok(Resolution { path: target, is_self_alias_shadowed }) => {
                    if is_self_alias_shadowed {
                        warnings.push(Diagnostic::w004(site.call_span));
                    }
                    (site, Some(target))
                }
                Err(error) => {
                    errors.push(resolve_error_diagnostic(&site, &error));
                    (site, None)
                }
            })
            .collect();
        Discovered { errors, warnings, module: Some(ResolvedModuleInfo { info, requires }) }
    }
}

/// Discovers every module `entry` reaches, by canonical path.
///
/// Each module is a task on rayon's pool that reads it into the arena of the
/// thread running the task and resolves its requires. This thread alone
/// keeps the graph, and starts a task for each module a finished one is the
/// first to reach.
pub(crate) fn discover_reachable<'ast>(
    arenas: &'ast ThreadAllocators,
    reader: Reader<'_>,
    entry: &Path,
) -> FxHashMap<PathBuf, Discovered<'ast>> {
    let mut found: FxHashMap<PathBuf, Discovered<'ast>> = FxHashMap::default();
    // Every path read or being read, which stops growing once more are known
    // than a graph may hold.
    let mut seen = FxHashSet::from_iter([entry.to_path_buf()]);
    let (sender, receiver) = mpsc::channel::<(PathBuf, thread::Result<Discovered<'ast>>)>();
    let is_worker_thread = rayon::current_thread_index().is_some();
    rayon::in_place_scope(|scope| {
        let start = |path: PathBuf| {
            let task_sender = sender.clone();
            scope.spawn(move |_| {
                // A task that panicked would otherwise leave this thread
                // waiting for it forever.
                let outcome =
                    panic::catch_unwind(AssertUnwindSafe(|| reader.discover(arenas.get(), &path)));
                let _unused = task_sender.send((path, outcome));
            });
        };
        start(entry.to_path_buf());
        while found.len() < seen.len() {
            let report =
                if is_worker_thread { receiver.try_recv().ok() } else { receiver.recv().ok() };
            // A thread of the pool runs queued tasks while it waits, so a pool
            // of one runs them all here.
            let Some((path, outcome)) = report else {
                rayon::yield_now();
                continue;
            };
            let discovered = outcome.unwrap_or_else(|payload| panic::resume_unwind(payload));
            for target in discovered
                .module
                .iter()
                .flat_map(|info| &info.requires)
                .filter_map(|(_, target)| target.as_ref())
            {
                if seen.len() <= MAX_MODULE_COUNT && !seen.contains(target) {
                    seen.insert(target.clone());
                    start(target.clone());
                }
            }
            found.insert(path, discovered);
        }
    });
    found
}
