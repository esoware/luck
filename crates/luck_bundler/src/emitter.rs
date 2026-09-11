use luck_ast::shared::Block;
use luck_token::LuaVersion;
use rustc_hash::{FxHashMap, FxHashSet};
use std::ops::Range;

use crate::graph::DependencyGraph;
use crate::module::{Dependency, ModuleId, ModuleInfo};

/// One contiguous run of bundle lines that came from a single module.
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
    pub path: String,
}

/// Generates the bundled Lua output from a resolved dependency graph.
///
/// Modules register as functions in a table and load LAZILY on first
/// require, through a loader that mirrors the target version's own
/// `require` implementation (see `loadlib.c` `ll_require`):
///
/// - Lua targets key the cache by module NAME (like `package.loaded`,
///   which backs the cache directly when available), so two require
///   strings reaching one file execute it twice, exactly like real Lua.
///   `package.preload` entries win over bundled modules, chunks receive
///   the module name (5.2+ also the file path), a `false` return is
///   reloaded on the next require, and 5.4+ returns the loader data as
///   a second result on first load.
/// - Luau keys the cache by resolved file, calls chunks with no
///   arguments, and errors unless a module returns exactly one non-nil
///   value, matching Roblox and the Luau CLI.
///
/// The entry module body is wrapped in a function invoked with the
/// chunk's varargs, so CLI args flow through unchanged and a module
/// requiring the entry re-executes it as a fresh instance, the same
/// thing real Lua does when a file doubles as main chunk and module.
///
/// Every generated identifier shares one prefix chosen to appear
/// nowhere in any module source, so user code can never capture or
/// shadow loader internals.
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

    if modules.len() == 1 && modules[entry_id.0].dependencies.is_empty() {
        let entry = &modules[entry_id.0];
        let line_map = vec![LineMapEntry {
            bundle_start_line: 1,
            bundle_end_line: entry.source.lines().count().max(1),
            path: entry.path.clone(),
        }];
        return (entry.source.clone(), line_map);
    }

    let prefix = choose_prefix(modules);
    let path_to_id: FxHashMap<&str, ModuleId> = modules
        .iter()
        .enumerate()
        .map(|(idx, module)| (module.path.as_str(), ModuleId(idx)))
        .collect();

    // Bundle size is roughly the module sources plus the loader and a
    // small per-module wrapper; reserving it up front avoids realloc
    // copies of the whole bundle.
    let source_total: usize = modules.iter().map(|module| module.source.len()).sum();
    let mut output = String::with_capacity(source_total + 512 + 96 * modules.len());
    let mut line_map: Vec<LineMapEntry> = Vec::with_capacity(modules.len());
    // 1-based line the NEXT pushed character lands on; every push below
    // ends on a newline, so this stays exact.
    let mut next_line = 1usize;
    let push = |output: &mut String, next_line: &mut usize, fragment: &str| {
        output.push_str(fragment);
        *next_line += fragment.matches('\n').count();
    };

    // Luau hot comments only apply before any code. Hoisting the entry
    // module's leading run above the loader keeps their effect.
    if version.is_luau() {
        let entry_module = &modules[entry_id.0];
        for line in entry_module.source.lines() {
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("--") {
                if rest.starts_with('!') {
                    push(&mut output, &mut next_line, trimmed);
                    push(&mut output, &mut next_line, "\n");
                }
                continue;
            }
            break;
        }
    }

    let has_dynamic_require = modules
        .iter()
        .any(|module| !module.dynamic_callees.is_empty());

    push(&mut output, &mut next_line, "do\n");
    push(
        &mut output,
        &mut next_line,
        &loader_text(&prefix, version, has_dynamic_require),
    );

    let emit_body = |output: &mut String,
                     next_line: &mut usize,
                     line_map: &mut Vec<LineMapEntry>,
                     module: &ModuleInfo,
                     replacement: &dyn Fn(&Dependency, usize) -> String| {
        let mut body = transform_module_body(
            &module.source,
            &module.dependencies,
            &module.dynamic_callees,
            &prefix,
            version,
            module.parsed_block.as_ref(),
            replacement,
        );
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        let body_start = *next_line;
        push(output, next_line, &body);
        line_map.push(LineMapEntry {
            bundle_start_line: body_start,
            bundle_end_line: (*next_line).saturating_sub(1),
            path: module.path.clone(),
        });
    };

    if version.is_luau() {
        // Luau caches per resolved file: one numeric slot per module,
        // including the entry when something requires it.
        let mut path_to_slot: FxHashMap<&str, usize> = FxHashMap::default();
        let mut slot = 0usize;
        for id in topo_order {
            if *id == entry_id {
                continue;
            }
            slot += 1;
            path_to_slot.insert(modules[id.0].path.as_str(), slot);
        }
        let entry_required = modules
            .iter()
            .flat_map(|module| &module.dependencies)
            .any(|dep| dep.resolved_path == modules[entry_id.0].path);
        if entry_required {
            slot += 1;
            path_to_slot.insert(modules[entry_id.0].path.as_str(), slot);
        }

        let replacement = |dep: &Dependency, newlines: usize| {
            let slot = path_to_slot[dep.resolved_path.as_str()];
            format!("{prefix}require({slot}{})", "\n".repeat(newlines))
        };

        for id in topo_order {
            if *id == entry_id {
                continue;
            }
            let module = &modules[id.0];
            let module_slot = path_to_slot[module.path.as_str()];
            push(
                &mut output,
                &mut next_line,
                &format!("-- module #{module_slot}: {}\n", module.relative_path),
            );
            push(
                &mut output,
                &mut next_line,
                &format!("{prefix}modules[{module_slot}]=function(...)\n"),
            );
            emit_body(
                &mut output,
                &mut next_line,
                &mut line_map,
                module,
                &replacement,
            );
            push(&mut output, &mut next_line, "end\n");
        }

        let entry = &modules[entry_id.0];
        push(
            &mut output,
            &mut next_line,
            &format!("local {prefix}entry=function(...)\n"),
        );
        emit_body(
            &mut output,
            &mut next_line,
            &mut line_map,
            entry,
            &replacement,
        );
        push(&mut output, &mut next_line, "end\n");
        if let Some(&entry_slot) = path_to_slot.get(entry.path.as_str()) {
            push(
                &mut output,
                &mut next_line,
                &format!("{prefix}modules[{entry_slot}]={prefix}entry\n"),
            );
        }
    } else {
        // Lua caches per module NAME: one slot per unique require
        // string, in first-discovery order. Strings reaching the same
        // file share its function but keep separate cache entries, so
        // the file executes once per name, exactly like package.loaded.
        let mut seen: FxHashSet<&str> = FxHashSet::default();
        let mut slots: Vec<(&str, ModuleId)> = Vec::new();
        for id in topo_order {
            for dep in &modules[id.0].dependencies {
                if seen.insert(dep.require_string.as_str()) {
                    slots.push((
                        dep.require_string.as_str(),
                        path_to_id[dep.resolved_path.as_str()],
                    ));
                }
            }
        }
        let mut strings_by_module: FxHashMap<usize, Vec<&str>> = FxHashMap::default();
        for (string, target) in &slots {
            strings_by_module.entry(target.0).or_default().push(string);
        }

        let has_paths = version.has_require_filepath_arg();
        let replacement = |dep: &Dependency, newlines: usize| {
            format!(
                "{prefix}require({}{})",
                quote_lua_string(&dep.require_string),
                "\n".repeat(newlines)
            )
        };
        let register_paths =
            |output: &mut String, next_line: &mut usize, strings: &[&str], relative_path: &str| {
                if !has_paths {
                    return;
                }
                for string in strings {
                    push(
                        output,
                        next_line,
                        &format!(
                            "{prefix}paths[{}]={}\n",
                            quote_lua_string(string),
                            quote_lua_string(relative_path)
                        ),
                    );
                }
            };

        for id in topo_order {
            if *id == entry_id {
                continue;
            }
            let module = &modules[id.0];
            let Some(strings) = strings_by_module.get(&id.0) else {
                continue;
            };
            let first = quote_lua_string(strings[0]);
            push(
                &mut output,
                &mut next_line,
                &format!("-- module {first}: {}\n", module.relative_path),
            );
            push(
                &mut output,
                &mut next_line,
                &format!("{prefix}modules[{first}]=function(...)\n"),
            );
            emit_body(
                &mut output,
                &mut next_line,
                &mut line_map,
                module,
                &replacement,
            );
            push(&mut output, &mut next_line, "end\n");
            for alias in &strings[1..] {
                push(
                    &mut output,
                    &mut next_line,
                    &format!(
                        "{prefix}modules[{}]={prefix}modules[{first}]\n",
                        quote_lua_string(alias)
                    ),
                );
            }
            register_paths(&mut output, &mut next_line, strings, &module.relative_path);
        }

        let entry = &modules[entry_id.0];
        push(
            &mut output,
            &mut next_line,
            &format!("local {prefix}entry=function(...)\n"),
        );
        emit_body(
            &mut output,
            &mut next_line,
            &mut line_map,
            entry,
            &replacement,
        );
        push(&mut output, &mut next_line, "end\n");
        if let Some(entry_strings) = strings_by_module.get(&entry_id.0) {
            for string in entry_strings {
                push(
                    &mut output,
                    &mut next_line,
                    &format!(
                        "{prefix}modules[{}]={prefix}entry\n",
                        quote_lua_string(string)
                    ),
                );
            }
            register_paths(
                &mut output,
                &mut next_line,
                entry_strings,
                &entry.relative_path,
            );
        }
    }

    push(
        &mut output,
        &mut next_line,
        &format!("return {prefix}entry(...)\n"),
    );
    push(&mut output, &mut next_line, "end\n");

    (output, line_map)
}

/// Identifier prefix for every generated name, guaranteed absent from
/// all module sources so user code can never collide with the loader.
fn choose_prefix(modules: &[ModuleInfo]) -> String {
    let mut candidate = "__luck_".to_string();
    let mut counter = 0usize;
    while modules
        .iter()
        .any(|module| module.source.contains(&candidate))
    {
        counter += 1;
        candidate = format!("__luck{counter}_");
    }
    candidate
}

/// A double-quoted Lua string literal that decodes to exactly `value`
/// in every supported version. Non-printable bytes use zero-padded
/// decimal escapes so a following digit can never extend them.
fn quote_lua_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for byte in value.bytes() {
        match byte {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            0x20..=0x7e => quoted.push(byte as char),
            _ => {
                quoted.push('\\');
                quoted.push_str(&format!("{byte:03}"));
            }
        }
    }
    quoted.push('"');
    quoted
}

/// The memoizing loader emitted at the top of every multi-module
/// bundle, mirroring the target version's `ll_require` exactly.
///
/// `has_dynamic_require` adds the entry point that `require(expr)` calls are
/// retargeted at; bundles without one carry no extra bytes.
fn loader_text(prefix: &str, version: LuaVersion, has_dynamic_require: bool) -> String {
    if version.is_luau() {
        // Luau/Roblox require: private cache keyed by file, cycles and
        // repeated failures raise, modules must return exactly one
        // non-nil value, chunks receive no arguments.
        return format!(
            "local {p}modules={{}}\n\
             local {p}loaded={{}}\n\
             local {p}loading={{}}\n\
             local function {p}result(...)\n\
             if select(\"#\",...)~=1 or (...)==nil then\n\
             error(\"luck bundle: module must return exactly one non-nil value\",0)\n\
             end\n\
             return ...\n\
             end\n\
             local function {p}require(id)\n\
             local value={p}loaded[id]\n\
             if value~=nil then\n\
             if value=={p}loading then error(\"luck bundle: require cycle hit while loading module #\"..id,2)end\n\
             return value\n\
             end\n\
             {p}loaded[id]={p}loading\n\
             value={p}result({p}modules[id]())\n\
             {p}loaded[id]=value\n\
             return value\n\
             end\n",
            p = prefix
        );
    }

    // Lua: the cache IS package.loaded when available, keyed by module
    // name, and package.preload wins over bundled files, giving the same
    // observable state and searcher order as real require. The
    // truthiness check makes a module that returned false reload on the
    // next require, as in every PUC version.
    let mut loader = format!(
        "local {p}modules={{}}\n\
         local {p}loaded=type(package)==\"table\" and type(package.loaded)==\"table\" and package.loaded or {{}}\n\
         local {p}preload=type(package)==\"table\" and type(package.preload)==\"table\" and package.preload or {{}}\n",
        p = prefix
    );
    if version.has_require_filepath_arg() {
        loader.push_str(&format!("local {p}paths={{}}\n", p = prefix));
    }

    if !version.has_require_error_retry() {
        // 5.1: a sentinel marks in-progress loads; hitting it (a cycle,
        // or a module whose load previously raised) reproduces 5.1's
        // "loop or previous error" failure.
        loader.push_str(&format!(
            "local {p}loading={{}}\n\
             local function {p}require(name)\n\
             local value={p}loaded[name]\n\
             if value then\n\
             if value=={p}loading then error(\"loop or previous error loading module '\"..name..\"'\",2)end\n\
             return value\n\
             end\n\
             local loader={p}preload[name] or {p}modules[name]\n\
             {p}loaded[name]={p}loading\n\
             value=loader(name)\n\
             if value~=nil then {p}loaded[name]=value end\n\
             value={p}loaded[name]\n\
             if value=={p}loading then\n\
             value=true\n\
             {p}loaded[name]=value\n\
             end\n\
             return value\n\
             end\n",
            p = prefix
        ));
    } else if !version.has_require_loaderdata() {
        // 5.2/5.3: no sentinel. A load-time cycle recurses until the
        // stack overflows, and an error during load leaves the cache
        // unset so a later require retries, both exactly as in real
        // 5.2+.
        loader.push_str(&format!(
            "local function {p}require(name)\n\
             local value={p}loaded[name]\n\
             if value then return value end\n\
             local preloader={p}preload[name]\n\
             if preloader then\n\
             value=preloader(name)\n\
             else\n\
             value={p}modules[name](name,{p}paths[name])\n\
             end\n\
             if value~=nil then {p}loaded[name]=value end\n\
             value={p}loaded[name]\n\
             if value==nil then\n\
             value=true\n\
             {p}loaded[name]=value\n\
             end\n\
             return value\n\
             end\n",
            p = prefix
        ));
    } else {
        // 5.4/5.5: additionally return the loader data as a second
        // result on first load only (cache hits return one value).
        loader.push_str(&format!(
            "local function {p}require(name)\n\
             local value={p}loaded[name]\n\
             if value then return value end\n\
             local loaderdata\n\
             local loader={p}preload[name]\n\
             if loader then\n\
             loaderdata=\":preload:\"\n\
             else\n\
             loader={p}modules[name]\n\
             loaderdata={p}paths[name]\n\
             end\n\
             value=loader(name,loaderdata)\n\
             if value~=nil then {p}loaded[name]=value end\n\
             value={p}loaded[name]\n\
             if value==nil then\n\
             value=true\n\
             {p}loaded[name]=value\n\
             end\n\
             return value,loaderdata\n\
             end\n",
            p = prefix
        ));
    }

    if has_dynamic_require {
        // A name computed at runtime can still be a bundled one - the Lua
        // cache is keyed by module name - so try the bundle first and fall
        // through to the host's own require, read at call time so a `require`
        // the host installs later is still honored.
        loader.push_str(&format!(
            "local function {p}dynamic(name)\n\
             if {p}modules[name]~=nil or {p}preload[name]~=nil then return {p}require(name)end\n\
             if type(require)==\"function\" then return require(name)end\n\
             error(\"luck bundle: dynamic require of '\"..tostring(name)..\"' is not in the bundle and this runtime has no require\",2)\n\
             end\n",
            p = prefix
        ));
    }

    loader
}

/// Replace require() calls with loader calls by splicing over the exact
/// byte ranges the dependency scan recorded, and lower Luau `export`
/// declarations. Replacements carry the newline count of the call they
/// replace, keeping the line map 1:1 even for multiline requires.
fn transform_module_body(
    source: &str,
    dependencies: &[Dependency],
    dynamic_callees: &[Range<usize>],
    prefix: &str,
    version: luck_token::LuaVersion,
    cached_block: Option<&Block>,
    replacement: &dyn Fn(&Dependency, usize) -> String,
) -> String {
    let mut replacements: Vec<(usize, usize, String)> = dependencies
        .iter()
        .filter(|dep| dep.call_span.end <= source.len() && dep.call_span.start < dep.call_span.end)
        .map(|dep| {
            let newlines = source[dep.call_span.clone()].matches('\n').count();
            (
                dep.call_span.start,
                dep.call_span.end,
                replacement(dep, newlines),
            )
        })
        .collect();

    // Only the callee token is retargeted, so the argument expression - which
    // may itself contain a static require - is spliced independently.
    replacements.extend(
        dynamic_callees
            .iter()
            .filter(|callee| callee.end <= source.len() && callee.start < callee.end)
            .map(|callee| (callee.start, callee.end, format!("{prefix}dynamic"))),
    );

    let mut value_exports = Vec::new();
    if version.has_value_exports() {
        let owned_parse_result;
        let block = match cached_block {
            Some(block) => block,
            None => {
                owned_parse_result = luck_parser::parse(source, version);
                &owned_parse_result.block
            }
        };

        // Luau rejects every form of `export` below the top level, and every
        // bundled module body lands inside a loader function. Type exports
        // become private aliases. Value exports become ordinary
        // declarations plus the frozen table Luau would implicitly return
        // for the module.
        for stmt in &block.stmts {
            match stmt {
                luck_ast::Statement::TypeDeclaration(type_decl) if type_decl.is_exported => {
                    let start = type_decl.span.start as usize;
                    replacements.push((start, start + "export".len(), String::new()));
                }
                luck_ast::Statement::LocalAssignment(local) if local.is_exported => {
                    for name in local.names.iter() {
                        if let luck_token::TokenKind::Identifier(name) = &name.name.kind {
                            value_exports.push(name.to_string());
                        }
                    }
                    let start = local.span.start as usize;
                    replacements.push((start, start + "export".len(), String::new()));
                }
                luck_ast::Statement::LocalFunction(function) if function.is_exported => {
                    if let luck_token::TokenKind::Identifier(name) = &function.name.kind {
                        value_exports.push(name.to_string());
                    }
                    // `export function` declares a module-local function.
                    // An attributed statement's span starts at the first
                    // attribute rather than at `export`, so the head is
                    // rewritten from the last attribute onward; comments
                    // between the keywords do not survive.
                    match function.attributes.last() {
                        Some(attr) => replacements.push((
                            attr.span.end as usize,
                            function.name.span.start as usize,
                            "\nlocal function ".to_string(),
                        )),
                        None => {
                            let start = function.span.start as usize;
                            replacements.push((start, start + "export".len(), "local".to_string()));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    replacements.sort_by_key(|(start, _, _)| *start);

    if replacements.is_empty() && value_exports.is_empty() {
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

    if !value_exports.is_empty() {
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str("return table.freeze({");
        for (index, name) in value_exports.iter().enumerate() {
            if index != 0 {
                result.push(',');
            }
            result.push_str(name);
            result.push('=');
            result.push_str(name);
        }
        result.push_str("})\n");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::require_extraction::extract_requires;
    use luck_core::types::DynamicRequire;
    use luck_token::LuaVersion;

    /// Build a ModuleInfo by running the REAL require extraction over
    /// `source`, resolving each extracted string through `resolve`.
    fn module(
        path: &str,
        source: &str,
        resolve: &[(&str, &str)],
        version: LuaVersion,
    ) -> ModuleInfo {
        let parsed = luck_parser::parse(source, version);
        assert!(
            parsed.errors.is_empty(),
            "fixture must parse: {:?}",
            parsed.errors
        );
        let extracted = extract_requires(&parsed.block, path, version, DynamicRequire::default());
        let dependencies = extracted
            .requires
            .iter()
            .map(|info| {
                let resolved = resolve
                    .iter()
                    .find(|(string, _)| *string == info.require_string)
                    .unwrap_or_else(|| panic!("no resolution for {:?}", info.require_string));
                Dependency {
                    require_string: info.require_string.clone(),
                    resolved_path: resolved.1.to_string(),
                    call_span: info.call_span.clone(),
                }
            })
            .collect();
        ModuleInfo {
            path: path.to_string(),
            source: source.to_string(),
            dependencies,
            relative_path: path.to_string(),
            dynamic_callees: extracted.dynamic_callees,
            parsed_block: Some(parsed.block),
        }
    }

    /// Modules in topo order with the LAST one as entry.
    fn graph(modules: Vec<ModuleInfo>) -> DependencyGraph {
        let entry_id = ModuleId(modules.len() - 1);
        DependencyGraph {
            topo_order: (0..modules.len()).map(ModuleId).collect(),
            entry_id,
            modules,
            warnings: vec![],
        }
    }

    fn reparses(output: &str, version: LuaVersion) {
        let result = luck_parser::parse(output, version);
        assert!(
            result.errors.is_empty(),
            "bundle must reparse: {:?}\n{output}",
            result.errors
        );
    }

    #[test]
    fn basic_bundle_uses_lazy_loader() {
        let modules = vec![
            module(
                "src/utils.lua",
                "local M = {}\nreturn M\n",
                &[],
                LuaVersion::Lua54,
            ),
            module(
                "src/main.lua",
                "local utils = require(\"utils\")\nprint(utils)\n",
                &[("utils", "src/utils.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("__luck_modules[\"utils\"]=function(...)"),
            "{output}"
        );
        assert!(
            output.contains("local utils = __luck_require(\"utils\")"),
            "{output}"
        );
        assert!(
            output.contains("-- module \"utils\": src/utils.lua"),
            "{output}"
        );
        assert!(
            output.contains("__luck_paths[\"utils\"]=\"src/utils.lua\""),
            "5.4 chunks receive the file path: {output}"
        );
        assert!(
            output.contains("local __luck_entry=function(...)"),
            "{output}"
        );
        assert!(output.contains("return __luck_entry(...)"), "{output}");
        reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn many_modules_use_table_slots_not_locals() {
        // One `local` per module would break past Lua's
        // 200-locals-per-function limit at ~199 modules.
        let mut modules = Vec::new();
        let mut entry_source = String::new();
        let mut resolve: Vec<(String, String)> = Vec::new();
        for i in 0..500 {
            modules.push(module(
                &format!("src/mod_{i}.lua"),
                &format!("return {i}\n"),
                &[],
                LuaVersion::Lua54,
            ));
            entry_source.push_str(&format!("require(\"mod_{i}\")\n"));
            resolve.push((format!("mod_{i}"), format!("src/mod_{i}.lua")));
        }
        let resolve_refs: Vec<(&str, &str)> = resolve
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        modules.push(module(
            "src/main.lua",
            &entry_source,
            &resolve_refs,
            LuaVersion::Lua54,
        ));
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("__luck_modules[\"mod_499\"]=function(...)"),
            "all modules registered"
        );
        assert!(
            !output.contains("local __luck_src_mod"),
            "no per-module locals"
        );
        reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn single_module_emits_verbatim() {
        let modules = vec![module(
            "src/main.lua",
            "print(\"hello\")\n",
            &[],
            LuaVersion::Lua54,
        )];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert_eq!(output, "print(\"hello\")\n");
    }

    #[test]
    fn entry_cycle_registers_entry_module() {
        // The entry function registers under the require string, so a
        // module requiring the entry leaves no raw require() in the
        // output. The extra load executes a fresh instance, exactly like
        // real Lua loading the main file a second time as a module.
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
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("__luck_modules[\"a\"]=__luck_entry"),
            "{output}"
        );
        assert!(
            !output.contains("require(\"a\")") || output.contains("__luck_require(\"a\")"),
            "no raw require may survive: {output}"
        );
        assert!(output.contains("__luck_require(\"a\")"), "{output}");
        reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn two_require_strings_for_one_file_get_separate_slots() {
        // Real Lua caches by NAME: require("foo") and require("foo.init")
        // hitting the same file are two instances executed twice. The
        // slots share one function but separate cache entries.
        let source = "local a = require(\"foo\")\nlocal b = require(\"foo.init\")\nprint(a, b)\n";
        let modules = vec![
            module("src/foo/init.lua", "return {}\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                source,
                &[
                    ("foo", "src/foo/init.lua"),
                    ("foo.init", "src/foo/init.lua"),
                ],
                LuaVersion::Lua54,
            ),
        ];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("__luck_modules[\"foo\"]=function(...)"),
            "{output}"
        );
        assert!(
            output.contains("__luck_modules[\"foo.init\"]=__luck_modules[\"foo\"]"),
            "{output}"
        );
        reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn generated_prefix_avoids_user_identifiers() {
        let source =
            "local __luck_require = 1\nlocal d = require(\"dep\")\nprint(__luck_require, d)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("local function __luck1_require"),
            "loader must rename itself: {output}"
        );
        assert!(
            output.contains("local d = __luck1_require(\"dep\")"),
            "{output}"
        );
        assert!(
            output.contains("local __luck_require = 1"),
            "user identifier untouched: {output}"
        );
        reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn multiline_require_preserves_line_count() {
        let source = "local d = require(\n\"dep\"\n)\nprint(d)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let (output, line_map) = emit_with_line_map(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("local d = __luck_require(\"dep\"\n\n)"),
            "padding keeps following lines in place: {output}"
        );
        let entry = line_map
            .iter()
            .find(|e| e.path == "src/main.lua")
            .expect("entry line map entry");
        let output_lines: Vec<&str> = output.lines().collect();
        // Source line 4 is print(d); it must land exactly 3 lines below
        // the entry body start despite the rewritten multiline call.
        assert_eq!(output_lines[entry.bundle_start_line - 1 + 3], "print(d)");
        reparses(&output, LuaVersion::Lua54);
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
        let output = emit(&graph(modules), LuaVersion::Lua51);
        assert!(
            output.contains("loop or previous error loading module"),
            "{output}"
        );
        assert!(
            !output.contains("__luck_paths"),
            "5.1 chunks receive only the module name: {output}"
        );
        reparses(&output, LuaVersion::Lua51);
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
        let output = emit(&graph(modules), LuaVersion::Lua52);
        assert!(
            !output.contains("loop or previous error"),
            "5.2+ has no sentinel; failed loads retry: {output}"
        );
        assert!(
            output.contains("(name,__luck_paths[name])"),
            "5.2+ chunks receive (name, path): {output}"
        );
        assert!(
            !output.contains(":preload:"),
            "loaderdata second result is 5.4+ only: {output}"
        );
        reparses(&output, LuaVersion::Lua52);
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
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(output.contains(":preload:"), "{output}");
        assert!(output.contains("return value,loaderdata"), "{output}");
        reparses(&output, LuaVersion::Lua54);
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
        let output = emit(&graph(modules), LuaVersion::Luau);
        assert!(
            output.contains("local m = __luck_require(1) :: any"),
            "{output}"
        );
        assert!(
            output.contains("select(\"#\",...)~=1"),
            "Luau modules must return exactly one value: {output}"
        );
        assert!(
            !output.contains("package.loaded"),
            "Luau has no package table: {output}"
        );
        reparses(&output, LuaVersion::Luau);
    }

    #[test]
    fn shadowed_require_call_survives_unrewritten() {
        let source = "local d = require(\"dep\")\nlocal function f()\n    local require = print\n    require(\"dep\")\nend\nf()\nprint(d)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("local d = __luck_require(\"dep\")"),
            "{output}"
        );
        assert!(
            output.contains("    require(\"dep\")"),
            "shadowed call keeps user semantics: {output}"
        );
        reparses(&output, LuaVersion::Lua54);
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
        let output = emit(&graph(modules), LuaVersion::Luau);

        assert!(!output.contains("export "), "{output}");
        assert!(output.contains("local value = 129312i"), "{output}");
        assert!(
            output.contains("const LIMIT = 0xffffffffffffffffi"),
            "{output}"
        );
        assert!(output.contains("@native\nlocal function get()"), "{output}");
        assert!(
            output.contains("return table.freeze({value=value,LIMIT=LIMIT,get=get})"),
            "{output}"
        );
        reparses(&output, LuaVersion::Luau);
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
            module(
                "src/dep.lua",
                "return { value = 1 }\n",
                &[],
                LuaVersion::Lua54,
            ),
            module(
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("local field = __luck_require(\"dep\").value"),
            "{output}"
        );
        assert!(
            output.contains("return __luck_require(\"dep\")"),
            "{output}"
        );
        assert!(
            output.contains("local inner = __luck_require(\"dep\")"),
            "{output}"
        );
        assert!(
            !output.contains("= require(\"dep\")"),
            "no raw require may survive: {output}"
        );
        reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn require_inside_string_literal_untouched() {
        let source = "local d = require(\"dep\")\nlocal msg = 'use require(\"dep\") to load'\nprint(d, msg)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("local d = __luck_require(\"dep\")"),
            "{output}"
        );
        assert!(
            output.contains("require(\"dep\") to load"),
            "string literal corrupted: {output}"
        );
    }

    #[test]
    fn escaped_require_string_is_rewritten_to_decoded_name() {
        // require("a\46b") is require("a.b"); both spellings share one
        // cache entry keyed by the decoded name.
        let source = "local x = require(\"a\\46b\")\nlocal y = require(\"a.b\")\nprint(x, y)\n";
        let modules = vec![
            module("src/a/b.lua", "return {}\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                source,
                &[("a.b", "src/a/b.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert_eq!(
            output.matches("__luck_require(\"a.b\")").count(),
            2,
            "{output}"
        );
        assert!(
            output.contains("__luck_modules[\"a.b\"]=function(...)"),
            "{output}"
        );
        reparses(&output, LuaVersion::Lua54);
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
            module("src/utils.lua", utils_source, &[], LuaVersion::Lua54),
            module("src/helper.lua", helper_source, &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                entry_source,
                &[("utils", "src/utils.lua"), ("helper", "src/helper.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let (output, line_map) = emit_with_line_map(&graph(modules), LuaVersion::Lua54);
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
            output_lines[utils.bundle_start_line - 3].starts_with("-- module "),
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
        assert_eq!(
            output_lines[entry.bundle_start_line - 1 + 2],
            "print(utils, helper)"
        );
        assert_eq!(entry.bundle_end_line - entry.bundle_start_line + 1, 3);

        // Ranges never overlap and are ordered utils, helper, entry.
        assert_eq!(line_map.len(), 3);
        assert!(utils.bundle_end_line < helper.bundle_start_line);
        assert!(helper.bundle_end_line < entry.bundle_start_line);
        reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn line_map_single_module_covers_whole_source() {
        let source = "print(\"a\")\nprint(\"b\")\nprint(\"c\")\n";
        let modules = vec![module("src/main.lua", source, &[], LuaVersion::Lua54)];
        let (output, line_map) = emit_with_line_map(&graph(modules), LuaVersion::Lua54);
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
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                "local d = require [[dep]]\nprint(d)\n",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = emit(&graph(modules), LuaVersion::Lua54);
        assert!(
            output.contains("local d = __luck_require(\"dep\")"),
            "{output}"
        );
        assert!(!output.contains("require [[dep]]"), "{output}");
    }

    #[test]
    fn quote_lua_string_escapes() {
        assert_eq!(quote_lua_string("plain"), "\"plain\"");
        assert_eq!(quote_lua_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_lua_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote_lua_string("a\nb"), "\"a\\nb\"");
        // Zero-padded decimal escapes can never absorb a following digit.
        assert_eq!(quote_lua_string("\u{7}5"), "\"\\0075\"");
    }
}
