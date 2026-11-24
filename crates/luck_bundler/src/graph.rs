use core::mem;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use luck_allocator::ThreadAllocators;
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_resolver::{FileSystem as _, FileSystemOs, ResolveOptions, Resolver, to_slash_lossy};
use luck_token::LuaVersion;
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{Control, DfsEvent, depth_first_search};
use rustc_hash::FxHashMap;

use crate::discovery::{Discovered, Reader, ResolvedModuleInfo, discover_reachable};
use crate::dynamic_require::DynamicRequire;
use crate::module::{Dependency, ModuleId, ModuleInfo};
use crate::relative_path::absolute_project_root;

pub(crate) const MAX_MODULE_COUNT: usize = 10_000;

#[derive(Debug, Clone, Copy)]
pub struct BundleOptions<'input> {
    pub version: LuaVersion,
    /// Lua search path templates, resolved against `project_root`. Empty
    /// means the default templates.
    pub search_paths: &'input [String],
    /// Every module's `relative_path`, and so every path in the output, is
    /// relative to this.
    pub project_root: &'input Path,
    pub dynamic_require: DynamicRequire,
}

#[derive(Debug)]
pub struct DependencyGraph<'ast> {
    /// In discovery order, so the entry comes first.
    pub modules: Vec<ModuleInfo<'ast>>,
    /// Leaves first, or discovery order when there's a cycle.
    pub topological_order: Vec<ModuleId>,
    pub entry_id: ModuleId,
    /// By module, in discovery order.
    pub warnings: Vec<FileDiagnostics>,
}

impl<'ast> DependencyGraph<'ast> {
    /// Reads and parses every module the entry reaches, each on rayon's pool
    /// into the arena of the thread that reads it.
    ///
    /// The calling thread keeps the graph. On a thread of the pool, it runs
    /// queued modules while it waits, so a pool of one thread reads them all
    /// there. The graph and its diagnostics are the same however many threads
    /// read them.
    ///
    /// # Errors
    ///
    /// Returns every error found during discovery, such as unreadable,
    /// oversized, or unparsable files, the entry included, and unresolved or
    /// ambiguous requires.
    pub fn build(
        arenas: &'ast ThreadAllocators,
        entry_path: &Path,
        options: &BundleOptions<'_>,
    ) -> Result<Self, Vec<FileDiagnostics>> {
        // Emitted paths are made relative to this, so it must be canonical like
        // the modules' paths.
        let root = absolute_project_root(options.project_root);
        let resolver = Resolver::new(ResolveOptions {
            version: options.version,
            search_paths: options.search_paths.to_vec(),
            root: root.clone(),
        });
        let reader = Reader::new(options, &root, &resolver);
        let entry =
            FileSystemOs.canonicalize(entry_path).unwrap_or_else(|_| entry_path.to_path_buf());
        let found = discover_reachable(arenas, reader, &entry);
        number_modules(entry, found, |path| reader.discover(arenas.get(), path))
    }
}

/// Numbers the modules breadth-first from `entry`, in require order.
///
/// The diagnostics are gathered in that order too, so neither depends on how
/// discovery was scheduled. A module that discovery skipped is discovered
/// now. Stops with E009 at the module limit.
///
/// # Errors
///
/// Returns the errors discovery found.
fn number_modules<'ast>(
    entry: PathBuf,
    mut found: FxHashMap<OsString, Discovered<'ast>>,
    mut discover: impl FnMut(&Path) -> Discovered<'ast>,
) -> Result<DependencyGraph<'ast>, Vec<FileDiagnostics>> {
    // By canonical path, as text, as `discover_reachable` keys it.
    let mut ids: FxHashMap<OsString, ModuleId> = FxHashMap::default();
    ids.insert(entry.as_os_str().to_owned(), ModuleId(0));
    let mut paths = vec![entry];
    let mut modules: Vec<ModuleInfo<'ast>> = Vec::new();
    let mut errors = Vec::new();
    // By module, so each holds what's found in its file.
    let mut module_warnings: Vec<Vec<Diagnostic>> = Vec::new();
    let mut next = 0;
    while let Some(path) = paths.get(next) {
        if next >= MAX_MODULE_COUNT {
            let diags = vec![Diagnostic::e009(MAX_MODULE_COUNT)];
            errors.push(FileDiagnostics { path: path.clone(), diags });
            break;
        }
        let Discovered { errors: found_errors, warnings: found_warnings, module } =
            found.remove(path.as_os_str()).unwrap_or_else(|| discover(path));
        if !found_errors.is_empty() {
            errors.push(FileDiagnostics { path: path.clone(), diags: found_errors });
        }
        if let Some(ResolvedModuleInfo { mut info, requires }) = module {
            module_warnings.push(found_warnings);
            info.dependencies = requires
                .into_iter()
                .filter_map(|(require, resolved)| {
                    let next_id = ModuleId(paths.len());
                    let id = *ids.entry(resolved?.into_os_string()).or_insert_with_key(|target| {
                        paths.push(PathBuf::from(target));
                        next_id
                    });
                    Some(Dependency {
                        require_string: require.require_string,
                        target: id,
                        call_span: require.call_span,
                    })
                })
                .collect();
            modules.push(info);
        }
        next = next.saturating_add(1);
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // No errors, so every module was read and each index is its `ModuleId`.
    let topological_order = topological_order(&modules, &mut module_warnings);
    let entry_id = ModuleId(0);
    warn_inert_hot_comments(&modules, entry_id, &mut module_warnings);
    let warnings = modules
        .iter()
        .zip(&mut module_warnings)
        .filter(|(_, diags)| !diags.is_empty())
        .map(|(module, diags)| FileDiagnostics {
            path: module.path.clone(),
            diags: mem::take(diags),
        })
        .collect();
    Ok(DependencyGraph { modules, topological_order, entry_id, warnings })
}

/// Luau hot comments only take effect at the top of a chunk. The emitter
/// hoists the entry module's leading run above the loader; any other module's
/// hot comments become inert in the bundle.
fn warn_inert_hot_comments(
    modules: &[ModuleInfo<'_>],
    entry_id: ModuleId,
    warnings: &mut [Vec<Diagnostic>],
) {
    for ((i, module), module_warnings) in modules.iter().enumerate().zip(warnings) {
        if ModuleId(i) == entry_id {
            continue;
        }
        if let Some(span) = module.hot_comments.first() {
            module_warnings
                .push(Diagnostic::w006(*span, &String::from_utf8_lossy(span.text(module.source))));
        }
    }
}

/// The loader doesn't depend on registration order, so a cycle only warns
/// (W003) and falls back to discovery order.
///
/// A deferred cycle works as in real Lua, and a load-time cycle fails at
/// runtime the way the target version fails.
fn topological_order(
    modules: &[ModuleInfo<'_>],
    warnings: &mut [Vec<Diagnostic>],
) -> Vec<ModuleId> {
    // Node `i` is `modules[i]`. Insertion order breaks toposort's ties, so the
    // edges go in require order.
    let mut graph: DiGraph<(), ()> = DiGraph::with_capacity(modules.len(), 0);
    for _ in modules {
        graph.add_node(());
    }
    for (from, module) in modules.iter().enumerate() {
        for dependency in &module.dependencies {
            graph.add_edge(NodeIndex::new(from), NodeIndex::new(dependency.target.0), ());
        }
    }

    let cycle_node = match toposort(&graph, None) {
        // toposort gives roots first; reverse so leaves come first.
        Ok(sorted_nodes) => {
            return sorted_nodes.into_iter().rev().map(|node| ModuleId(node.index())).collect();
        }
        Err(cycle) => cycle.node_id(),
    };

    let cycle = find_cycle(&graph, cycle_node);
    let cycle_paths: Vec<String> = cycle
        .iter()
        .filter_map(|id| modules.get(id.0))
        .map(|module| to_slash_lossy(&module.path).into_owned())
        .collect();

    // The require that closes the cycle: the last module in the path requiring
    // the first.
    let (reporting_id, closing_span) = match cycle.as_slice() {
        [.., from, to] => {
            let closing_span = modules.get(from.0).and_then(|module| {
                let dependency = module.dependencies.iter().find(|found| found.target == *to);
                dependency.map(|found| found.call_span)
            });
            (*from, closing_span)
        }
        [] | [_] => (ModuleId(0), None),
    };
    if let Some(module_warnings) = warnings.get_mut(reporting_id.0) {
        module_warnings.push(Diagnostic::w003(closing_span, &cycle_paths));
    }
    (0..modules.len()).map(ModuleId).collect()
}

/// The first cycle a depth-first search from `start_node` closes, as the
/// modules around it with the first repeated at the end. Just `start_node`
/// when the search closes none.
fn find_cycle(graph: &DiGraph<(), ()>, start_node: NodeIndex) -> Vec<ModuleId> {
    let mut path: Vec<NodeIndex> = Vec::new();
    let mut cycle: Vec<ModuleId> = Vec::new();

    depth_first_search(graph, Some(start_node), |event| {
        match event {
            DfsEvent::Discover(node, _) => path.push(node),
            DfsEvent::BackEdge(_, target) => {
                if let Some(cycle_start) = path.iter().position(|&node| node == target) {
                    cycle = path
                        .iter()
                        .skip(cycle_start)
                        .chain([&target])
                        .map(|node| ModuleId(node.index()))
                        .collect();
                    return Control::Break(());
                }
            }
            DfsEvent::Finish(_, _) => {
                path.pop();
            }
            DfsEvent::TreeEdge(_, _) | DfsEvent::CrossForwardEdge(_, _) => {}
        }
        Control::<()>::Continue
    });

    if cycle.is_empty() { vec![ModuleId(start_node.index())] } else { cycle }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(node_count: usize, edges: &[(usize, usize)]) -> DiGraph<(), ()> {
        let mut graph = DiGraph::new();
        for _ in 0..node_count {
            graph.add_node(());
        }
        for &(from, to) in edges {
            graph.add_edge(NodeIndex::new(from), NodeIndex::new(to), ());
        }
        graph
    }

    fn ids(indices: &[usize]) -> Vec<ModuleId> {
        indices.iter().copied().map(ModuleId).collect()
    }

    #[test]
    fn self_cycle_reports_module_twice() {
        let cycle = find_cycle(&graph(1, &[(0, 0)]), NodeIndex::new(0));
        assert_eq!(cycle, ids(&[0, 0]));
    }

    #[test]
    fn two_module_cycle_reports_closed_loop() {
        let cycle = find_cycle(&graph(2, &[(0, 1), (1, 0)]), NodeIndex::new(0));
        assert_eq!(cycle, ids(&[0, 1, 0]));
    }

    #[test]
    fn three_module_cycle_reports_ordered_path() {
        let cycle = find_cycle(&graph(3, &[(0, 1), (1, 2), (2, 0)]), NodeIndex::new(0));
        assert_eq!(cycle, ids(&[0, 1, 2, 0]));
    }

    #[test]
    fn acyclic_graph_reports_only_the_start_module() {
        // a -> b, a -> c, b -> d, c -> d: a DAG with no back edge.
        let diamond = graph(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        assert!(toposort(&diamond, None).is_ok(), "diamond is acyclic");
        // With no cycle, the finder falls back to the lone start module.
        assert_eq!(find_cycle(&diamond, NodeIndex::new(0)), ids(&[0]));
    }

    #[test]
    fn independent_cycles_report_the_one_reachable_from_start() {
        // Two disjoint 2-cycles; the search only explores from its start,
        // so it reports whichever cycle the start node belongs to.
        let two_cycles = graph(4, &[(0, 1), (1, 0), (2, 3), (3, 2)]);
        assert_eq!(find_cycle(&two_cycles, NodeIndex::new(2)), ids(&[2, 3, 2]));
        assert_eq!(find_cycle(&two_cycles, NodeIndex::new(0)), ids(&[0, 1, 0]));
    }
}
