use std::path::Path;

use luck_allocator::Allocator;
use luck_core::Diagnostic;
use luck_resolver::{Resolver, normalize_path};
use luck_token::{LuaVersion, Span};
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
    pub warnings: Vec<Diagnostic>,
}

impl<'ast> DependencyGraph<'ast> {
    /// Reads and parses every module the entry reaches, each into the arena
    /// of the thread that reads it.
    ///
    /// Each arena after the first backs a thread that reads modules and
    /// resolves requires as the calling thread hands them out. The calling
    /// thread reads the entry into the first arena, and takes a task itself
    /// only while no other thread is busy, so it does all the work when
    /// there's one arena or one task at a time. The graph and its diagnostics
    /// are the same however many arenas there are.
    ///
    /// # Errors
    ///
    /// Returns every error found during discovery, such as unreadable,
    /// oversized, or unparsable files, the entry included, and unresolved or
    /// ambiguous requires.
    ///
    /// # Panics
    ///
    /// Panics if `arenas` is empty.
    pub fn build(
        arenas: &'ast mut [Allocator],
        entry_path: &Path,
        options: &BundleOptions<'_>,
    ) -> Result<Self, Vec<Diagnostic>> {
        build_graph(arenas, entry_path, options).1
    }
}

/// [`DependencyGraph::build`], and the first of `arenas`, which the calling
/// thread keeps.
///
/// # Panics
///
/// Panics if `arenas` is empty.
pub(crate) fn build_graph<'ast>(
    arenas: &'ast mut [Allocator],
    entry_path: &Path,
    options: &BundleOptions<'_>,
) -> (&'ast Allocator, Result<DependencyGraph<'ast>, Vec<Diagnostic>>) {
    #[expect(clippy::expect_used, reason = "the caller gives the calling thread an arena")]
    let (first, spare_arenas) = arenas.split_first_mut().expect("an arena for the calling thread");
    let allocator: &'ast Allocator = first;
    // Emitted paths are made relative to this, so it must be absolute and
    // normalized. `luck bundle main.lua` passes the entry's parent, which is
    // empty for a bare file name.
    let root = absolute_project_root(options.project_root);
    let resolver = Resolver::new(options.version, options.search_paths, Path::new(&root));
    let reader = Reader::new(options, &root, &resolver);
    let entry = normalize_path(entry_path);
    let mut found = discover_reachable(allocator, spare_arenas, reader, &entry);
    let graph = number_modules(entry, &mut found, |path| reader.discover(allocator, path));
    (allocator, graph)
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
    entry: String,
    found: &mut FxHashMap<String, Discovered<'ast>>,
    mut discover: impl FnMut(&str) -> Discovered<'ast>,
) -> Result<DependencyGraph<'ast>, Vec<Diagnostic>> {
    let mut ids: FxHashMap<String, ModuleId> = FxHashMap::default();
    ids.insert(entry.clone(), ModuleId(0));
    let mut paths = vec![entry];
    let mut modules: Vec<ModuleInfo<'ast>> = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut next = 0;
    while let Some(path) = paths.get(next) {
        if next >= MAX_MODULE_COUNT {
            errors.push(Diagnostic::e009(path, Span::default(), MAX_MODULE_COUNT));
            break;
        }
        let Discovered { errors: module_errors, warnings: module_warnings, module } =
            found.remove(path).unwrap_or_else(|| discover(path));
        errors.extend(module_errors);
        warnings.extend(module_warnings);
        if let Some(ResolvedModuleInfo { mut info, requires }) = module {
            info.dependencies = requires
                .into_iter()
                .filter_map(|(site, resolved)| {
                    let next_id = ModuleId(paths.len());
                    let id = *ids.entry(resolved?).or_insert_with_key(|target| {
                        paths.push(target.clone());
                        next_id
                    });
                    Some(Dependency {
                        require_string: site.require_string,
                        target: id,
                        call_span: site.call_span,
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
    let topological_order = topological_order(&modules, &mut warnings);
    let entry_id = ModuleId(0);
    warn_inert_hot_comments(&modules, entry_id, &mut warnings);
    Ok(DependencyGraph { modules, topological_order, entry_id, warnings })
}

/// Luau hot comments only take effect at the top of a chunk. The emitter
/// hoists the entry module's leading run above the loader; any other module's
/// hot comments become inert in the bundle.
fn warn_inert_hot_comments(
    modules: &[ModuleInfo<'_>],
    entry_id: ModuleId,
    warnings: &mut Vec<Diagnostic>,
) {
    for (i, module) in modules.iter().enumerate() {
        if ModuleId(i) == entry_id {
            continue;
        }
        if let Some(span) = module.hot_comments.first() {
            warnings.push(Diagnostic::w006(&module.path, *span, span.text(module.source)));
        }
    }
}

/// The loader doesn't depend on registration order, so a cycle only warns
/// (W003) and falls back to discovery order.
///
/// A deferred cycle works as in real Lua, and a load-time cycle fails at
/// runtime the way the target version fails.
fn topological_order(modules: &[ModuleInfo<'_>], warnings: &mut Vec<Diagnostic>) -> Vec<ModuleId> {
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
    let cycle_paths: Vec<String> =
        cycle.iter().filter_map(|id| modules.get(id.0)).map(|module| module.path.clone()).collect();

    // The require that closes the cycle: the last module in the path requiring
    // the first.
    let closing_require = match cycle.as_slice() {
        [.., from, to] => modules.get(from.0).map(|module| (module, *to)),
        [] | [_] => None,
    };
    let (file_path, closing_span) = closing_require.map_or_else(
        || (modules.first().map_or("", |module| module.path.as_str()), None),
        |(module, to)| {
            let closing_span = module
                .dependencies
                .iter()
                .find(|dependency| dependency.target == to)
                .map(|dependency| dependency.call_span);
            (module.path.as_str(), closing_span)
        },
    );

    warnings.push(Diagnostic::w003(file_path, closing_span.unwrap_or_default(), &cycle_paths));
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
