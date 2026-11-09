use alloc::collections::VecDeque;
use std::path::Path;

use luck_core::{Diagnostic, DynamicRequire, native_path, normalize_path, read_source_file};
use luck_resolver::{ResolveRequest, ResolvedModule, Resolver};
use luck_token::{CommentKind, LuaVersion, Span, leading_hot_comments};
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{Control, DfsEvent, depth_first_search};
use rustc_hash::FxHashMap;

use crate::module::{Dependency, ModuleId, ModuleInfo};
use crate::relative_path::{absolute_project_root, make_relative};
use crate::require_extraction::{ExtractedRequires, extract_requires};

const MAX_MODULE_COUNT: usize = 10_000;
const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

/// How to find a bundle's modules and what to build it for.
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

/// Every module an entry reaches, and the order the bundle registers them in.
#[derive(Debug)]
pub struct DependencyGraph {
    /// In discovery order, so the entry comes first.
    pub modules: Vec<ModuleInfo>,
    /// Leaves first, or discovery order when there's a cycle.
    pub topological_order: Vec<ModuleId>,
    pub entry_id: ModuleId,
    pub warnings: Vec<Diagnostic>,
}

impl DependencyGraph {
    /// Discovers all modules reachable from `entry_path`, then topologically
    /// sorts them.
    ///
    /// # Errors
    ///
    /// Returns every error found during discovery, such as unreadable,
    /// oversized, or unparsable files, the entry included, and unresolved or
    /// ambiguous requires.
    pub fn build(entry_path: &Path, options: &BundleOptions<'_>) -> Result<Self, Vec<Diagnostic>> {
        let BundleOptions { version, search_paths, project_root, dynamic_require } = *options;
        // Emitted paths are made relative to this, so it must be absolute
        // and normalized. `luck bundle main.lua` passes the entry's parent,
        // which is empty for a bare file name.
        let root = absolute_project_root(project_root);

        let mut builder = GraphBuilder {
            version,
            project_root: &root,
            dynamic_require,
            resolver: Resolver::new(version, search_paths, Path::new(&root)),
            modules: Vec::new(),
            path_to_id: FxHashMap::default(),
            queue: VecDeque::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        };
        let entry_id = builder.module_id(normalize_path(entry_path));
        builder.discover_dependencies();
        builder.finish(entry_id)
    }
}

/// The graph and its diagnostics while modules are still being discovered.
struct GraphBuilder<'input> {
    version: LuaVersion,
    /// From [`absolute_project_root`].
    project_root: &'input str,
    dynamic_require: DynamicRequire,
    resolver: Resolver<'input>,
    /// By `ModuleId`. `None` means not read yet or, on the slots left after
    /// discovery, an error was recorded for it.
    modules: Vec<Option<ModuleInfo>>,
    path_to_id: FxHashMap<String, ModuleId>,
    queue: VecDeque<(ModuleId, String)>,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
}

impl GraphBuilder<'_> {
    /// The id of the module at `file_path`, first queueing it for discovery
    /// if this is the first require to reach it.
    fn module_id(&mut self, file_path: String) -> ModuleId {
        if let Some(&id) = self.path_to_id.get(&file_path) {
            return id;
        }
        let id = ModuleId(self.modules.len());
        self.modules.push(None);
        self.path_to_id.insert(file_path.clone(), id);
        self.queue.push_back((id, file_path));
        id
    }

    /// Walks breadth-first from the modules queued so far, stopping with E009
    /// at the module limit.
    fn discover_dependencies(&mut self) {
        while let Some((id, file_path)) = self.queue.pop_front() {
            if id.0 >= MAX_MODULE_COUNT {
                self.errors.push(Diagnostic::e009(&file_path, Span::default(), MAX_MODULE_COUNT));
                return;
            }
            self.add_module(id, file_path);
        }
    }

    /// Reads and parses one module and fills its slot with its resolved
    /// requires. A file that can't be read or is too large records the error
    /// and leaves the slot empty.
    fn add_module(&mut self, id: ModuleId, file_path: String) {
        let source = match read_source_file(native_path(&file_path)) {
            Ok(source) => source,
            Err(io_error) => {
                self.errors.push(Diagnostic::e010(
                    &file_path,
                    Span::default(),
                    &io_error.to_string(),
                ));
                return;
            }
        };

        if source.len() > MAX_FILE_SIZE {
            self.errors.push(Diagnostic::e012(
                &file_path,
                Span::default(),
                source.len(),
                MAX_FILE_SIZE,
            ));
            return;
        }

        let parsed = luck_parser::parse(source, self.version);
        for error in &parsed.errors {
            self.errors.push(Diagnostic::e008(&file_path, error));
        }

        let ExtractedRequires { sites, dynamic_callees, diagnostics } =
            extract_requires(&parsed.block, &file_path, self.version, self.dynamic_require);
        for diagnostic in diagnostics {
            if diagnostic.is_error() {
                self.errors.push(diagnostic);
            } else {
                self.warnings.push(diagnostic);
            }
        }

        let mut dependencies = Vec::with_capacity(sites.len());
        for site in sites {
            match self.resolver.resolve(&ResolveRequest {
                module: &site.require_string,
                from_file: &file_path,
                span: site.call_span,
            }) {
                Ok(ResolvedModule { path, warnings }) => {
                    let target = self.module_id(path);
                    self.warnings.extend(warnings);
                    dependencies.push(Dependency {
                        require_string: site.require_string,
                        target,
                        call_span: site.call_span,
                    });
                }
                Err(diagnostic) => self.errors.push(diagnostic),
            }
        }

        let relative_path = make_relative(&file_path, self.project_root);
        let hot_comments = if self.version.is_luau() {
            leading_hot_comments(&parsed.comments, &parsed.source)
                .map(|comment| comment.span)
                .collect()
        } else {
            Vec::new()
        };
        let shebang = parsed
            .comments
            .iter()
            .find(|comment| comment.kind == CommentKind::Shebang)
            .map(|comment| comment.span);
        let module = ModuleInfo {
            path: file_path,
            source: parsed.source,
            dependencies,
            relative_path,
            dynamic_callees,
            hot_comments,
            shebang,
            block: parsed.block,
        };
        if let Some(slot) = self.modules.get_mut(id.0) {
            *slot = Some(module);
        }
    }

    /// # Errors
    ///
    /// Returns the errors collected during discovery.
    fn finish(self, entry_id: ModuleId) -> Result<DependencyGraph, Vec<Diagnostic>> {
        let Self { modules: module_slots, errors, mut warnings, .. } = self;
        if !errors.is_empty() {
            return Err(errors);
        }

        // No errors, so every slot was filled and each index is its `ModuleId`.
        let modules: Vec<ModuleInfo> = module_slots.into_iter().flatten().collect();
        let topological_order = topological_order(&modules, &mut warnings);
        warn_inert_hot_comments(&modules, entry_id, &mut warnings);

        Ok(DependencyGraph { modules, topological_order, entry_id, warnings })
    }
}

/// Luau hot comments only take effect at the top of a chunk. The emitter
/// hoists the entry module's leading run above the loader; any other module's
/// hot comments become inert in the bundle.
fn warn_inert_hot_comments(
    modules: &[ModuleInfo],
    entry_id: ModuleId,
    warnings: &mut Vec<Diagnostic>,
) {
    for (index, module) in modules.iter().enumerate() {
        if ModuleId(index) == entry_id {
            continue;
        }
        if let Some(span) = module.hot_comments.first() {
            warnings.push(Diagnostic::w006(&module.path, *span, span.text(&module.source)));
        }
    }
}

/// Topological order, leaves first.
///
/// The loader doesn't depend on registration order, so a cycle only warns
/// (W003) and falls back to discovery order. A deferred cycle works as in real
/// Lua, and a load-time cycle fails at runtime the way the target version
/// fails.
fn topological_order(modules: &[ModuleInfo], warnings: &mut Vec<Diagnostic>) -> Vec<ModuleId> {
    // Node `i` is `modules[i]`. Insertion order breaks toposort's ties, so the
    // edges go in require order.
    let mut graph: DiGraph<(), ()> = DiGraph::new();
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

    /// Builds a graph of `node_count` nodes from `(from, to)` edges.
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
