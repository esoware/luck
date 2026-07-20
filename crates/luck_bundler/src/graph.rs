use crate::module::{Dependency, ModuleId, ModuleInfo, sanitize_module_name};
use crate::require_extraction::{ExtractResult, extract_requires};
use luck_core::config::DEFAULT_SEARCH_PATHS;
use luck_core::diagnostics::{Diagnostic, errors};
use luck_core::types::LuaTarget;
use luck_resolver::{ResolveRequest, Resolver, normalize_path_str};
use luck_token::LuaVersion;
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{Control, DfsEvent, depth_first_search};
use rustc_hash::FxHashMap;
use std::ops::Range;
use std::path::Path;

const MAX_MODULE_COUNT: usize = 10_000;
const MAX_FILE_SIZE: usize = 10 * 1024 * 1024; // 10 MB

/// The resolved module dependency graph: modules in topological order with the entry point identified.
pub struct DependencyGraph {
    pub modules: Vec<ModuleInfo>,
    pub topo_order: Vec<ModuleId>,
    pub entry_id: ModuleId,
    pub warnings: Vec<Diagnostic>,
}

/// Discovers all modules reachable from `entry_path`, then topologically sorts them.
pub fn build_graph(
    entry_path: &Path,
    target: LuaTarget,
    search_paths: &[String],
    rc_dir: &Path,
) -> Result<DependencyGraph, Vec<Diagnostic>> {
    // When the caller passes no search paths, fall back to the Lua
    // defaults. This is the single chokepoint for both `build_graph` and
    // `bundle` (which calls it), so the default is applied exactly once.
    let default_search_paths: Vec<String>;
    let search_paths = if search_paths.is_empty() {
        default_search_paths = DEFAULT_SEARCH_PATHS.iter().map(|s| s.to_string()).collect();
        &default_search_paths
    } else {
        search_paths
    };

    let mut builder = GraphBuilder::new(target, search_paths, rc_dir);
    let entry_normalized = normalize_path_str(entry_path);
    builder.discover(entry_normalized.clone());
    builder.finish(&entry_normalized)
}

/// Accumulates modules, edges, and diagnostics as the BFS walk discovers
/// them. Owning the whole in-progress graph on one struct keeps
/// [`GraphBuilder::process_module`] a plain method instead of a function
/// threading a dozen `&mut` scratch buffers.
struct GraphBuilder<'a> {
    lua_version: LuaVersion,
    target: LuaTarget,
    search_paths: &'a [String],
    rc_dir: &'a Path,
    resolver: Resolver,
    modules: Vec<ModuleInfo>,
    path_to_id: FxHashMap<String, ModuleId>,
    graph: DiGraph<ModuleId, ()>,
    node_indices: Vec<NodeIndex>,
    queue: Vec<String>,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
}

impl<'a> GraphBuilder<'a> {
    fn new(target: LuaTarget, search_paths: &'a [String], rc_dir: &'a Path) -> Self {
        GraphBuilder {
            lua_version: target.lua_version(),
            target,
            search_paths,
            rc_dir,
            resolver: Resolver::new(),
            modules: Vec::new(),
            path_to_id: FxHashMap::default(),
            graph: DiGraph::new(),
            node_indices: Vec::new(),
            queue: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Breadth-first walk from the entry module, resolving and enqueuing
    /// each newly discovered dependency until the queue drains.
    fn discover(&mut self, entry_normalized: String) {
        self.queue.push(entry_normalized);
        while let Some(file_path) = self.queue.pop() {
            if self.path_to_id.contains_key(&file_path) {
                continue;
            }
            if self.modules.len() >= MAX_MODULE_COUNT {
                self.errors
                    .push(errors::e009(&file_path, 0..0, MAX_MODULE_COUNT));
                return;
            }
            self.process_module(&file_path);
        }
    }

    /// Reads, parses, and resolves dependencies for a single module, adding it to the graph.
    fn process_module(&mut self, file_path: &str) {
        let os_path = file_path.replace('/', std::path::MAIN_SEPARATOR_STR);
        let source = match luck_core::source_io::read_source_file(&os_path) {
            Ok(source) => source,
            Err(io_error) => {
                self.errors
                    .push(errors::e010(file_path, 0..0, &io_error.to_string()));
                return;
            }
        };

        if source.len() > MAX_FILE_SIZE {
            self.errors
                .push(errors::e012(file_path, 0..0, source.len(), MAX_FILE_SIZE));
            return;
        }

        let parse_result = luck_parser::parse(source, self.lua_version);
        for err in &parse_result.errors {
            let span = err.span.start as usize..err.span.end as usize;
            self.errors
                .push(errors::e008(file_path, span, &err.message));
        }

        let ExtractResult {
            requires,
            diagnostics,
        } = extract_requires(&parse_result.block, file_path);
        for diag in diagnostics {
            if diag.is_error() {
                self.errors.push(diag);
            } else {
                self.warnings.push(diag);
            }
        }

        let mut dependencies: Vec<Dependency> = Vec::with_capacity(requires.len());
        for req in &requires {
            match self.resolver.resolve(&ResolveRequest {
                module: &req.require_string,
                from_file: file_path,
                target: self.target,
                search_paths: self.search_paths,
                project_root: self.rc_dir,
                span: req.span,
            }) {
                Ok(resolved) => {
                    // The resolver returns a forward-slash normalized PathBuf; the
                    // graph keys on its string form, so derive it here at the boundary.
                    let resolved_path = resolved.path.to_string_lossy().into_owned();
                    if !self.path_to_id.contains_key(&resolved_path) {
                        self.queue.push(resolved_path.clone());
                    }
                    dependencies.push(Dependency {
                        require_string: req.require_string.clone(),
                        resolved_path,
                        call_span: req.call_span.clone(),
                    });
                    self.warnings.extend(resolved.warnings);
                }
                Err(diag) => self.errors.push(diag),
            }
        }

        let module_id = ModuleId(self.modules.len());
        let node_idx = self.graph.add_node(module_id);
        let sanitized_name = sanitize_module_name(&make_relative(file_path, self.rc_dir));

        self.path_to_id.insert(file_path.to_string(), module_id);
        self.node_indices.push(node_idx);
        self.modules.push(ModuleInfo {
            path: file_path.to_string(),
            source: parse_result.source,
            dependencies,
            sanitized_name,
            parsed_block: Some(parse_result.block),
        });
    }

    fn finish(mut self, entry_normalized: &str) -> Result<DependencyGraph, Vec<Diagnostic>> {
        if !self.errors.is_empty() {
            return Err(self.errors);
        }

        self.add_edges();
        let topo_order = self.topo_order();
        let entry_id = *self
            .path_to_id
            .get(entry_normalized)
            .ok_or_else(|| vec![errors::e011(entry_normalized, 0..0)])?;

        Ok(DependencyGraph {
            modules: self.modules,
            topo_order,
            entry_id,
            warnings: self.warnings,
        })
    }

    fn add_edges(&mut self) {
        let mut edges: Vec<(NodeIndex, NodeIndex)> = Vec::new();
        for (id, module) in self.modules.iter().enumerate() {
            let from_idx = self.node_indices[id];
            for dep in &module.dependencies {
                if let Some(&dep_id) = self.path_to_id.get(&dep.resolved_path) {
                    edges.push((from_idx, self.node_indices[dep_id.0]));
                }
            }
        }
        for (from_idx, to_idx) in edges {
            self.graph.add_edge(from_idx, to_idx, ());
        }
    }

    /// Topological order (leaves first). The lazy loader is
    /// registration-order independent, so a cycle no longer blocks
    /// bundling: deferred cycles (mutual requires inside function bodies)
    /// work exactly like real Lua, and a load-time cycle raises at runtime
    /// with a clear loader error. On a cycle, warn and fall back to
    /// discovery order.
    fn topo_order(&mut self) -> Vec<ModuleId> {
        match toposort(&self.graph, None) {
            // toposort gives roots first; reverse so leaves come first.
            Ok(sorted_nodes) => sorted_nodes
                .into_iter()
                .rev()
                .map(|idx| self.graph[idx])
                .collect(),
            Err(cycle) => {
                let cycle_path = find_cycle_path(&self.graph, &self.modules, cycle.node_id());

                // The require that closes the cycle: the last module in the
                // path requiring the first.
                let closing_span = if cycle_path.len() >= 2 {
                    find_require_span(
                        &self.modules,
                        &self.path_to_id,
                        &cycle_path[cycle_path.len() - 2],
                        &cycle_path[cycle_path.len() - 1],
                    )
                } else {
                    None
                };
                let file_path = if cycle_path.len() >= 2 {
                    cycle_path[cycle_path.len() - 2].clone()
                } else {
                    self.modules[0].path.clone()
                };

                self.warnings.push(errors::w003(
                    &file_path,
                    closing_span.unwrap_or(0..0),
                    &cycle_path,
                ));
                (0..self.modules.len()).map(ModuleId).collect()
            }
        }
    }
}

/// Finds the call span of the require in `from_path` that resolves to `to_path`.
fn find_require_span(
    modules: &[ModuleInfo],
    path_to_id: &FxHashMap<String, ModuleId>,
    from_path: &str,
    to_path: &str,
) -> Option<Range<usize>> {
    let from_id = path_to_id.get(from_path)?;
    modules[from_id.0]
        .dependencies
        .iter()
        .find(|dep| dep.resolved_path == to_path)
        .map(|dep| dep.call_span.clone())
}

fn find_cycle_path(
    graph: &DiGraph<ModuleId, ()>,
    modules: &[ModuleInfo],
    start_node: NodeIndex,
) -> Vec<String> {
    let mut path: Vec<NodeIndex> = Vec::new();
    let mut cycle: Vec<String> = Vec::new();

    depth_first_search(graph, Some(start_node), |event| {
        match event {
            DfsEvent::Discover(node, _) => path.push(node),
            DfsEvent::BackEdge(_, target) => {
                if let Some(pos) = path.iter().position(|&node| node == target) {
                    cycle = path[pos..]
                        .iter()
                        .map(|&idx| modules[graph[idx].0].path.clone())
                        .collect();
                    cycle.push(modules[graph[target].0].path.clone());
                    return Control::Break(());
                }
            }
            DfsEvent::Finish(_, _) => {
                path.pop();
            }
            _ => {}
        }
        Control::<()>::Continue
    });

    if cycle.is_empty() {
        vec![modules[graph[start_node].0].path.clone()]
    } else {
        cycle
    }
}

fn make_relative(path: &str, base: &Path) -> String {
    let base_normalized = normalize_path_str(base);
    let base_prefix = if base_normalized.ends_with('/') {
        base_normalized
    } else {
        format!("{base_normalized}/")
    };

    match path.strip_prefix(&base_prefix) {
        Some(relative) => relative.to_string(),
        None => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(path: &str) -> ModuleInfo {
        ModuleInfo {
            path: path.to_string(),
            source: String::new(),
            dependencies: vec![],
            sanitized_name: sanitize_module_name(path),
            parsed_block: None,
        }
    }

    /// Builds a graph whose `NodeIndex` `i` carries `ModuleId(i)`, matching
    /// `modules[i]`, from `(from, to)` edges given as module indices.
    fn build(paths: &[&str], edges: &[(usize, usize)]) -> (DiGraph<ModuleId, ()>, Vec<ModuleInfo>) {
        let modules: Vec<ModuleInfo> = paths.iter().map(|path| module(path)).collect();
        let mut graph: DiGraph<ModuleId, ()> = DiGraph::new();
        let nodes: Vec<NodeIndex> = (0..paths.len())
            .map(|i| graph.add_node(ModuleId(i)))
            .collect();
        for &(from, to) in edges {
            graph.add_edge(nodes[from], nodes[to], ());
        }
        (graph, modules)
    }

    #[test]
    fn self_cycle_reports_module_twice() {
        let (graph, modules) = build(&["a.lua"], &[(0, 0)]);
        let cycle = find_cycle_path(&graph, &modules, NodeIndex::new(0));
        assert_eq!(cycle, vec!["a.lua".to_string(), "a.lua".to_string()]);
    }

    #[test]
    fn two_module_cycle_reports_closed_loop() {
        let (graph, modules) = build(&["a.lua", "b.lua"], &[(0, 1), (1, 0)]);
        let cycle = find_cycle_path(&graph, &modules, NodeIndex::new(0));
        assert_eq!(
            cycle,
            vec![
                "a.lua".to_string(),
                "b.lua".to_string(),
                "a.lua".to_string()
            ]
        );
    }

    #[test]
    fn three_module_cycle_reports_ordered_path() {
        let (graph, modules) = build(&["a.lua", "b.lua", "c.lua"], &[(0, 1), (1, 2), (2, 0)]);
        let cycle = find_cycle_path(&graph, &modules, NodeIndex::new(0));
        assert_eq!(
            cycle,
            vec![
                "a.lua".to_string(),
                "b.lua".to_string(),
                "c.lua".to_string(),
                "a.lua".to_string()
            ]
        );
    }

    #[test]
    fn diamond_dependency_reports_no_cycle() {
        // a -> b, a -> c, b -> d, c -> d: a DAG with no back edge.
        let (graph, modules) = build(
            &["a.lua", "b.lua", "c.lua", "d.lua"],
            &[(0, 1), (0, 2), (1, 3), (2, 3)],
        );
        assert!(toposort(&graph, None).is_ok(), "diamond is acyclic");
        // With no cycle, the finder falls back to the lone start module.
        let cycle = find_cycle_path(&graph, &modules, NodeIndex::new(0));
        assert_eq!(cycle, vec!["a.lua".to_string()]);
    }

    #[test]
    fn independent_cycles_report_the_one_reachable_from_start() {
        // Two disjoint 2-cycles; the search only explores from its start,
        // so it reports whichever cycle the start node belongs to.
        let (graph, modules) = build(
            &["a.lua", "b.lua", "c.lua", "d.lua"],
            &[(0, 1), (1, 0), (2, 3), (3, 2)],
        );
        let from_c = find_cycle_path(&graph, &modules, NodeIndex::new(2));
        assert_eq!(
            from_c,
            vec![
                "c.lua".to_string(),
                "d.lua".to_string(),
                "c.lua".to_string()
            ]
        );
        let from_a = find_cycle_path(&graph, &modules, NodeIndex::new(0));
        assert_eq!(
            from_a,
            vec![
                "a.lua".to_string(),
                "b.lua".to_string(),
                "a.lua".to_string()
            ]
        );
    }
}
