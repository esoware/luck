use std::ffi::OsString;
use std::panic;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use luck_allocator::{Allocator, ThreadAllocators};
use luck_diagnostics::Diagnostic;
use luck_resolver::{FileSystem as _, FileSystemOs, Resolver, to_slash_lossy};
use luck_token::{CommentKind, leading_hot_comments};
use memchr::memmem;
use rayon::Yield;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::analysis::{Analysis, StaticRequire, analyze};
use crate::dynamic_require::DynamicRequire;
use crate::graph::MAX_MODULE_COUNT;
use crate::module::ModuleInfo;
use crate::plan::PREFIX_STEM;
use crate::relative_path::make_relative;

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
    pub requires: Vec<(StaticRequire<'ast>, Option<PathBuf>)>,
}

/// A module parsed and analyzed, before its requires resolve.
pub(crate) struct ParsedModule<'ast> {
    /// Every field but the dependencies.
    pub info: ModuleInfo<'ast>,
    /// In source order.
    pub requires: Vec<StaticRequire<'ast>>,
    /// Parse errors, then the analysis's.
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

/// Parses `source`, the module at `path`, into `allocator` and analyzes its
/// tree.
pub(crate) fn parse_module<'ast>(
    allocator: &'ast Allocator,
    path: PathBuf,
    relative_path: String,
    source: &'ast [u8],
    dynamic_require: DynamicRequire,
) -> ParsedModule<'ast> {
    let parsed = luck_parser::parse(allocator, source);
    let mut errors = parsed.errors;
    let Analysis { requires, diags, exports } =
        analyze(allocator, &parsed.chunk, parsed.counts, source, dynamic_require);
    let (analysis_errors, warnings): (Vec<Diagnostic>, Vec<Diagnostic>) =
        diags.into_iter().partition(Diagnostic::is_error);
    errors.extend(analysis_errors);
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
        hot_comments,
        shebang,
        has_prefix_stem: memmem::find(source, PREFIX_STEM.as_bytes()).is_some(),
        exports,
    };
    ParsedModule { info, requires, errors, warnings }
}

/// What reading and resolving modules takes, which every task shares.
#[derive(Clone, Copy)]
pub(crate) struct Reader<'input> {
    dynamic_require: DynamicRequire,
    /// From [`absolute_project_root`](crate::relative_path::absolute_project_root).
    project_root: &'input Path,
    resolver: &'input Resolver,
}

impl<'input> Reader<'input> {
    pub(crate) const fn new(
        dynamic_require: DynamicRequire,
        project_root: &'input Path,
        resolver: &'input Resolver,
    ) -> Self {
        Self { dynamic_require, project_root, resolver }
    }

    /// Reads and parses the module at `path` into `allocator`.
    fn read<'ast>(
        &self,
        allocator: &'ast Allocator,
        path: &Path,
    ) -> Result<ParsedModule<'ast>, Diagnostic> {
        let file = FileSystemOs
            .read(path)
            .map_err(|io_error| Diagnostic::e010(&to_slash_lossy(path), &io_error))?;
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
            self.dynamic_require,
        ))
    }

    /// Reads the module at `path` into `allocator` and resolves its requires.
    pub(crate) fn discover<'ast>(
        &self,
        allocator: &'ast Allocator,
        path: &Path,
    ) -> Discovered<'ast> {
        let ParsedModule { info, requires, mut errors, mut warnings } =
            match self.read(allocator, path) {
                Ok(module) => module,
                Err(error) => {
                    return Discovered { errors: vec![error], warnings: Vec::new(), module: None };
                }
            };
        let resolved = requires
            .into_iter()
            .map(|require| match self.resolver.resolve(path, require.require_string) {
                Ok(resolution) => {
                    warnings.extend(resolution.to_diagnostic(require.path_span));
                    (require, Some(resolution.path))
                }
                Err(error) => {
                    errors.push(error.to_diagnostic(require.path_span, require.require_string));
                    (require, None)
                }
            })
            .collect();
        Discovered {
            errors,
            warnings,
            module: Some(ResolvedModuleInfo { info, requires: resolved }),
        }
    }
}

/// Discovers every module `entry` reaches, by canonical path.
///
/// Each module is a task on rayon's pool that reads it into the arena of the
/// thread running the task and resolves its requires. This thread alone
/// keeps the graph, and starts a task for each module a finished one is the
/// first to reach.
///
/// A canonical path is one module whatever its components, so the maps key
/// it as text, which hashes in one pass where a `Path` hashes a component at
/// a time.
pub(crate) fn discover_reachable<'ast>(
    arenas: &'ast ThreadAllocators,
    reader: Reader<'_>,
    entry: &Path,
) -> FxHashMap<OsString, Discovered<'ast>> {
    let mut found: FxHashMap<OsString, Discovered<'ast>> = FxHashMap::default();
    // Every path read or being read, which stops growing once more are known
    // than a graph may hold.
    let mut seen = FxHashSet::from_iter([entry.as_os_str().to_owned()]);
    let (sender, receiver) = mpsc::channel::<(PathBuf, thread::Result<Discovered<'ast>>)>();
    rayon::in_place_scope(|scope| {
        let start = |path: PathBuf| {
            let task_sender = sender.clone();
            scope.spawn(move |_| {
                // A task that panicked would otherwise leave this thread
                // waiting for it forever.
                let outcome =
                    panic::catch_unwind(AssertUnwindSafe(|| reader.discover(arenas.get(), &path)));
                let _ = task_sender.send((path, outcome));
            });
        };
        start(entry.to_path_buf());
        while found.len() < seen.len() {
            let (path, outcome) = next_report(
                || receiver.try_recv().ok(),
                || {
                    receiver
                        .recv()
                        .expect("`sender` outlives the loop, so a task still out reports")
                },
            );
            let discovered = outcome.unwrap_or_else(|payload| panic::resume_unwind(payload));
            for target in discovered
                .module
                .iter()
                .flat_map(|info| &info.requires)
                .filter_map(|(_, target)| target.as_ref())
            {
                if seen.len() <= MAX_MODULE_COUNT && !seen.contains(target.as_os_str()) {
                    seen.insert(target.as_os_str().to_owned());
                    start(target.clone());
                }
            }
            found.insert(path.into_os_string(), discovered);
        }
    });
    found
}

/// The first report `poll` gives, with this thread running the pool's queued
/// tasks while there's none, so a pool of one runs them all here.
///
/// Only this thread starts tasks, so once none is queued, each one still to
/// report is running on another thread, and this blocks in `wait` rather
/// than spin until one does. Off the pool, nothing runs here, so it blocks
/// after the first poll.
fn next_report<Report>(
    mut poll: impl FnMut() -> Option<Report>,
    wait: impl FnOnce() -> Report,
) -> Report {
    loop {
        if let Some(report) = poll() {
            return report;
        }
        if !matches!(rayon::yield_now(), Some(Yield::Executed)) {
            return wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};

    use rayon::ThreadPoolBuilder;

    use super::*;

    fn on_one_thread<T: Send>(work: impl FnOnce() -> T + Send) -> T {
        ThreadPoolBuilder::new().num_threads(1).build().expect("the pool starts").install(work)
    }

    /// Polling again would spin until a task on another thread reported, and
    /// with nothing queued, there's nothing to run in the meantime.
    #[test]
    fn next_report_waits_once_no_task_is_queued() {
        let (report, polls) = on_one_thread(|| {
            let polls = Cell::new(0);
            let poll = || {
                polls.set(polls.get() + 1);
                (polls.get() > 1).then_some("polled again")
            };
            (next_report(poll, || "waited"), polls.get())
        });
        assert_eq!((report, polls), ("waited", 1));
    }

    /// On a pool of one, a queued task can only run here, so waiting first
    /// would wait for it forever.
    #[test]
    fn next_report_runs_a_queued_task_before_it_waits() {
        let report = on_one_thread(|| {
            let has_run = AtomicBool::new(false);
            rayon::in_place_scope(|scope| {
                scope.spawn(|_| has_run.store(true, Ordering::Relaxed));
                next_report(
                    || has_run.load(Ordering::Relaxed).then_some("ran the task"),
                    || "waited",
                )
            })
        });
        assert_eq!(report, "ran the task");
    }
}
