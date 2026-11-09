use alloc::collections::VecDeque;
use std::path::Path;

use luck_core::{
    DEFAULT_SEARCH_PATHS, Diagnostic, DynamicRequire, native_path, normalize_path, read_source_file,
};
use luck_resolver::{ResolveRequest, ResolvedModule, Resolver};
use luck_token::{LuaVersion, Span};
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{Control, DfsEvent, depth_first_search};
use rustc_hash::FxHashMap;

use crate::hot_comments::leading_hot_comments;
use crate::module::{Dependency, ModuleId, ModuleInfo};
use crate::relative_path::{absolute_project_root, make_relative};
use crate::require_extraction::{ExtractedRequires, extract_requires};

const MAX_MODULE_COUNT: usize = 10_000;
const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

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
    /// Lua search paths resolve against `project_root`, and every module's
    /// `relative_path` is relative to it. An empty `search_paths` means the
    /// default Lua templates.
    ///
    /// # Errors
    ///
    /// Returns every error found during discovery, such as unreadable,
    /// oversized, or unparsable files, the entry included, and unresolved or
    /// ambiguous requires.
    pub fn build(
        entry_path: &Path,
        version: LuaVersion,
        search_paths: &[String],
        project_root: &Path,
        dynamic_require: DynamicRequire,
    ) -> Result<Self, Vec<Diagnostic>> {
        let default_search_paths: Vec<String>;
        let effective_search_paths = if search_paths.is_empty() {
            default_search_paths = DEFAULT_SEARCH_PATHS.iter().map(ToString::to_string).collect();
            &default_search_paths
        } else {
            search_paths
        };

        // Emitted paths are made relative to this, so it must be absolute
        // and normalized. `luck bundle main.lua` passes the entry's parent,
        // which is empty for a bare file name.
        let root = absolute_project_root(project_root);

        let mut builder = GraphBuilder {
            version,
            project_root: &root,
            dynamic_require,
            resolver: Resolver::new(version, effective_search_paths, Path::new(&root)),
            modules: Vec::new(),
            path_to_id: FxHashMap::default(),
            queue: VecDeque::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        };
        let Some(entry_id) = builder.add_module(normalize_path(entry_path)) else {
            return Err(builder.errors);
        };
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
    modules: Vec<ModuleInfo>,
    path_to_id: FxHashMap<String, ModuleId>,
    queue: VecDeque<String>,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
}

impl GraphBuilder<'_> {
    /// Walks breadth-first from the modules added so far, stopping with E009
    /// at the module limit.
    fn discover_dependencies(&mut self) {
        while let Some(file_path) = self.queue.pop_front() {
            if self.path_to_id.contains_key(&file_path) {
                continue;
            }
            if self.modules.len() >= MAX_MODULE_COUNT {
                self.errors.push(Diagnostic::e009(&file_path, Span::default(), MAX_MODULE_COUNT));
                return;
            }
            self.add_module(file_path);
        }
    }

    /// Reads and parses one module and adds it with its resolved requires.
    ///
    /// `None` when the file can't be read or is too large, which records the
    /// error and adds nothing.
    fn add_module(&mut self, file_path: String) -> Option<ModuleId> {
        let source = match read_source_file(native_path(&file_path)) {
            Ok(source) => source,
            Err(io_error) => {
                self.errors.push(Diagnostic::e010(
                    &file_path,
                    Span::default(),
                    &io_error.to_string(),
                ));
                return None;
            }
        };

        if source.len() > MAX_FILE_SIZE {
            self.errors.push(Diagnostic::e012(
                &file_path,
                Span::default(),
                source.len(),
                MAX_FILE_SIZE,
            ));
            return None;
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
                    if !self.path_to_id.contains_key(&path) {
                        self.queue.push_back(path.clone());
                    }
                    self.warnings.extend(warnings);
                    dependencies.push(Dependency {
                        require_string: site.require_string,
                        resolved_path: path,
                        call_span: site.call_span,
                    });
                }
                Err(diagnostic) => self.errors.push(diagnostic),
            }
        }

        let relative_path = make_relative(&file_path, self.project_root);
        let id = ModuleId(self.modules.len());
        self.path_to_id.insert(file_path.clone(), id);
        self.modules.push(ModuleInfo {
            path: file_path,
            source: parsed.source,
            dependencies,
            relative_path,
            dynamic_callees,
            block: parsed.block,
        });
        Some(id)
    }

    /// # Errors
    ///
    /// Returns the errors collected during discovery.
    fn finish(mut self, entry_id: ModuleId) -> Result<DependencyGraph, Vec<Diagnostic>> {
        if !self.errors.is_empty() {
            return Err(self.errors);
        }

        let topological_order = self.topological_order();
        self.warn_inert_hot_comments(entry_id);

        Ok(DependencyGraph {
            modules: self.modules,
            topological_order,
            entry_id,
            warnings: self.warnings,
        })
    }

    /// Luau hot comments only take effect at the top of a chunk. The
    /// emitter hoists the entry module's leading run above the loader;
    /// any other module's hot comments become inert in the bundle.
    fn warn_inert_hot_comments(&mut self, entry_id: ModuleId) {
        if !self.version.is_luau() {
            return;
        }
        for (index, module) in self.modules.iter().enumerate() {
            if ModuleId(index) == entry_id {
                continue;
            }
            if let Some((offset, line)) = leading_hot_comments(&module.source).next() {
                // Unlike the hoisted copy, the warning drops the carriage
                // returns `str::lines` leaves on a line.
                let hot_comment = line.trim_end_matches('\r');
                self.warnings.push(Diagnostic::w006(
                    &module.path,
                    Span::from_indices(offset, offset.saturating_add(hot_comment.len())),
                    hot_comment,
                ));
            }
        }
    }

    /// Topological order, leaves first.
    ///
    /// The loader doesn't depend on registration order, so a cycle only warns
    /// (W003) and falls back to discovery order. A deferred cycle works as in
    /// real Lua, and a load-time cycle fails at runtime the way the target
    /// version fails.
    fn topological_order(&mut self) -> Vec<ModuleId> {
        // Node `i` is `modules[i]`. Insertion order breaks toposort's ties,
        // so the edges go in require order.
        let mut graph: DiGraph<(), ()> = DiGraph::new();
        for _ in &self.modules {
            graph.add_node(());
        }
        for (from, module) in self.modules.iter().enumerate() {
            for dependency in &module.dependencies {
                if let Some(to) = self.path_to_id.get(&dependency.resolved_path) {
                    graph.add_edge(NodeIndex::new(from), NodeIndex::new(to.0), ());
                }
            }
        }

        let cycle_node = match toposort(&graph, None) {
            // toposort gives roots first; reverse so leaves come first.
            Ok(sorted_nodes) => {
                return sorted_nodes.into_iter().rev().map(|node| ModuleId(node.index())).collect();
            }
            Err(cycle) => cycle.node_id(),
        };

        let cycle: Vec<&ModuleInfo> = find_cycle(&graph, cycle_node)
            .into_iter()
            .filter_map(|id| self.modules.get(id.0))
            .collect();
        let cycle_paths: Vec<String> = cycle.iter().map(|module| module.path.clone()).collect();

        // The require that closes the cycle: the last module in the path
        // requiring the first.
        let (file_path, closing_span) = match cycle.as_slice() {
            [.., from, to] => (
                from.path.as_str(),
                from.dependencies
                    .iter()
                    .find(|dependency| dependency.resolved_path == to.path)
                    .map(|dependency| dependency.call_span),
            ),
            [] | [_] => (self.modules.first().map_or("", |module| module.path.as_str()), None),
        };

        self.warnings.push(Diagnostic::w003(
            file_path,
            closing_span.unwrap_or_default(),
            &cycle_paths,
        ));
        (0..self.modules.len()).map(ModuleId).collect()
    }
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
