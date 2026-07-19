use luck_ast::expr::{Expression, FunctionCall};
use luck_ast::shared::Block;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;

use crate::graph::DependencyGraph;
use crate::require_extraction::extract_require_string;

/// The memoizing loader emitted at the top of every multi-module bundle.
/// Mirrors real `require` semantics: results cache after first load, a
/// module returning nil caches as `true` (like `package.loaded`), and a
/// cycle hit DURING a module's load raises - while cycles deferred into
/// function bodies (mutually recursive modules) work, exactly as in Lua.
const LOADER: &str = "local __luck_modules={}\n\
local __luck_loaded={}\n\
local __luck_loading={}\n\
local function __luck_require(id)\n\
local value=__luck_loaded[id]\n\
if value~=nil then\n\
if value==__luck_loading then error(\"luck bundle: require cycle hit while loading module #\"..id,2)end\n\
return value\n\
end\n\
__luck_loaded[id]=__luck_loading\n\
value=__luck_modules[id](id)\n\
if value==nil then value=true end\n\
__luck_loaded[id]=value\n\
return value\n\
end\n";

/// One contiguous run of bundle lines that came from a single module.
/// Line 1 of the module's source lands on `bundle_start_line`, so a
/// runtime traceback line `L` inside the range maps back to source line
/// `L - bundle_start_line + 1`. Require rewrites are span replacements
/// that stay on one line, so the correspondence is 1:1 unless a source
/// file spreads a single `require(...)` call across multiple lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMapEntry {
    /// 1-based first bundle line of the module body (inclusive).
    pub bundle_start_line: usize,
    /// 1-based last bundle line of the module body (inclusive).
    pub bundle_end_line: usize,
    pub path: String,
}

/// Generates the bundled Lua output from a resolved dependency graph.
///
/// Modules register as functions in a table and load LAZILY on first
/// require. Compared to the old eager IIFE-per-module emit this
/// preserves real require semantics (side effects run at require time,
/// not bundle load), has no 200-locals-per-function ceiling, and places
/// no restriction on where requires appear.
pub fn emit(dep_graph: &DependencyGraph, version: luck_token::LuaVersion) -> String {
    emit_with_line_map(dep_graph, version).0
}

/// [`emit`], plus the module->bundle line map for the produced output.
pub fn emit_with_line_map(
    dep_graph: &DependencyGraph,
    version: luck_token::LuaVersion,
) -> (String, Vec<LineMapEntry>) {
    let modules = &dep_graph.modules;
    let topo_order = &dep_graph.topo_order;
    let entry_id = dep_graph.entry_id;

    // Numeric ids in topo order (dependencies first - cosmetic only; the
    // loader is order-independent). Entry never registers: it inlines.
    let mut path_to_slot: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut slot = 0usize;
    for id in topo_order {
        if *id == entry_id {
            continue;
        }
        slot += 1;
        path_to_slot.insert(&modules[id.0].path, slot);
    }

    if path_to_slot.is_empty() {
        let entry = &modules[entry_id.0];
        let line_map = vec![LineMapEntry {
            bundle_start_line: 1,
            bundle_end_line: entry.source.lines().count().max(1),
            path: entry.path.clone(),
        }];
        return (entry.source.clone(), line_map);
    }

    // Bundle size is roughly the module sources plus the loader and a
    // small per-module wrapper; reserving it up front avoids realloc
    // copies of the whole bundle.
    let source_total: usize = modules.iter().map(|module| module.source.len()).sum();
    let mut output = String::with_capacity(source_total + LOADER.len() + 64 * modules.len());
    let mut line_map: Vec<LineMapEntry> = Vec::with_capacity(modules.len());
    // 1-based line the NEXT pushed character lands on; every push below
    // ends on a newline, so this stays exact.
    let mut next_line = 1usize;
    let push = |output: &mut String, next_line: &mut usize, fragment: &str| {
        output.push_str(fragment);
        *next_line += fragment.matches('\n').count();
    };

    push(&mut output, &mut next_line, "do\n");
    push(&mut output, &mut next_line, LOADER);

    for id in topo_order {
        if *id == entry_id {
            continue;
        }
        let module = &modules[id.0];
        let module_slot = path_to_slot[module.path.as_str()];
        let mut body = transform_module_body(
            &module.source,
            &module.dependencies,
            &path_to_slot,
            version,
            module.parsed_block.as_ref(),
        );

        // Provenance: runtime tracebacks and human readers can map slots
        // back to modules. The sanitized name is machine-independent
        // (absolute paths would leak build-host details into output).
        push(
            &mut output,
            &mut next_line,
            &format!("-- module #{module_slot}: {}\n", module.sanitized_name),
        );
        push(
            &mut output,
            &mut next_line,
            &format!("__luck_modules[{module_slot}]=function(...)\n"),
        );
        if !body.ends_with('\n') {
            body.push('\n');
        }
        let body_start = next_line;
        push(&mut output, &mut next_line, &body);
        line_map.push(LineMapEntry {
            bundle_start_line: body_start,
            bundle_end_line: next_line - 1,
            path: module.path.clone(),
        });
        push(&mut output, &mut next_line, "end\n");
    }

    let entry = &modules[entry_id.0];
    let mut entry_body = transform_module_body(
        &entry.source,
        &entry.dependencies,
        &path_to_slot,
        version,
        entry.parsed_block.as_ref(),
    );
    // `output` always ends in '\n' here, so the old output-based newline
    // check reduces to: pad only a non-empty body missing its newline.
    if !entry_body.is_empty() && !entry_body.ends_with('\n') {
        entry_body.push('\n');
    }
    let body_start = next_line;
    push(&mut output, &mut next_line, &entry_body);
    line_map.push(LineMapEntry {
        bundle_start_line: body_start,
        bundle_end_line: next_line - 1,
        path: entry.path.clone(),
    });
    push(&mut output, &mut next_line, "end\n");

    (output, line_map)
}

/// Walk the ENTIRE parsed AST - every statement, expression, and
/// function body - collecting the span of every `require("...")` call
/// that resolved to a bundled module. String values come from the SAME
/// extractor the dependency scan used, so a require the graph resolved
/// can never be silently left behind in the output.
fn collect_require_replacements(
    block: &Block,
    path_to_slot: &std::collections::HashMap<&str, usize>,
    dependencies: &[(String, String, String, std::ops::Range<usize>)],
) -> Vec<(usize, usize, String)> {
    // require string -> loader slot
    let mut require_to_slot: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (_local_name, require_string, resolved_path, _) in dependencies {
        if let Some(&slot) = path_to_slot.get(resolved_path.as_str()) {
            require_to_slot.insert(require_string.clone(), slot);
        }
    }

    let mut finder = RequireCallFinder {
        require_to_slot: &require_to_slot,
        replacements: Vec::new(),
    };
    finder.visit_block(block);
    finder.replacements.sort_by_key(|r| r.0);
    finder.replacements
}

struct RequireCallFinder<'a> {
    require_to_slot: &'a std::collections::HashMap<String, usize>,
    replacements: Vec<(usize, usize, String)>,
}

impl RequireCallFinder<'_> {
    fn check_call(&mut self, call: &FunctionCall) {
        if let Some((require_string, call_span)) = extract_require_string(call)
            && let Some(&slot) = self.require_to_slot.get(&require_string)
        {
            self.replacements.push((
                call_span.start,
                call_span.end,
                format!("__luck_require({slot})"),
            ));
        }
    }
}

impl Visitor for RequireCallFinder<'_> {
    fn visit_expression(&mut self, expr: &Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.check_call(call);
        }
        self.walk_expression(expr);
    }

    fn visit_statement(&mut self, stmt: &Statement) {
        // Statement-level calls don't surface as Expression::FunctionCall.
        if let Statement::FunctionCall(call_stmt) = stmt {
            self.check_call(&call_stmt.call);
        }
        self.walk_statement(stmt);
    }
}

/// Replace require() calls with module variable references using source splicing.
/// The replacements are collected by walking the AST, ensuring correctness.
fn transform_module_body(
    source: &str,
    dependencies: &[(String, String, String, std::ops::Range<usize>)],
    path_to_slot: &std::collections::HashMap<&str, usize>,
    version: luck_token::LuaVersion,
    cached_block: Option<&Block>,
) -> String {
    let owned_parse_result;
    let block = match cached_block {
        Some(block) => block,
        None => {
            owned_parse_result = luck_parser::parse(source, version);
            &owned_parse_result.block
        }
    };
    let replacements = collect_require_replacements(block, path_to_slot, dependencies);

    if replacements.is_empty() {
        return source.to_string();
    }

    let mut result = String::with_capacity(source.len());
    let mut cursor = 0;

    for (start, end, name) in &replacements {
        let start = *start;
        let end = *end;
        if start > cursor && start <= source.len() {
            result.push_str(&source[cursor..start]);
        }
        result.push_str(name);
        cursor = end;
    }

    if cursor < source.len() {
        result.push_str(&source[cursor..]);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{ModuleId, ModuleInfo, sanitize_module_name};

    fn module(
        path: &str,
        source: &str,
        deps: Vec<(String, String, String, std::ops::Range<usize>)>,
    ) -> ModuleInfo {
        ModuleInfo {
            path: path.to_string(),
            source: source.to_string(),
            dependencies: deps,
            sanitized_name: sanitize_module_name(path),
            parsed_block: None,
        }
    }

    fn dep(
        require_string: &str,
        resolved: &str,
    ) -> (String, String, String, std::ops::Range<usize>) {
        (
            String::new(),
            require_string.to_string(),
            resolved.to_string(),
            0..0,
        )
    }

    fn reparses(output: &str) {
        let result = luck_parser::parse(output, luck_token::LuaVersion::Lua54);
        assert!(
            result.errors.is_empty(),
            "bundle must reparse: {:?}\n{output}",
            result.errors
        );
    }

    #[test]
    fn basic_bundle_uses_lazy_loader() {
        let modules = vec![
            module("src/utils.lua", "local M = {}\nreturn M\n", vec![]),
            module(
                "src/main.lua",
                "local utils = require(\"utils\")\nprint(utils)\n",
                vec![dep("utils", "src/utils.lua")],
            ),
        ];
        let graph = DependencyGraph {
            topo_order: vec![ModuleId(0), ModuleId(1)],
            entry_id: ModuleId(1),
            modules,
            warnings: vec![],
        };
        let output = emit(&graph, luck_token::LuaVersion::Lua54);
        assert!(
            output.contains("__luck_modules[1]=function(...)"),
            "{output}"
        );
        assert!(
            output.contains("local utils = __luck_require(1)"),
            "{output}"
        );
        assert!(
            output.contains("-- module #1: __luck_src_utils"),
            "{output}"
        );
        reparses(&output);
    }

    #[test]
    fn many_modules_use_table_slots_not_locals() {
        // The old emit created one `local` per module and broke past
        // Lua's 200-locals-per-function limit at ~199 modules.
        let mut modules = Vec::new();
        for i in 0..500 {
            modules.push(module(
                &format!("src/mod_{i}.lua"),
                &format!("return {i}\n"),
                vec![],
            ));
        }
        modules.push(module("src/main.lua", "print(\"hello\")\n", vec![]));
        let entry_idx = modules.len() - 1;
        let topo_order: Vec<ModuleId> = (0..modules.len()).map(ModuleId).collect();
        let graph = DependencyGraph {
            topo_order,
            entry_id: ModuleId(entry_idx),
            modules,
            warnings: vec![],
        };
        let output = emit(&graph, luck_token::LuaVersion::Lua54);
        assert!(
            output.contains("__luck_modules[500]=function(...)"),
            "all modules registered"
        );
        assert!(
            !output.contains("local __luck_src_mod"),
            "no per-module locals"
        );
        reparses(&output);
    }

    #[test]
    fn single_module_emits_verbatim() {
        let modules = vec![module("src/main.lua", "print(\"hello\")\n", vec![])];
        let graph = DependencyGraph {
            topo_order: vec![ModuleId(0)],
            entry_id: ModuleId(0),
            modules,
            warnings: vec![],
        };
        let output = emit(&graph, luck_token::LuaVersion::Lua54);
        assert_eq!(output, "print(\"hello\")\n");
    }

    #[test]
    fn typed_luau_require_is_rewritten() {
        // `local m = require("x") :: T` used to be silently dropped -
        // the raw require survived into the bundle and failed at runtime.
        let modules = vec![
            module("src/dep.luau", "return {}\n", vec![]),
            module(
                "src/main.luau",
                "local m = require(\"dep\") :: any\nprint(m)\n",
                vec![dep("dep", "src/dep.luau")],
            ),
        ];
        let graph = DependencyGraph {
            topo_order: vec![ModuleId(0), ModuleId(1)],
            entry_id: ModuleId(1),
            modules,
            warnings: vec![],
        };
        let output = emit(&graph, luck_token::LuaVersion::Luau);
        assert!(
            output.contains("local m = __luck_require(1) :: any"),
            "{output}"
        );
    }

    #[test]
    fn nested_and_field_access_requires_are_rewritten() {
        let source = "\
local field = require(\"dep\").value
local function lazy()
    return require(\"dep\")
end
if true then
    local inner = require(\"dep\")
    print(inner)
end
print(field, lazy())
";
        let modules = vec![
            module("src/dep.lua", "return { value = 1 }\n", vec![]),
            module("src/main.lua", source, vec![dep("dep", "src/dep.lua")]),
        ];
        let graph = DependencyGraph {
            topo_order: vec![ModuleId(0), ModuleId(1)],
            entry_id: ModuleId(1),
            modules,
            warnings: vec![],
        };
        let output = emit(&graph, luck_token::LuaVersion::Lua54);
        assert!(
            output.contains("local field = __luck_require(1).value"),
            "{output}"
        );
        assert!(output.contains("return __luck_require(1)"), "{output}");
        assert!(
            output.contains("local inner = __luck_require(1)"),
            "{output}"
        );
        assert!(
            !output.contains("require(\"dep\")"),
            "no raw require may survive: {output}"
        );
        reparses(&output);
    }

    #[test]
    fn require_inside_string_literal_untouched() {
        let source = "local d = require(\"dep\")\nlocal msg = 'use require(\"dep\") to load'\nprint(d, msg)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", vec![]),
            module("src/main.lua", source, vec![dep("dep", "src/dep.lua")]),
        ];
        let graph = DependencyGraph {
            topo_order: vec![ModuleId(0), ModuleId(1)],
            entry_id: ModuleId(1),
            modules,
            warnings: vec![],
        };
        let output = emit(&graph, luck_token::LuaVersion::Lua54);
        assert!(output.contains("local d = __luck_require(1)"), "{output}");
        assert!(
            output.contains("require(\"dep\") to load"),
            "string literal corrupted: {output}"
        );
    }

    fn find_line_entry<'a>(line_map: &'a [LineMapEntry], path: &str) -> &'a LineMapEntry {
        line_map
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("no line map entry for {path}"))
    }

    #[test]
    fn line_map_maps_module_lines_to_original_source_lines() {
        let utils_source = "local M = {}\nfunction M.foo()\n    return 42\nend\nreturn M\n";
        let entry_source = "local utils = require(\"utils\")\nlocal helper = require(\"helper\")\nprint(utils, helper)\n";
        // Leading comments and a blank line make source-line offsets nontrivial.
        let helper_source = "-- helper module\n-- second comment line\n\nlocal helper = { ready = true }\nreturn helper\n";
        let modules = vec![
            module("src/utils.lua", utils_source, vec![]),
            module("src/helper.lua", helper_source, vec![]),
            module(
                "src/main.lua",
                entry_source,
                vec![
                    dep("utils", "src/utils.lua"),
                    dep("helper", "src/helper.lua"),
                ],
            ),
        ];
        let graph = DependencyGraph {
            topo_order: vec![ModuleId(0), ModuleId(1), ModuleId(2)],
            entry_id: ModuleId(2),
            modules,
            warnings: vec![],
        };
        let (output, line_map) = emit_with_line_map(&graph, luck_token::LuaVersion::Lua54);
        let output_lines: Vec<&str> = output.lines().collect();

        let utils = find_line_entry(&line_map, "src/utils.lua");
        // Source line 1 lands on bundle_start_line; the module has 5 lines.
        assert_eq!(output_lines[utils.bundle_start_line - 1], "local M = {}");
        assert_eq!(output_lines[utils.bundle_start_line - 1 + 4], "return M");
        assert_eq!(
            utils.bundle_end_line - utils.bundle_start_line + 1,
            5,
            "utils body spans all 5 source lines"
        );
        // The two lines directly above a module body are its provenance
        // marker and the loader-function opener.
        assert!(
            output_lines[utils.bundle_start_line - 2].contains("=function(...)"),
            "{output}"
        );
        assert!(
            output_lines[utils.bundle_start_line - 3].starts_with("-- module #"),
            "{output}"
        );

        let helper = find_line_entry(&line_map, "src/helper.lua");
        // Nontrivial offset: the real declaration is on source line 4.
        assert_eq!(
            output_lines[helper.bundle_start_line - 1],
            "-- helper module"
        );
        assert_eq!(output_lines[helper.bundle_start_line - 1 + 2], "");
        assert_eq!(
            output_lines[helper.bundle_start_line - 1 + 3],
            "local helper = { ready = true }"
        );
        assert_eq!(helper.bundle_end_line - helper.bundle_start_line + 1, 5);

        let entry = find_line_entry(&line_map, "src/main.lua");
        // Entry is inlined verbatim (no wrapper), so line 1 is its first line.
        assert!(
            output_lines[entry.bundle_start_line - 1].contains("__luck_require"),
            "entry require rewritten: {output}"
        );
        assert_eq!(
            output_lines[entry.bundle_start_line - 1 + 2],
            "print(utils, helper)"
        );
        assert_eq!(entry.bundle_end_line - entry.bundle_start_line + 1, 3);

        // Ranges never overlap and are ordered utils, helper, entry.
        assert_eq!(line_map.len(), 3);
        assert!(utils.bundle_end_line < helper.bundle_start_line);
        assert!(helper.bundle_end_line < entry.bundle_start_line);
        reparses(&output);
    }

    #[test]
    fn line_map_single_module_covers_whole_source() {
        let source = "print(\"a\")\nprint(\"b\")\nprint(\"c\")\n";
        let modules = vec![module("src/main.lua", source, vec![])];
        let graph = DependencyGraph {
            topo_order: vec![ModuleId(0)],
            entry_id: ModuleId(0),
            modules,
            warnings: vec![],
        };
        let (output, line_map) = emit_with_line_map(&graph, luck_token::LuaVersion::Lua54);
        assert_eq!(output, source);
        assert_eq!(line_map.len(), 1);
        assert_eq!(
            line_map[0],
            LineMapEntry {
                bundle_start_line: 1,
                bundle_end_line: 3,
                path: "src/main.lua".to_string(),
            }
        );
    }

    #[test]
    fn long_bracket_require_is_rewritten() {
        // The emitter's own string parsing used to diverge from the
        // extractor's on long brackets, leaving `require [[dep]]` raw.
        let modules = vec![
            module("src/dep.lua", "return 1\n", vec![]),
            module(
                "src/main.lua",
                "local d = require [[dep]]\nprint(d)\n",
                vec![dep("dep", "src/dep.lua")],
            ),
        ];
        let graph = DependencyGraph {
            topo_order: vec![ModuleId(0), ModuleId(1)],
            entry_id: ModuleId(1),
            modules,
            warnings: vec![],
        };
        let output = emit(&graph, luck_token::LuaVersion::Lua54);
        assert!(output.contains("local d = __luck_require(1)"), "{output}");
        assert!(!output.contains("require [[dep]]"), "{output}");
    }
}
