use core::ops::Range;

use luck_ast::{MAX_NESTING_DEPTH, block_nesting_depth, spans_past_nesting_cap};
use luck_core::Diagnostic;
use luck_token::{
    CompactString, LuaVersion, Span, chunk_start, encode_string, ends_with_line_break, line_breaks,
};
use rustc_hash::FxHashSet;

use crate::block_emitter::rewrite_module;
use crate::exports::{ExportPlan, TextLowering, has_value_exports, lower_exports_in_text};
use crate::graph::DependencyGraph;
use crate::loader::{dynamic_entry_text, export_runtime_text, loader_text};
use crate::module::{Dependency, ModuleId, ModuleInfo};

/// `LUAI_MAXUPVALUES` in Lua 5.1.
const LUA51_MAX_UPVALUES: usize = 60;

/// Upvalues a rewritten function can gain: the loader and its dynamic entry.
const LOADER_UPVALUES: usize = 2;

/// Line 1 of the module's source lands on `bundle_start_line`, so a
/// runtime traceback line `L` inside the range maps back to source line
/// `L - bundle_start_line + 1`.
///
/// Require rewrites preserve the newline count of the call they replace, so
/// the correspondence is 1:1.
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

/// What a bundle holds, which its text and its tree both follow.
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
///   arguments, and errors unless a module returns exactly one value, as
///   the Luau runtime does. A value-export module in a cycle hands out a
///   placeholder.
///
/// The entry body is wrapped in a function called with the chunk's
/// varargs, so CLI args pass straight through, and a module that requires
/// the entry gets a fresh instance, as in real Lua when the main file is
/// also required as a module.
///
/// Every generated identifier shares a prefix that appears nowhere in any
/// module source, so user code can't capture or shadow loader internals.
///
/// Each module registers in a top-level statement, so the only nesting a
/// module gains is its own wrapper's.
pub(crate) struct Plan {
    pub prefix: String,
    /// True only for a 5.1 bundle with a function that has no upvalue left
    /// for a local loader.
    pub is_loader_global: bool,
    /// Only Lua targets record dynamic callees, so only a Lua loader gets the
    /// entry point they're retargeted at.
    pub has_dynamic_entry: bool,
    pub has_export_runtime: bool,
    /// Luau: each module's numeric slot, by `ModuleId`. `None` on Lua, whose
    /// cache is keyed by require string.
    pub slots: Option<Vec<Option<usize>>>,
    /// In bundle order.
    pub items: Vec<Item>,
}

pub(crate) enum Item {
    Module(WrappedModule),
    Registration(Registration),
}

pub(crate) struct WrappedModule {
    pub id: ModuleId,
    /// What follows `-- module ` in the comment above the wrapper. `None` for
    /// the entry.
    pub label: Option<CompactString>,
    pub binding: Binding,
    pub wrapper: Wrapper,
    pub exports: ExportPlan,
}

/// What a wrapped module's function is bound to.
pub(crate) enum Binding {
    /// A slot of the module table, such as `__luck_modules[1]`.
    Slot(Key),
    /// The entry's own local, which the bundle calls last.
    Entry,
}

/// A key of the module or path table.
#[derive(Clone)]
pub(crate) enum Key {
    /// Luau: a resolved file's slot.
    Slot(usize),
    /// Lua: a require string, as `package.loaded` keys its cache, held as a
    /// string literal.
    Name(CompactString),
}

impl Key {
    fn name(require_string: &str) -> Self {
        Self::Name(encode_string(require_string.as_bytes()))
    }

    pub(crate) fn text(&self) -> CompactString {
        match self {
            Self::Slot(slot) => CompactString::from(slot.to_string()),
            Self::Name(literal) => literal.clone(),
        }
    }
}

/// A statement that registers a module's function under another key.
pub(crate) enum Registration {
    /// Luau: a value-export module's function, wrapped so a cycle gets a
    /// placeholder: `P_modules[slot]=P_exporting(slot,P_modules[slot])`.
    Exporting { slot: usize },
    /// The entry's function under `key`, wrapped like a value-export module's
    /// when `exporting_slot` is its slot.
    Entry { key: Key, exporting_slot: Option<usize> },
    /// Lua: a second require string for one file, which shares its function
    /// but not its cache entry.
    Alias { alias: Key, primary: Key },
    /// Lua 5.2+: the project-relative path the chunk receives under `name`,
    /// held as a string literal.
    Path { name: Key, path: CompactString },
}

impl Registration {
    fn text(&self, prefix: &str) -> String {
        match self {
            Self::Exporting { slot } => {
                format!(
                    "{prefix}modules[{slot}]={prefix}exporting({slot},{prefix}modules[{slot}])\n"
                )
            }
            Self::Entry { key, exporting_slot } => {
                let registered = exporting_slot.map_or_else(
                    || format!("{prefix}entry"),
                    |slot| format!("{prefix}exporting({slot},{prefix}entry)"),
                );
                format!("{prefix}modules[{}]={registered}\n", key.text())
            }
            Self::Alias { alias, primary } => {
                format!("{prefix}modules[{}]={prefix}modules[{}]\n", alias.text(), primary.text())
            }
            Self::Path { name, path } => format!("{prefix}paths[{}]={path}\n", name.text()),
        }
    }
}

/// The function around a module's body.
#[derive(Clone, Copy)]
pub(crate) struct Wrapper {
    pub has_vararg_param: bool,
    /// Lua 5.2+: the body gets a private `_ENV` of its own, which its first
    /// statement makes upvalue 1, since the module can rebind it.
    pub has_private_env: bool,
    /// Lua 5.1: the body's first statement restores the global `arg`, which
    /// the vararg function's hidden local shadows.
    pub restores_arg: bool,
}

impl Wrapper {
    const fn of(module: &ModuleInfo, version: LuaVersion) -> Self {
        // 5.1's hidden `arg` and 5.5's `(vararg table)` each take a local
        // slot in a vararg function, so a body that reads no `...` gets none.
        let has_vararg_param = module.has_vararg_read
            || !(version.has_implicit_vararg_table() || version.has_named_varargs());
        Self {
            has_vararg_param,
            has_private_env: module.has_env_rebinding,
            restores_arg: !module.has_env_rebinding
                && module.has_vararg_read
                && module.has_global_arg_read,
        }
    }

    /// How many levels deeper the body nests than the module's own chunk, as
    /// [`MAX_NESTING_DEPTH`] counts them.
    pub(crate) const fn levels(self) -> usize {
        if self.has_private_env { 3 } else { 2 }
    }
}

impl Plan {
    /// # Errors
    ///
    /// Returns an E013 for each construct a wrapper would nest past
    /// [`MAX_NESTING_DEPTH`], at its place in its module's source, since luck
    /// couldn't read that bundle back.
    pub(crate) fn new(
        graph: &DependencyGraph,
        version: LuaVersion,
    ) -> Result<Self, Vec<Diagnostic>> {
        let modules = &graph.modules;
        let mut plan = Self {
            prefix: choose_prefix(modules),
            is_loader_global: modules.iter().any(|module| {
                module.max_function_upvalues.saturating_add(LOADER_UPVALUES) > LUA51_MAX_UPVALUES
            }),
            has_dynamic_entry: modules.iter().any(|module| !module.dynamic_callees.is_empty()),
            has_export_runtime: version.has_value_exports()
                && modules.iter().any(|module| has_value_exports(&module.block)),
            slots: None,
            items: Vec::new(),
        };
        if version.is_luau() {
            plan.push_luau_modules(graph, version);
        } else {
            plan.push_lua_modules(graph, version);
        }
        let errors = plan.nesting_errors(graph, version);
        if errors.is_empty() { Ok(plan) } else { Err(errors) }
    }

    /// The key of the module `dependency` loads. `None` when its file has no
    /// slot, which leaves the call as written.
    pub(crate) fn key(&self, dependency: &Dependency) -> Option<Key> {
        self.slots.as_ref().map_or_else(
            || Some(Key::name(&dependency.require_string)),
            |slots| slots.get(dependency.target.0).copied().flatten().map(Key::Slot),
        )
    }

    fn push_module(
        &mut self,
        id: ModuleId,
        module: &ModuleInfo,
        version: LuaVersion,
        label: Option<CompactString>,
        binding: Binding,
    ) -> bool {
        let exports = ExportPlan::new(&module.block, version);
        let has_values = exports.has_values;
        let wrapper = Wrapper::of(module, version);
        self.items.push(Item::Module(WrappedModule { id, label, binding, wrapper, exports }));
        has_values
    }

    /// Luau caches per resolved file: one numeric slot per module, including
    /// the entry when something requires it.
    fn push_luau_modules(&mut self, graph: &DependencyGraph, version: LuaVersion) {
        let mut slots: Vec<Option<usize>> = vec![None; graph.modules.len()];
        for (slot, (id, _)) in (1usize..).zip(non_entry_modules(graph)) {
            if let Some(module_slot) = slots.get_mut(id.0) {
                *module_slot = Some(slot);
            }
        }
        let is_entry_required = graph
            .modules
            .iter()
            .flat_map(|module| &module.dependencies)
            .any(|dependency| dependency.target == graph.entry_id);
        // The slot after every other module's.
        let entry_slot = is_entry_required.then_some(graph.modules.len());
        if let Some(entry_module_slot) = slots.get_mut(graph.entry_id.0) {
            *entry_module_slot = entry_slot;
        }
        self.slots = Some(slots);

        for (slot, (id, module)) in (1usize..).zip(non_entry_modules(graph)) {
            let label = Some(CompactString::from(format!("#{slot}")));
            if self.push_module(id, module, version, label, Binding::Slot(Key::Slot(slot))) {
                self.items.push(Item::Registration(Registration::Exporting { slot }));
            }
        }

        let Some(entry) = graph.modules.get(graph.entry_id.0) else {
            return;
        };
        let has_entry_exports =
            self.push_module(graph.entry_id, entry, version, None, Binding::Entry);
        if let Some(slot) = entry_slot {
            let exporting_slot = has_entry_exports.then_some(slot);
            let registration = Registration::Entry { key: Key::Slot(slot), exporting_slot };
            self.items.push(Item::Registration(registration));
        }
    }

    /// Lua caches per module name: one slot per unique require string.
    /// Strings reaching the same file share its function but keep separate
    /// cache entries, so the file runs once per name, as with
    /// `package.loaded`.
    fn push_lua_modules(&mut self, graph: &DependencyGraph, version: LuaVersion) {
        let mut seen_strings: FxHashSet<&str> = FxHashSet::default();
        let mut strings_by_module: Vec<Vec<&str>> = vec![Vec::new(); graph.modules.len()];
        let ordered_dependencies = graph
            .topological_order
            .iter()
            .filter_map(|id| graph.modules.get(id.0))
            .flat_map(|module| &module.dependencies);
        for dependency in ordered_dependencies {
            if seen_strings.insert(dependency.require_string.as_str())
                && let Some(strings) = strings_by_module.get_mut(dependency.target.0)
            {
                strings.push(dependency.require_string.as_str());
            }
        }

        for (id, module) in non_entry_modules(graph) {
            let Some(strings) = strings_by_module.get(id.0) else {
                continue;
            };
            let Some((primary, aliases)) = strings.split_first() else {
                continue;
            };
            let first = Key::name(primary);
            let label = Some(first.text());
            self.push_module(id, module, version, label, Binding::Slot(first.clone()));
            for alias in aliases {
                let registration =
                    Registration::Alias { alias: Key::name(alias), primary: first.clone() };
                self.items.push(Item::Registration(registration));
            }
            self.push_path_registrations(strings, &module.relative_path, version);
        }

        let Some(entry) = graph.modules.get(graph.entry_id.0) else {
            return;
        };
        self.push_module(graph.entry_id, entry, version, None, Binding::Entry);
        if let Some(entry_strings) =
            strings_by_module.get(graph.entry_id.0).filter(|strings| !strings.is_empty())
        {
            for string in entry_strings {
                let registration =
                    Registration::Entry { key: Key::name(string), exporting_slot: None };
                self.items.push(Item::Registration(registration));
            }
            self.push_path_registrations(entry_strings, &entry.relative_path, version);
        }
    }

    fn push_path_registrations(
        &mut self,
        strings: &[&str],
        relative_path: &str,
        version: LuaVersion,
    ) {
        if !version.has_require_filepath_arg() {
            return;
        }
        let path = encode_string(relative_path.as_bytes());
        for string in strings {
            let registration = Registration::Path { name: Key::name(string), path: path.clone() };
            self.items.push(Item::Registration(registration));
        }
    }

    /// Each construct a module's wrapper nests past [`MAX_NESTING_DEPTH`],
    /// measured on the module as the bundle holds it, since lowering an
    /// export to a field of the export table reads it one level deeper.
    fn nesting_errors(&self, graph: &DependencyGraph, version: LuaVersion) -> Vec<Diagnostic> {
        let mut errors = Vec::new();
        for item in &self.items {
            let Item::Module(wrapped) = item else {
                continue;
            };
            let Some(module) = graph.modules.get(wrapped.id.0) else {
                continue;
            };
            let levels = wrapped.wrapper.levels();
            let most_levels = block_nesting_depth(&module.block)
                .saturating_add(levels)
                .saturating_add(usize::from(wrapped.exports.has_table()));
            if most_levels <= MAX_NESTING_DEPTH {
                continue;
            }
            let rewritten =
                rewrite_module(module.block.clone(), module, self, &wrapped.exports, version);
            errors.extend(
                spans_past_nesting_cap(&rewritten, levels)
                    .into_iter()
                    .map(|span| Diagnostic::e013(&module.path, span, levels, MAX_NESTING_DEPTH)),
            );
        }
        errors
    }
}

/// The bundle's text, which keeps every module's source as written, with
/// splices only where a require or an export is rewritten, and a line map
/// back to it.
///
/// A lone module with no requires is emitted verbatim.
///
/// A shebang is only valid on a chunk's first line, so the entry's moves to
/// the bundle's first line, which keeps the bundle runnable as a script, and
/// every other module's is dropped.
///
/// # Errors
///
/// Returns the errors [`Plan::new`] does.
pub(crate) fn emit(
    graph: &DependencyGraph,
    version: LuaVersion,
) -> Result<(String, Vec<LineMapEntry>), Vec<Diagnostic>> {
    let modules = &graph.modules;
    let Some(entry) = modules.get(graph.entry_id.0) else {
        return Ok((String::new(), Vec::new()));
    };

    if modules.len() == 1 && entry.dependencies.is_empty() {
        let line_map = vec![LineMapEntry {
            bundle_start_line: 1,
            // A final line break closes its line rather than opening an empty one.
            bundle_end_line: line_breaks(&entry.source, version)
                .count()
                .saturating_add(usize::from(!ends_with_line_break(&entry.source, version))),
            path: entry.path.clone(),
        }];
        return Ok((entry.source.clone(), line_map));
    }

    let plan = Plan::new(graph, version)?;
    let prefix = plan.prefix.as_str();

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
        plan: &plan,
        version,
    };

    if let Some(shebang) = entry.shebang {
        writer.push(shebang.text(&entry.source));
        writer.push("\n");
    }

    // Luau only honors hot comments before any code, so the entry's leading
    // ones go above the loader.
    for hot_comment in &entry.hot_comments {
        writer.push(hot_comment.text(&entry.source));
        writer.push("\n");
    }

    writer.push(&loader_text(prefix, version, plan.is_loader_global));
    if plan.has_dynamic_entry {
        writer.push(&dynamic_entry_text(prefix, plan.is_loader_global));
    }
    if plan.has_export_runtime {
        writer.push(&export_runtime_text(prefix));
    }

    for item in &plan.items {
        match item {
            Item::Module(wrapped) => {
                if let Some(module) = modules.get(wrapped.id.0) {
                    writer.push_wrapped(wrapped, module);
                }
            }
            Item::Registration(registration) => writer.push(&registration.text(prefix)),
        }
    }

    writer.push(&format!("return {prefix}entry(...)\n"));

    Ok((writer.output, writer.line_map))
}

struct BundleWriter<'plan> {
    output: String,
    /// 1-based line the next pushed character lands on.
    next_line: usize,
    line_map: Vec<LineMapEntry>,
    plan: &'plan Plan,
    version: LuaVersion,
}

impl BundleWriter<'_> {
    fn push(&mut self, fragment: &str) {
        self.output.push_str(fragment);
        self.next_line = self.next_line.saturating_add(line_breaks(fragment, self.version).count());
    }

    /// A line break in a file name would end the comment and turn the rest
    /// of the name into code, so it's written as an escape.
    fn push_provenance(&mut self, label: &str, relative_path: &str) {
        let path = relative_path.replace('\n', "\\n").replace('\r', "\\r");
        self.push(&format!("-- module {label}: {path}\n"));
    }

    fn push_wrapped(&mut self, wrapped: &WrappedModule, module: &ModuleInfo) {
        if let Some(label) = &wrapped.label {
            self.push_provenance(label, &module.relative_path);
        }
        let prefix = self.plan.prefix.as_str();
        let lowering = if self.version.has_value_exports() {
            lower_exports_in_text(
                &wrapped.exports,
                &module.block,
                &module.source,
                self.version,
                prefix,
            )
        } else {
            TextLowering::default()
        };
        let (opening, closing) = wrapper_text(&wrapped.binding, wrapped.wrapper, prefix);
        self.push(&opening);
        self.push(lowering.table_declaration.as_deref().unwrap_or_default());
        self.push("\n");
        self.push_body(module, lowering.edits, lowering.closing.as_deref());
        self.push(closing);
    }

    /// A loader call keeps the newline count of the call it replaces, so the
    /// line map stays 1:1 for multiline requires too.
    fn push_body(
        &mut self,
        module: &ModuleInfo,
        export_edits: Vec<(Span, String)>,
        closing: Option<&str>,
    ) {
        let source = module.source.as_str();
        let prefix = self.plan.prefix.as_str();
        let mut edits: Vec<(Span, String)> = module
            .dependencies
            .iter()
            .filter_map(|dependency| {
                let call = dependency.call_span.text(source);
                if call.is_empty() {
                    return None;
                }
                let newlines = "\n".repeat(line_breaks(call, self.version).count());
                let key = self.plan.key(dependency)?;
                Some((dependency.call_span, format!("{prefix}require({}{newlines})", key.text())))
            })
            .collect();

        // Only the callee token is retargeted, so a static require inside the
        // argument still gets its own splice.
        edits.extend(
            module
                .dynamic_callees
                .iter()
                .filter(|callee| !callee.text(source).is_empty())
                .map(|callee| (*callee, format!("{prefix}dynamic"))),
        );

        edits.extend(export_edits);
        // Only a chunk may open with a shebang or a BOM, and this body is
        // inside a function. The shebang's line break stays for the line map.
        edits.extend(module.shebang.map(|span| (span, String::new())));
        edits.sort_by_key(|(span, _)| span.start);

        let first_line = self.next_line;
        let body_start = self.output.len();
        let mut cursor = chunk_start(source, self.version);
        // A lone CR opening the body would join the generated LF above it into
        // one LFCR line break, losing the body's first line.
        let body = source.get(cursor..).unwrap_or_default();
        if self.version.has_lfcr_line_break() && body.starts_with('\r') && !body.starts_with("\r\n")
        {
            self.push(" ");
        }
        for (span, replacement) in &edits {
            let range = Range::<usize>::from(*span);
            self.push(source.get(cursor..range.start).unwrap_or_default());
            self.push(replacement);
            cursor = range.end;
        }
        self.push(source.get(cursor..).unwrap_or_default());

        if let Some(closing_lines) = closing {
            if self.output.len() == body_start || !ends_with_line_break(&self.output, self.version)
            {
                self.push("\n");
            }
            self.push(closing_lines);
        }
        if self.output.len() > body_start && !ends_with_line_break(&self.output, self.version) {
            self.push("\n");
        }

        self.line_map.push(LineMapEntry {
            bundle_start_line: first_line,
            bundle_end_line: self.next_line.saturating_sub(1),
            path: module.path.clone(),
        });
    }
}

/// The text before and after a wrapped body. A statement the wrapper puts
/// before the body ends in `;`, so a body that opens with `(` can't continue
/// it as a call.
fn wrapper_text(binding: &Binding, wrapper: Wrapper, prefix: &str) -> (String, &'static str) {
    let params = if wrapper.has_vararg_param { "..." } else { "" };
    let target = match binding {
        Binding::Slot(key) => format!("{prefix}modules[{}]", key.text()),
        Binding::Entry => format!("{prefix}entry"),
    };
    let is_entry = matches!(binding, Binding::Entry);
    if wrapper.has_private_env {
        let declaration = if is_entry { format!("local {target} ") } else { String::new() };
        return (
            format!("{declaration}do local _ENV=_ENV {target}=function({params})_ENV=_ENV;"),
            "end end\n",
        );
    }
    let declaration = if is_entry { "local " } else { "" };
    let restore = if wrapper.restores_arg { "arg=_G.arg;" } else { "" };
    (format!("{declaration}{target}=function({params}){restore}"), "end\n")
}

/// Every module but the entry, in the order the bundle registers them.
fn non_entry_modules(graph: &DependencyGraph) -> impl Iterator<Item = (ModuleId, &ModuleInfo)> {
    graph
        .topological_order
        .iter()
        .filter(|id| **id != graph.entry_id)
        .filter_map(|id| graph.modules.get(id.0).map(|module| (*id, module)))
}

fn choose_prefix(modules: &[ModuleInfo]) -> String {
    let mut candidate = "__luck_".to_owned();
    let mut counter = 0usize;
    while modules.iter().any(|module| module.source.contains(&candidate)) {
        counter = counter.saturating_add(1);
        candidate = format!("__luck{counter}_");
    }
    candidate
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use rustc_hash::FxHashMap;

    use super::*;
    use crate::dynamic_require::DynamicRequire;
    use crate::require_extraction::extract_requires;

    struct ModuleFixture {
        info: ModuleInfo,
        resolved_paths: Vec<String>,
    }

    fn module(
        path: &str,
        source: &str,
        resolutions: &[(&str, &str)],
        version: LuaVersion,
    ) -> ModuleFixture {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "fixture must parse: {:?}", parsed.errors);
        let extracted = extract_requires(&parsed.block, path, version, DynamicRequire::default());
        let mut resolved_paths = Vec::new();
        let dependencies = extracted
            .sites
            .into_iter()
            .map(|site| {
                let resolved = resolutions
                    .iter()
                    .find(|(require_string, _)| *require_string == site.require_string)
                    .unwrap_or_else(|| panic!("no resolution for {:?}", site.require_string));
                resolved_paths.push(resolved.1.to_owned());
                Dependency {
                    require_string: site.require_string,
                    target: ModuleId(usize::MAX),
                    call_span: site.call_span,
                }
            })
            .collect();
        let hot_comments = if version.is_luau() {
            luck_token::leading_hot_comments(&parsed.comments, source)
                .map(|comment| comment.span)
                .collect()
        } else {
            Vec::new()
        };
        let shebang = parsed
            .comments
            .iter()
            .find(|comment| comment.kind == luck_token::CommentKind::Shebang)
            .map(|comment| comment.span);
        let info = ModuleInfo {
            path: path.to_owned(),
            source: source.to_owned(),
            dependencies,
            relative_path: path.to_owned(),
            dynamic_callees: extracted.dynamic_callees,
            hot_comments,
            shebang,
            has_vararg_read: extracted.has_vararg_read,
            has_global_arg_read: extracted.has_global_arg_read,
            has_env_rebinding: extracted.has_env_rebinding,
            max_function_upvalues: extracted.max_function_upvalues,
            block: parsed.block,
        };
        ModuleFixture { info, resolved_paths }
    }

    /// The graph of `fixtures`, given in topological order with the last one
    /// as entry, laid out as `DependencyGraph::build` lays it out, entry first.
    fn graph(mut fixtures: Vec<ModuleFixture>) -> DependencyGraph {
        fixtures.rotate_right(1);
        let ids_by_path: FxHashMap<String, ModuleId> = fixtures
            .iter()
            .enumerate()
            .map(|(i, fixture)| (fixture.info.path.clone(), ModuleId(i)))
            .collect();
        let modules: Vec<ModuleInfo> = fixtures
            .into_iter()
            .map(|mut fixture| {
                for (dependency, path) in
                    fixture.info.dependencies.iter_mut().zip(&fixture.resolved_paths)
                {
                    dependency.target = ids_by_path[path];
                }
                fixture.info
            })
            .collect();
        let topological_order = (1..modules.len()).chain([0]).map(ModuleId).collect();
        DependencyGraph { modules, topological_order, entry_id: ModuleId(0), warnings: vec![] }
    }

    fn emitted(fixtures: Vec<ModuleFixture>, version: LuaVersion) -> (String, Vec<LineMapEntry>) {
        emit(&graph(fixtures), version).unwrap_or_else(|errors| panic!("bundle failed: {errors:?}"))
    }

    fn bundle(fixtures: Vec<ModuleFixture>, version: LuaVersion) -> String {
        emitted(fixtures, version).0
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
        let (output, line_map) = emitted(modules, LuaVersion::Lua54);
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
    fn entry_shebang_moves_up_and_keeps_its_line_for_the_map() {
        let source = "#!/usr/bin/env lua\nlocal d = require(\"dep\")\nprint(d)\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module("src/main.lua", source, &[("dep", "src/dep.lua")], LuaVersion::Lua54),
        ];
        let (output, line_map) = emitted(modules, LuaVersion::Lua54);
        assert!(output.starts_with("#!/usr/bin/env lua\nlocal __luck_modules={}\n"), "{output}");
        assert_eq!(output.matches("#!").count(), 1, "{output}");
        let entry = find_line_entry(&line_map, "src/main.lua");
        let output_lines: Vec<&str> = output.lines().collect();
        assert_eq!(output_lines[entry.bundle_start_line - 1], "");
        assert_eq!(output_lines[entry.bundle_start_line - 1 + 2], "print(d)");
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
    fn lua51_bundle_writes_no_global_by_default() {
        let source = "local d = require(\"dep\")\nlocal name = \"dep\"\nprint(d, require(name))\n";
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua51),
            module("src/main.lua", source, &[("dep", "src/dep.lua")], LuaVersion::Lua51),
        ];
        let output = bundle(modules, LuaVersion::Lua51);
        assert!(output.contains("local function __luck_require(name)"), "{output}");
        assert!(output.contains("local function __luck_dynamic(name)"), "{output}");
        assert!(!output.contains("\nfunction __luck_"), "{output}");
        assert_reparses(&output, LuaVersion::Lua51);
    }

    fn lua51_bundle_capturing(captures: usize) -> String {
        let locals =
            (0..captures).map(|i| format!("local u{i} = {i}\n")).collect::<Vec<_>>().concat();
        let sum = (0..captures).map(|i| format!("u{i}")).collect::<Vec<_>>().join(" + ");
        let source = format!(
            "{locals}local function f()\n    return require(\"dep\"), {sum}\nend\nprint(f())\n"
        );
        let modules = vec![
            module("src/dep.lua", "return 1\n", &[], LuaVersion::Lua51),
            module("src/main.lua", &source, &[("dep", "src/dep.lua")], LuaVersion::Lua51),
        ];
        bundle(modules, LuaVersion::Lua51)
    }

    #[test]
    fn lua51_loader_goes_global_only_without_room_for_its_upvalues() {
        let with_room = lua51_bundle_capturing(58);
        assert!(with_room.contains("local function __luck_require(name)"), "{with_room}");

        let without_room = lua51_bundle_capturing(59);
        assert!(without_room.contains("\ndo function __luck_require(name)"), "{without_room}");
        assert!(!without_room.contains("local function __luck_require"), "{without_room}");
        assert_reparses(&without_room, LuaVersion::Lua51);
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
            output.contains("return __luck_freeze({value=value,LIMIT=LIMIT,get=get})"),
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
        let (output, line_map) = emitted(modules, LuaVersion::Lua54);
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
        let (output, line_map) = emitted(modules, LuaVersion::Lua54);
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
    fn line_break_in_a_module_path_stays_inside_the_provenance_comment() {
        let path = "src/a\nerror(1)--.lua";
        let modules = vec![
            module(path, "return 1\n", &[], LuaVersion::Lua54),
            module(
                "src/main.lua",
                "local d = require(\"dep\")\nprint(d)\n",
                &[("dep", path)],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        assert!(
            !output.lines().any(|line| line.starts_with("error(1)")),
            "a module path must not escape its comment:\n{output}"
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
    fn loader_keys_escape_with_encode_string() {
        let modules = vec![
            module("/p/dep.lua", "return 1", &[], LuaVersion::Lua54),
            module(
                "/p/main.lua",
                "local d = require(\"a\\tb\\ac\\u{e9}\\0015\")",
                &[("a\tb\u{7}c\u{e9}\u{1}5", "/p/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(modules, LuaVersion::Lua54);
        // `\t` and `\a` stay symbolic, UTF-8 stays literal, and a control
        // byte before a digit stays padded.
        assert!(output.contains("__luck_require(\"a\\tb\\ac\u{e9}\\0015\")"), "{output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }
}
