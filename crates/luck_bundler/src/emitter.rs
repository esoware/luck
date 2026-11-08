use core::fmt::Write as _;
use core::ops::Range;

use luck_token::{LuaVersion, Span};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::exports::lower_exports;
use crate::graph::DependencyGraph;
use crate::hot_comments::leading_hot_comments;
use crate::loader::{dynamic_entry_text, loader_text};
use crate::module::{Dependency, ModuleInfo};

/// One contiguous run of bundle lines that came from a single module.
///
/// Line 1 of the module's source lands on `bundle_start_line`, so a
/// runtime traceback line `L` inside the range maps back to source line
/// `L - bundle_start_line + 1`. Require rewrites preserve the newline
/// count of the call they replace, so the correspondence is 1:1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMapEntry {
    /// 1-based first bundle line of the module body (inclusive).
    pub bundle_start_line: usize,
    /// 1-based last bundle line of the module body (inclusive).
    pub bundle_end_line: usize,
    /// The module's canonical path. The line map is local debug data, so it
    /// may name the host.
    pub path: String,
}

/// Generates the bundled Lua output from a resolved dependency graph, with
/// the module->bundle line map for it.
///
/// Modules register as functions in a table and load lazily on first
/// require, through a loader that mirrors the target version's own
/// `require` (`ll_require` in `loadlib.c`):
///
/// - Lua targets key the cache by module name and use `package.loaded`
///   itself as the cache when it exists, so two require strings reaching
///   one file run it twice, as in real Lua. `package.preload` wins over
///   bundled modules, chunks receive the module name (and on 5.2+ the
///   project-relative file path), a module that returns `false` reloads
///   on the next require, and 5.4+ returns the loader data as a second
///   result on first load.
/// - Luau keys the cache by resolved file, calls chunks with no
///   arguments, and errors unless a module returns exactly one non-nil
///   value, as Roblox and the Luau CLI do.
///
/// The entry body is wrapped in a function called with the chunk's
/// varargs, so CLI args pass straight through, and a module that requires
/// the entry gets a fresh instance, as in real Lua when the main file is
/// also required as a module.
///
/// Every generated identifier shares a prefix that appears nowhere in any
/// module source, so user code can't capture or shadow loader internals.
/// A lone module with no requires is emitted verbatim.
pub(crate) fn emit(graph: &DependencyGraph, version: LuaVersion) -> (String, Vec<LineMapEntry>) {
    let modules = &graph.modules;
    let Some(entry) = modules.get(graph.entry_id.0) else {
        return (String::new(), Vec::new());
    };

    if modules.len() == 1 && entry.dependencies.is_empty() {
        let line_map = vec![LineMapEntry {
            bundle_start_line: 1,
            bundle_end_line: entry.source.lines().count().max(1),
            path: entry.path.clone(),
        }];
        return (entry.source.clone(), line_map);
    }

    let prefix = choose_prefix(modules);

    // The sources plus rough room for each module's wrapper and the loader.
    let capacity = modules
        .iter()
        .map(|module| module.source.len().saturating_add(96))
        .sum::<usize>()
        .saturating_add(512);
    let mut writer = BundleWriter {
        output: String::with_capacity(capacity),
        next_line: 1,
        line_map: Vec::with_capacity(modules.len()),
        prefix: &prefix,
        version,
    };

    // Luau only honors hot comments before any code, so the entry's leading
    // ones go above the loader.
    if version.is_luau() {
        for (_, hot_comment) in leading_hot_comments(&entry.source) {
            writer.push(hot_comment);
            writer.push("\n");
        }
    }

    writer.push("do\n");
    writer.push(&loader_text(&prefix, version));
    // Only Lua targets record dynamic callees, so only a Lua loader gets the
    // entry point they're retargeted at.
    if modules.iter().any(|module| !module.dynamic_callees.is_empty()) {
        writer.push(&dynamic_entry_text(&prefix));
    }

    if version.is_luau() {
        writer.push_luau_modules(graph, entry);
    } else {
        writer.push_lua_modules(graph, entry);
    }

    writer.push(&format!("return {prefix}entry(...)\n"));
    writer.push("end\n");

    (writer.output, writer.line_map)
}

/// What a bundled require hands the loader to name its module.
#[derive(Clone, Copy)]
enum ModuleKeys<'slots> {
    /// Lua: the require string, as `package.loaded` keys its cache.
    RequireString,
    /// Luau: the numeric slot of the resolved file.
    Slot(&'slots FxHashMap<&'slots str, usize>),
}

impl ModuleKeys<'_> {
    /// `None` when the dependency's file has no slot, which leaves the call
    /// as written.
    fn key(self, dependency: &Dependency) -> Option<String> {
        match self {
            Self::RequireString => Some(quote_lua_string(&dependency.require_string)),
            Self::Slot(slots) => {
                slots.get(dependency.resolved_path.as_str()).map(ToString::to_string)
            }
        }
    }
}

/// The bundle being written, plus the bookkeeping behind its line map.
struct BundleWriter<'prefix> {
    output: String,
    /// 1-based line the next pushed character lands on.
    next_line: usize,
    line_map: Vec<LineMapEntry>,
    prefix: &'prefix str,
    version: LuaVersion,
}

impl BundleWriter<'_> {
    fn push(&mut self, fragment: &str) {
        self.output.push_str(fragment);
        self.next_line = self.next_line.saturating_add(fragment.matches('\n').count());
    }

    /// Luau caches per resolved file: one numeric slot per module, including
    /// the entry when something requires it.
    fn push_luau_modules(&mut self, graph: &DependencyGraph, entry: &ModuleInfo) {
        let prefix = self.prefix;
        let mut path_to_slot: FxHashMap<&str, usize> = (1usize..)
            .zip(non_entry_modules(graph))
            .map(|(slot, module)| (module.path.as_str(), slot))
            .collect();
        let is_entry_required = graph
            .modules
            .iter()
            .flat_map(|module| &module.dependencies)
            .any(|dependency| dependency.resolved_path == entry.path);
        if is_entry_required {
            path_to_slot.insert(entry.path.as_str(), path_to_slot.len().saturating_add(1));
        }
        let keys = ModuleKeys::Slot(&path_to_slot);

        for (slot, module) in (1usize..).zip(non_entry_modules(graph)) {
            self.push(&format!("-- module #{slot}: {}\n", module.relative_path));
            self.push(&format!("{prefix}modules[{slot}]=function(...)\n"));
            self.push_body(module, keys);
            self.push("end\n");
        }

        self.push(&format!("local {prefix}entry=function(...)\n"));
        self.push_body(entry, keys);
        self.push("end\n");
        if let Some(&entry_slot) = path_to_slot.get(entry.path.as_str()) {
            self.push(&format!("{prefix}modules[{entry_slot}]={prefix}entry\n"));
        }
    }

    /// Lua caches per module name: one slot per unique require string.
    /// Strings reaching the same file share its function but keep separate
    /// cache entries, so the file runs once per name, as with
    /// `package.loaded`.
    fn push_lua_modules(&mut self, graph: &DependencyGraph, entry: &ModuleInfo) {
        let prefix = self.prefix;
        let mut seen_strings: FxHashSet<&str> = FxHashSet::default();
        let mut strings_by_path: FxHashMap<&str, Vec<&str>> = FxHashMap::default();
        let ordered_dependencies = graph
            .topological_order
            .iter()
            .filter_map(|id| graph.modules.get(id.0))
            .flat_map(|module| &module.dependencies);
        for dependency in ordered_dependencies {
            if seen_strings.insert(dependency.require_string.as_str()) {
                strings_by_path
                    .entry(dependency.resolved_path.as_str())
                    .or_default()
                    .push(dependency.require_string.as_str());
            }
        }

        for module in non_entry_modules(graph) {
            let Some(strings) = strings_by_path.get(module.path.as_str()) else {
                continue;
            };
            let Some((primary, aliases)) = strings.split_first() else {
                continue;
            };
            let first = quote_lua_string(primary);
            self.push(&format!("-- module {first}: {}\n", module.relative_path));
            self.push(&format!("{prefix}modules[{first}]=function(...)\n"));
            self.push_body(module, ModuleKeys::RequireString);
            self.push("end\n");
            for alias in aliases {
                self.push(&format!(
                    "{prefix}modules[{}]={prefix}modules[{first}]\n",
                    quote_lua_string(alias)
                ));
            }
            self.push_path_registrations(strings, &module.relative_path);
        }

        self.push(&format!("local {prefix}entry=function(...)\n"));
        self.push_body(entry, ModuleKeys::RequireString);
        self.push("end\n");
        if let Some(entry_strings) = strings_by_path.get(entry.path.as_str()) {
            for string in entry_strings {
                self.push(&format!(
                    "{prefix}modules[{}]={prefix}entry\n",
                    quote_lua_string(string)
                ));
            }
            self.push_path_registrations(entry_strings, &entry.relative_path);
        }
    }

    fn push_path_registrations(&mut self, strings: &[&str], relative_path: &str) {
        if !self.version.has_require_filepath_arg() {
            return;
        }
        for string in strings {
            self.push(&format!(
                "{}paths[{}]={}\n",
                self.prefix,
                quote_lua_string(string),
                quote_lua_string(relative_path)
            ));
        }
    }

    /// Writes `module`'s source with a loader call spliced over each require
    /// the scan recorded, its dynamic requires retargeted, and its Luau
    /// `export` declarations lowered, and maps the lines it took.
    ///
    /// A loader call keeps the newline count of the call it replaces, so the
    /// line map stays 1:1 for multiline requires too.
    fn push_body(&mut self, module: &ModuleInfo, keys: ModuleKeys<'_>) {
        let source = module.source.as_str();
        let mut edits: Vec<(Span, String)> = module
            .dependencies
            .iter()
            .filter_map(|dependency| {
                let call = dependency.call_span.text(source);
                if call.is_empty() {
                    return None;
                }
                let newlines = "\n".repeat(call.matches('\n').count());
                let key = keys.key(dependency)?;
                Some((dependency.call_span, format!("{}require({key}{newlines})", self.prefix)))
            })
            .collect();

        // Only the callee token is retargeted, so a static require inside the
        // argument still gets its own splice.
        edits.extend(
            module
                .dynamic_callees
                .iter()
                .filter(|callee| !callee.text(source).is_empty())
                .map(|callee| (*callee, format!("{}dynamic", self.prefix))),
        );

        let value_exports = if self.version.has_value_exports() {
            let (export_edits, names) = lower_exports(&module.block);
            edits.extend(export_edits);
            names
        } else {
            Vec::new()
        };
        edits.sort_by_key(|(span, _)| span.start);

        let first_line = self.next_line;
        let body_start = self.output.len();
        let mut cursor = 0;
        for (span, replacement) in &edits {
            let range = Range::<usize>::from(*span);
            self.push(source.get(cursor..range.start).unwrap_or_default());
            self.push(replacement);
            cursor = range.end;
        }
        self.push(source.get(cursor..).unwrap_or_default());

        if !value_exports.is_empty() {
            if self.output.len() == body_start || !self.output.ends_with('\n') {
                self.push("\n");
            }
            let fields: Vec<String> =
                value_exports.iter().map(|name| format!("{name}={name}")).collect();
            self.push(&format!("return table.freeze({{{}}})\n", fields.join(",")));
        }
        if self.output.len() > body_start && !self.output.ends_with('\n') {
            self.push("\n");
        }

        self.line_map.push(LineMapEntry {
            bundle_start_line: first_line,
            bundle_end_line: self.next_line.saturating_sub(1),
            path: module.path.clone(),
        });
    }
}

/// Every module but the entry, in the order the bundle registers them.
fn non_entry_modules(graph: &DependencyGraph) -> impl Iterator<Item = &ModuleInfo> {
    graph
        .topological_order
        .iter()
        .filter(|id| **id != graph.entry_id)
        .filter_map(|id| graph.modules.get(id.0))
}

/// Prefix for every generated name, picked so it appears in no module
/// source.
fn choose_prefix(modules: &[ModuleInfo]) -> String {
    let mut candidate = "__luck_".to_owned();
    let mut counter = 0usize;
    while modules.iter().any(|module| module.source.contains(&candidate)) {
        counter = counter.saturating_add(1);
        candidate = format!("__luck{counter}_");
    }
    candidate
}

/// A double-quoted Lua string literal that decodes to exactly `value`
/// in every supported version. Bytes outside printable ASCII use
/// zero-padded decimal escapes so a following digit can't extend them.
fn quote_lua_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len().saturating_add(2));
    quoted.push('"');
    for byte in value.bytes() {
        match byte {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            0x20..=0x7e => quoted.push(char::from(byte)),
            _ => {
                let _unused = write!(quoted, "\\{byte:03}");
            }
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use luck_core::DynamicRequire;

    use super::*;
    use crate::module::ModuleId;
    use crate::require_extraction::extract_requires;

    /// Builds a `ModuleInfo` with the real require extraction, resolving
    /// each extracted string through `resolutions`.
    fn module(
        path: &str,
        source: &str,
        resolutions: &[(&str, &str)],
        version: LuaVersion,
    ) -> ModuleInfo {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "fixture must parse: {:?}", parsed.errors);
        let extracted = extract_requires(&parsed.block, path, version, DynamicRequire::default());
        let dependencies = extracted
            .sites
            .into_iter()
            .map(|site| {
                let resolved = resolutions
                    .iter()
                    .find(|(require_string, _)| *require_string == site.require_string)
                    .unwrap_or_else(|| panic!("no resolution for {:?}", site.require_string));
                Dependency {
                    require_string: site.require_string,
                    resolved_path: resolved.1.to_owned(),
                    call_span: site.call_span,
                }
            })
            .collect();
        ModuleInfo {
            path: path.to_owned(),
            source: source.to_owned(),
            dependencies,
            relative_path: path.to_owned(),
            dynamic_callees: extracted.dynamic_callees,
            block: parsed.block,
        }
    }

    /// The graph of `modules`, given in topological order with the last one as
    /// entry, laid out as `DependencyGraph::build` lays it out, entry first.
    fn graph(mut modules: Vec<ModuleInfo>) -> DependencyGraph {
        modules.rotate_right(1);
        let topological_order = (1..modules.len()).chain([0]).map(ModuleId).collect();
        DependencyGraph { modules, topological_order, entry_id: ModuleId(0), warnings: vec![] }
    }

    /// The bundle for `modules`, with the last one as entry.
    fn bundle(modules: Vec<ModuleInfo>, version: LuaVersion) -> String {
        emit(&graph(modules), version).0
    }

    fn assert_reparses(output: &str, version: LuaVersion) {
        let result = luck_parser::parse(output, version);
        assert!(result.errors.is_empty(), "bundle must reparse: {:?}\n{output}", result.errors);
    }

    #[test]
    fn basic_bundle_uses_lazy_loader() {
        let modules = vec![
            module("src/utils.lua", "local M = {}\nreturn M\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                "local utils = require(\"utils\")\nprint(utils)\n",
                &[("utils", "src/utils.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(output.contains("__luck_modules[\"utils\"]=function(...)"), "{output}");
        assert!(output.contains("local utils = __luck_require(\"utils\")"), "{output}");
        assert!(output.contains("-- module \"utils\": src/utils.lua"), "{output}");
        assert!(
            output.contains("__luck_paths[\"utils\"]=\"src/utils.lua\""),
            "5.4 chunks receive the file path: {output}"
        );
        assert!(output.contains("local __luck_entry=function(...)"), "{output}");
        assert!(output.contains("return __luck_entry(...)"), "{output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn many_modules_use_table_slots_not_locals() {
        // One `local` per module would break past Lua's
        // 200-locals-per-function limit at ~199 modules.
        let mut modules = Vec::new();
        let mut entry_source = String::new();
        let mut resolutions: Vec<(String, String)> = Vec::new();
        for i in 0..500usize {
            modules.push(module(
                &format!("src/mod_{i}.lua"),
                &format!("return {i}\n"),
                &[],
                LuaVersion::Lua54,
            ));
            let _unused = writeln!(entry_source, "require(\"mod_{i}\")");
            resolutions.push((format!("mod_{i}"), format!("src/mod_{i}.lua")));
        }
        let borrowed_resolutions: Vec<(&str, &str)> = resolutions
            .iter()
            .map(|(require_string, path)| (require_string.as_str(), path.as_str()))
            .collect();
        modules.push(module(
            "src/main.lua",
            &entry_source,
            &borrowed_resolutions,
            LuaVersion::Lua54,
        ));
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(
            output.contains("__luck_modules[\"mod_499\"]=function(...)"),
            "all modules registered"
        );
        assert!(!output.contains("local __luck_src_mod"), "no per-module locals");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn required_entry_registers_under_its_require_string() {
        // The entry function registers under the require string, so a
        // module requiring the entry leaves no raw require() in the
        // output. The extra load runs a fresh instance, as real Lua does
        // when the main file is required a second time as a module.
        let modules = vec![
            module(
                "src/b.lua",
                "local a = require(\"a\")\nreturn { a = a }\n",
                &[("a", "src/a.lua")],
                LuaVersion::Lua54,
            ),
            module(
                "src/a.lua",
                "local b = require(\"b\")\nreturn { b = b }\n",
                &[("b", "src/b.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(output.contains("__luck_modules[\"a\"]=__luck_entry"), "{output}");
        assert_eq!(
            output.matches("require(\"a\")").count(),
            output.matches("__luck_require(\"a\")").count(),
            "no raw require may survive: {output}"
        );
        assert!(output.contains("__luck_require(\"a\")"), "{output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn two_require_strings_for_one_file_get_separate_slots() {
        // Real Lua caches by name, so require("foo") and require("foo.init")
        // reaching one file run it twice. The slots share one function but
        // not a cache entry.
        let source = "local a = require(\"foo\")\nlocal b = require(\"foo.init\")\nprint(a, b)\n";
        let modules = vec![
            module("src/foo/init.lua", "return {}\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                source,
                &[("foo", "src/foo/init.lua"), ("foo.init", "src/foo/init.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(output.contains("__luck_modules[\"foo\"]=function(...)"), "{output}");
        assert!(
            output.contains("__luck_modules[\"foo.init\"]=__luck_modules[\"foo\"]"),
            "{output}"
        );
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn generated_prefix_avoids_user_identifiers() {
        let source =
            "local __luck_require = 1\nlocal d = require(\"dep\")\nprint(__luck_require, d)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module("src/main.lua", source, &[("dep", "src/dep.lua")], LuaVersion::Lua54),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(
            output.contains("local function __luck1_require"),
            "loader must rename itself: {output}"
        );
        assert!(output.contains("local d = __luck1_require(\"dep\")"), "{output}");
        assert!(output.contains("local __luck_require = 1"), "user identifier untouched: {output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn multiline_require_preserves_line_count() {
        let source = "local d = require(\n\"dep\"\n)\nprint(d)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module("src/main.lua", source, &[("dep", "src/dep.lua")], LuaVersion::Lua54),
        ];
        let (output, line_map) = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("local d = __luck_require(\"dep\"\n\n)"),
            "padding keeps following lines in place: {output}"
        );
        let entry = find_line_entry(&line_map, "src/main.lua");
        let output_lines: Vec<&str> = output.lines().collect();
        // Source line 4 is print(d); it must land exactly 3 lines below
        // the entry body start despite the rewritten multiline call.
        assert_eq!(output_lines[entry.bundle_start_line - 1 + 3], "print(d)");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn lua51_loader_uses_sentinel_and_no_paths() {
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua51),
            module(
                "src/main.lua",
                "local d = require(\"dep\")\nprint(d)\n",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua51,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua51);
        assert!(output.contains("loop or previous error loading module"), "{output}");
        assert!(
            !output.contains("__luck_paths"),
            "5.1 chunks receive only the module name: {output}"
        );
        assert_reparses(&output, LuaVersion::Lua51);
    }

    #[test]
    fn lua52_loader_retries_after_error_and_passes_path() {
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua52),
            module(
                "src/main.lua",
                "local d = require(\"dep\")\nprint(d)\n",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua52,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua52);
        assert!(
            !output.contains("loop or previous error"),
            "5.2+ has no sentinel; failed loads retry: {output}"
        );
        assert!(
            output.contains("(name,__luck_paths[name])"),
            "5.2+ chunks receive (name, path): {output}"
        );
        assert!(!output.contains(":preload:"), "loaderdata second result is 5.4+ only: {output}");
        assert_reparses(&output, LuaVersion::Lua52);
    }

    #[test]
    fn lua54_loader_returns_loaderdata() {
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                "local d = require(\"dep\")\nprint(d)\n",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(output.contains(":preload:"), "{output}");
        assert!(output.contains("return value,loaderdata"), "{output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn luau_loader_checks_single_return_and_uses_numeric_slots() {
        let modules = vec![
            module("src/dep.luau", "return {}\n", &[], LuaVersion::Luau),
            module(
                "src/main.luau",
                "local m = require(\"./dep\") :: any\nprint(m)\n",
                &[("./dep", "src/dep.luau")],
                LuaVersion::Luau,
            ),
        ];
        let output = bundle(modules, LuaVersion::Luau);
        assert!(output.contains("local m = __luck_require(1) :: any"), "{output}");
        assert!(
            output.contains("select(\"#\",...)~=1"),
            "Luau modules must return exactly one value: {output}"
        );
        assert!(!output.contains("package.loaded"), "Luau has no package table: {output}");
        assert_reparses(&output, LuaVersion::Luau);
    }

    #[test]
    fn shadowed_require_call_survives_unrewritten() {
        let source = "local d = require(\"dep\")\nlocal function f()\n    local require = print\n    require(\"dep\")\nend\nf()\nprint(d)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module("src/main.lua", source, &[("dep", "src/dep.lua")], LuaVersion::Lua54),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(output.contains("local d = __luck_require(\"dep\")"), "{output}");
        assert!(
            output.contains("    require(\"dep\")"),
            "shadowed call keeps user semantics: {output}"
        );
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn luau_value_exports_are_lowered_inside_bundle_wrappers() {
        let dependency = "\
export type Public = integer
export local value = 129312i
export const LIMIT = 0xffffffffffffffffi
@native
export function get(): integer
    return value
end
";
        let modules = vec![
            module("src/dep.luau", dependency, &[], LuaVersion::Luau),
            module(
                "src/main.luau",
                "local dep = require(\"./dep\")\nprint(dep.get())\n",
                &[("./dep", "src/dep.luau")],
                LuaVersion::Luau,
            ),
        ];
        let output = bundle(modules, LuaVersion::Luau);

        assert!(!output.contains("export "), "{output}");
        assert!(output.contains("local value = 129312i"), "{output}");
        assert!(output.contains("const LIMIT = 0xffffffffffffffffi"), "{output}");
        assert!(output.contains("@native\nlocal function get()"), "{output}");
        assert!(
            output.contains("return table.freeze({value=value,LIMIT=LIMIT,get=get})"),
            "{output}"
        );
        assert_reparses(&output, LuaVersion::Luau);
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
            module("src/dep.lua", "return { value = 1 }\n", &[], LuaVersion::Lua54),
            module("src/main.lua", source, &[("dep", "src/dep.lua")], LuaVersion::Lua54),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(output.contains("local field = __luck_require(\"dep\").value"), "{output}");
        assert!(output.contains("return __luck_require(\"dep\")"), "{output}");
        assert!(output.contains("local inner = __luck_require(\"dep\")"), "{output}");
        assert!(!output.contains("= require(\"dep\")"), "no raw require may survive: {output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn require_inside_string_literal_is_untouched() {
        let source = "local d = require(\"dep\")\nlocal msg = 'use require(\"dep\") to load'\nprint(d, msg)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module("src/main.lua", source, &[("dep", "src/dep.lua")], LuaVersion::Lua54),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(output.contains("local d = __luck_require(\"dep\")"), "{output}");
        assert!(output.contains("require(\"dep\") to load"), "string literal corrupted: {output}");
    }

    #[test]
    fn escaped_require_string_is_rewritten_to_decoded_name() {
        // require("a\46b") is require("a.b"); both spellings share one
        // cache entry keyed by the decoded name.
        let source = "local x = require(\"a\\46b\")\nlocal y = require(\"a.b\")\nprint(x, y)\n";
        let modules = vec![
            module("src/a/b.lua", "return {}\n", &[], LuaVersion::Lua54),
            module("src/main.lua", source, &[("a.b", "src/a/b.lua")], LuaVersion::Lua54),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert_eq!(output.matches("__luck_require(\"a.b\")").count(), 2, "{output}");
        assert!(output.contains("__luck_modules[\"a.b\"]=function(...)"), "{output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    fn find_line_entry<'map>(line_map: &'map [LineMapEntry], path: &str) -> &'map LineMapEntry {
        line_map
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("no line map entry for {path}"))
    }

    #[test]
    fn line_map_maps_module_lines_to_original_source_lines() {
        let utils_source = "local M = {}\nfunction M.foo()\n    return 42\nend\nreturn M\n";
        let entry_source = "local utils = require(\"utils\")\nlocal helper = require(\"helper\")\nprint(utils, helper)\n";
        // Leading comments and a blank line make the offsets nontrivial.
        let helper_source = "-- helper module\n-- second comment line\n\nlocal helper = { ready = true }\nreturn helper\n";
        let modules = vec![
            module("src/utils.lua", utils_source, &[], LuaVersion::Lua54),
            module("src/helper.lua", helper_source, &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                entry_source,
                &[("utils", "src/utils.lua"), ("helper", "src/helper.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let (output, line_map) = emit(&graph(modules), LuaVersion::Lua54);
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
        assert!(output_lines[utils.bundle_start_line - 2].contains("=function(...)"), "{output}");
        assert!(output_lines[utils.bundle_start_line - 3].starts_with("-- module "), "{output}");

        let helper = find_line_entry(&line_map, "src/helper.lua");
        // Nontrivial offset: the real declaration is on source line 4.
        assert_eq!(output_lines[helper.bundle_start_line - 1], "-- helper module");
        assert_eq!(output_lines[helper.bundle_start_line - 1 + 2], "");
        assert_eq!(
            output_lines[helper.bundle_start_line - 1 + 3],
            "local helper = { ready = true }"
        );
        assert_eq!(helper.bundle_end_line - helper.bundle_start_line + 1, 5);

        let entry = find_line_entry(&line_map, "src/main.lua");
        // The entry body sits inside its wrapper function; line 1 is
        // still its first source line.
        assert!(
            output_lines[entry.bundle_start_line - 1].contains("__luck_require"),
            "entry require rewritten: {output}"
        );
        assert!(
            output_lines[entry.bundle_start_line - 2].contains("__luck_entry=function(...)"),
            "{output}"
        );
        assert_eq!(output_lines[entry.bundle_start_line - 1 + 2], "print(utils, helper)");
        assert_eq!(entry.bundle_end_line - entry.bundle_start_line + 1, 3);

        // Ranges never overlap and are ordered utils, helper, entry.
        assert_eq!(line_map.len(), 3);
        assert!(utils.bundle_end_line < helper.bundle_start_line);
        assert!(helper.bundle_end_line < entry.bundle_start_line);
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn lone_module_emits_verbatim_and_maps_every_line() {
        let source = "print(\"a\")\nprint(\"b\")\nprint(\"c\")\n";
        let modules = vec![module("src/main.lua", source, &[], LuaVersion::Lua54)];
        let (output, line_map) = emit(&graph(modules), LuaVersion::Lua54);
        assert_eq!(output, source);
        assert_eq!(
            line_map,
            [LineMapEntry {
                bundle_start_line: 1,
                bundle_end_line: 3,
                path: "src/main.lua".to_owned(),
            }]
        );
    }

    #[test]
    fn long_bracket_require_is_rewritten() {
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                "local d = require [[dep]]\nprint(d)\n",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(output.contains("local d = __luck_require(\"dep\")"), "{output}");
        assert!(!output.contains("require [[dep]]"), "{output}");
    }

    #[test]
    fn quote_lua_string_escapes_specials_and_zero_pads_unprintable_bytes() {
        assert_eq!(quote_lua_string("plain"), "\"plain\"");
        assert_eq!(quote_lua_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_lua_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote_lua_string("a\nb"), "\"a\\nb\"");
        // Zero-padded decimal escapes can never absorb a following digit.
        assert_eq!(quote_lua_string("\u{7}5"), "\"\\0075\"");
    }
}
