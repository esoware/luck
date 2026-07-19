use crate::module::{ModuleId, ModuleInfo, sanitize_module_name};
use crate::require_extraction::{ExtractResult, extract_requires};
use luck_core::config::DEFAULT_SEARCH_PATHS;
use luck_core::diagnostics::{Diagnostic, errors};
use luck_core::types::LuaTarget;
use luck_resolver::{normalize_path, resolve};
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::depth_first_search;
use petgraph::visit::{Control, DfsEvent};
use std::collections::HashMap;
use std::path::Path;

type QueueItem = String;

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
    // When the caller passes no search paths, fall back to the Lua defaults.
    // This is the single chokepoint for both `build_graph` and `bundle`
    // (which calls `build_graph`), so the default is applied exactly once.
    let default_search_paths: Vec<String>;
    let search_paths = if search_paths.is_empty() {
        default_search_paths = DEFAULT_SEARCH_PATHS.iter().map(|s| s.to_string()).collect();
        &default_search_paths
    } else {
        search_paths
    };

    let lua_version = target.lua_version();
    let mut modules: Vec<ModuleInfo> = Vec::new();
    let mut path_to_id: HashMap<String, ModuleId> = HashMap::new();
    let mut graph: DiGraph<ModuleId, ()> = DiGraph::new();
    let mut node_indices: Vec<NodeIndex> = Vec::new();
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut warnings: Vec<Diagnostic> = Vec::new();

    let mut queue: Vec<QueueItem> = Vec::new();

    let entry_normalized = normalize_path(entry_path);
    queue.push(entry_normalized.clone());

    while let Some(file_path) = queue.pop() {
        if path_to_id.contains_key(&file_path) {
            continue;
        }

        if modules.len() >= MAX_MODULE_COUNT {
            errors.push(errors::e009(&file_path, 0..0, MAX_MODULE_COUNT));
            return Err(errors);
        }

        process_module(
            &file_path,
            lua_version,
            target,
            search_paths,
            rc_dir,
            &mut modules,
            &mut path_to_id,
            &mut graph,
            &mut node_indices,
            &mut queue,
            &mut errors,
            &mut warnings,
        );
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    for (id, module) in modules.iter().enumerate() {
        let from_idx = node_indices[id];
        for (_local_name, _req_str, resolved_path, _call_span) in &module.dependencies {
            if let Some(&dep_id) = path_to_id.get(resolved_path) {
                let to_idx = node_indices[dep_id.0];
                graph.add_edge(from_idx, to_idx, ());
            }
        }
    }

    // The lazy loader is registration-order independent, so a cycle no
    // longer blocks bundling: deferred cycles (mutual requires inside
    // function bodies) work exactly like real Lua, and a load-time cycle
    // raises at runtime with a clear loader error. Warn and fall back to
    // discovery order.
    let topo_result = toposort(&graph, None);
    let topo_order: Vec<ModuleId> = match topo_result {
        Ok(sorted_nodes) => {
            // Reverse: toposort gives roots-first, we need leaves-first.
            sorted_nodes
                .into_iter()
                .rev()
                .map(|idx| graph[idx])
                .collect()
        }
        Err(cycle) => {
            let cycle_path = find_cycle_path(&graph, &modules, cycle.node_id());

            // Find the span of the require that closes the cycle (last module requiring the first)
            let closing_span = if cycle_path.len() >= 2 {
                let closing_module_path = &cycle_path[cycle_path.len() - 2];
                let target_module_path = &cycle_path[cycle_path.len() - 1];
                find_require_span(
                    &modules,
                    &path_to_id,
                    closing_module_path,
                    target_module_path,
                )
            } else {
                None
            };

            let file_path = if cycle_path.len() >= 2 {
                &cycle_path[cycle_path.len() - 2]
            } else {
                &modules[0].path
            };
            let span = closing_span.unwrap_or(0..0);

            warnings.push(errors::w003(file_path, span, &cycle_path));
            (0..modules.len()).map(ModuleId).collect()
        }
    };

    let entry_id = *path_to_id
        .get(&entry_normalized)
        .ok_or_else(|| vec![errors::e011(&entry_normalized, 0..0)])?;

    Ok(DependencyGraph {
        modules,
        topo_order,
        entry_id,
        warnings,
    })
}

/// Reads, parses, and resolves dependencies for a single module, adding it to the graph.
#[allow(clippy::too_many_arguments)]
fn process_module(
    file_path: &str,
    lua_version: luck_token::LuaVersion,
    target: LuaTarget,
    search_paths: &[String],
    rc_dir: &Path,
    modules: &mut Vec<ModuleInfo>,
    path_to_id: &mut HashMap<String, ModuleId>,
    graph: &mut DiGraph<ModuleId, ()>,
    node_indices: &mut Vec<NodeIndex>,
    queue: &mut Vec<QueueItem>,
    errors: &mut Vec<Diagnostic>,
    warnings: &mut Vec<Diagnostic>,
) {
    let os_path = file_path.replace('/', std::path::MAIN_SEPARATOR_STR);
    let source = match luck_core::source_io::read_source_file(&os_path) {
        Ok(s) => s,
        Err(io_error) => {
            errors.push(errors::e010(file_path, 0..0, &io_error.to_string()));
            return;
        }
    };

    if source.len() > MAX_FILE_SIZE {
        errors.push(errors::e012(file_path, 0..0, source.len(), MAX_FILE_SIZE));
        return;
    }

    let parse_result = luck_parser::parse(source, lua_version);
    if !parse_result.errors.is_empty() {
        for err in &parse_result.errors {
            let span = err.span.start as usize..err.span.end as usize;
            errors.push(errors::e008(file_path, span, &err.message));
        }
    }

    let ExtractResult {
        requires,
        diagnostics: ast_diags,
    } = extract_requires(&parse_result.block, file_path);

    for d in ast_diags {
        if d.is_error() {
            errors.push(d);
        } else {
            warnings.push(d);
        }
    }

    let mut deps: Vec<(String, String, String, std::ops::Range<usize>)> = Vec::new();
    for req in &requires {
        match resolve(
            &req.require_string,
            target,
            file_path,
            search_paths,
            rc_dir,
            req.span.clone(),
        ) {
            Ok(result) => {
                deps.push((
                    req.local_name.clone(),
                    req.require_string.clone(),
                    result.path.clone(),
                    req.call_span.clone(),
                ));
                warnings.extend(result.warnings);

                if !path_to_id.contains_key(&result.path) {
                    queue.push(result.path);
                }
            }
            Err(diag) => {
                errors.push(diag);
            }
        }
    }

    let module_id = ModuleId(modules.len());
    let relative_path = make_relative(file_path, rc_dir);
    let sanitized = sanitize_module_name(&relative_path);
    let node_idx = graph.add_node(module_id);

    path_to_id.insert(file_path.to_string(), module_id);
    node_indices.push(node_idx);
    modules.push(ModuleInfo {
        path: file_path.to_string(),
        source: parse_result.source,
        dependencies: deps,
        sanitized_name: sanitized,
        parsed_block: Some(parse_result.block),
    });
}

/// Finds the call_span of the require statement in `from_path` that resolves to `to_path`.
fn find_require_span(
    modules: &[ModuleInfo],
    path_to_id: &HashMap<String, ModuleId>,
    from_path: &str,
    to_path: &str,
) -> Option<std::ops::Range<usize>> {
    let from_id = path_to_id.get(from_path)?;
    let from_module = &modules[from_id.0];
    for (_local_name, _req_str, resolved_path, call_span) in &from_module.dependencies {
        if resolved_path == to_path {
            return Some(call_span.clone());
        }
    }
    None
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
            DfsEvent::Discover(n, _) => {
                path.push(n);
            }
            DfsEvent::BackEdge(_, v) => {
                if let Some(pos) = path.iter().position(|&x| x == v) {
                    cycle = path[pos..]
                        .iter()
                        .map(|&idx| modules[graph[idx].0].path.clone())
                        .collect();
                    cycle.push(modules[graph[v].0].path.clone());
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
    let base_normalized = normalize_path(base);
    let base_prefix = if base_normalized.ends_with('/') {
        base_normalized
    } else {
        format!("{base_normalized}/")
    };

    if path.starts_with(&base_prefix) {
        path[base_prefix.len()..].to_string()
    } else {
        path.to_string()
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

    /// Builds a graph whose NodeIndex `i` carries `ModuleId(i)`, matching
    /// `modules[i]`, from `(from, to)` edges given as module indices.
    fn build(paths: &[&str], edges: &[(usize, usize)]) -> (DiGraph<ModuleId, ()>, Vec<ModuleInfo>) {
        let modules: Vec<ModuleInfo> = paths.iter().map(|p| module(p)).collect();
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
