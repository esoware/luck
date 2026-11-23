use core::fmt::Write as _;
use core::ops::Range;
use core::{iter, mem};
use std::path::PathBuf;

use luck_allocator::Allocator;
use luck_ast::MAX_NESTING_DEPTH;
use luck_diagnostics::FileDiagnostics;
use luck_token::{
    LuaVersion, Span, chunk_header, chunk_start, ends_with_line_break, line_breaks, spell_string,
};
use memchr::memmem;
use rustc_hash::FxHashSet;

use crate::block_emitter::{LoaderNames, overflow_errors, rewrite_module};
use crate::graph::DependencyGraph;
use crate::loader::{LoaderShape, PREFIX_STEM, write_prelude};
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
    pub path: PathBuf,
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
pub(crate) struct Plan<'ast> {
    pub prefix: String,
    pub loader: LoaderShape,
    /// Luau: each module's numeric slot, by `ModuleId`. `None` on Lua, whose
    /// cache is keyed by require string.
    pub slots: Option<Vec<Option<usize>>>,
    /// In bundle order.
    pub items: Vec<Item<'ast>>,
}

pub(crate) enum Item<'ast> {
    Module(WrappedModule<'ast>),
    Registration(Registration<'ast>),
}

pub(crate) struct WrappedModule<'ast> {
    pub id: ModuleId,
    /// What the comment above the wrapper names it by. `None` for the entry.
    pub label: Option<Key<'ast>>,
    pub binding: Binding<'ast>,
    pub wrapper: Wrapper,
}

/// What a wrapped module's function is bound to.
pub(crate) enum Binding<'ast> {
    /// A slot of the module table, such as `__luck_modules[1]`.
    Slot(Key<'ast>),
    /// The entry's own local, which the bundle calls last.
    Entry,
}

/// A key of the module or path table.
#[derive(Clone, Copy)]
pub(crate) enum Key<'ast> {
    /// Luau: a resolved file's slot.
    Slot(usize),
    /// Lua: a require string, as `package.loaded` keys its cache, written as
    /// a string literal.
    Name(&'ast str),
}

impl Key<'_> {
    pub(crate) fn write(self, output: &mut String) {
        match self {
            Self::Slot(slot) => push_number(output, slot),
            Self::Name(name) => {
                // A spelling is UTF-8, so the text takes it as it is.
                let mut literal = Vec::new();
                spell_string(name.as_bytes()).write_to(&mut literal);
                output.push_str(&String::from_utf8_lossy(&literal));
            }
        }
    }

    /// A Luau slot reads `#1`, and a Lua name its literal.
    fn write_label(self, output: &mut String) {
        if matches!(self, Self::Slot(_)) {
            output.push('#');
        }
        self.write(output);
    }
}

pub(crate) fn push_number(output: &mut String, number: usize) {
    let _infallible = write!(output, "{number}");
}

/// A statement that registers a module's function under another key.
pub(crate) enum Registration<'ast> {
    /// Luau: a value-export module's function, wrapped so a cycle gets a
    /// placeholder: `P_modules[slot]=P_exporting(slot,P_modules[slot])`.
    Exporting { slot: usize },
    /// The entry's function under `key`, wrapped like a value-export module's
    /// when `exporting_slot` is its slot.
    Entry { key: Key<'ast>, exporting_slot: Option<usize> },
    /// Lua: a second require string for one file, which shares its function
    /// but not its cache entry.
    Alias { alias: Key<'ast>, primary: Key<'ast> },
    /// Lua 5.2+: the project-relative path of `module`, which the chunk
    /// receives under `name`.
    Path { name: Key<'ast>, module: ModuleId },
}

impl Registration<'_> {
    fn write(&self, prefix: &str, modules: &[ModuleInfo<'_>], output: &mut String) {
        match *self {
            Self::Exporting { slot } => {
                output.extend([prefix, "modules["]);
                push_number(output, slot);
                output.extend(["]=", prefix, "exporting("]);
                push_number(output, slot);
                output.extend([",", prefix, "modules["]);
                push_number(output, slot);
                output.push_str("])\n");
            }
            Self::Entry { key, exporting_slot } => {
                output.extend([prefix, "modules["]);
                key.write(output);
                output.push_str("]=");
                if let Some(slot) = exporting_slot {
                    output.extend([prefix, "exporting("]);
                    push_number(output, slot);
                    output.extend([",", prefix, "entry)\n"]);
                } else {
                    output.extend([prefix, "entry\n"]);
                }
            }
            Self::Alias { alias, primary } => {
                output.extend([prefix, "modules["]);
                alias.write(output);
                output.extend(["]=", prefix, "modules["]);
                primary.write(output);
                output.push_str("]\n");
            }
            Self::Path { name, module } => {
                output.extend([prefix, "paths["]);
                name.write(output);
                output.push_str("]=");
                let path = modules.get(module.0).map_or("", |info| info.relative_path.as_str());
                let mut literal = Vec::new();
                spell_string(path.as_bytes()).write_to(&mut literal);
                output.push_str(&String::from_utf8_lossy(&literal));
                output.push('\n');
            }
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
    const fn of(module: &ModuleInfo<'_>, version: LuaVersion) -> Self {
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

    /// Whether `module`'s body could nest past [`MAX_NESTING_DEPTH`] in this
    /// wrapper, measured as its parse counted it. Lowering an export to a
    /// field of the export table reads it one level deeper.
    pub(crate) fn may_overflow(self, module: &ModuleInfo<'_>) -> bool {
        module
            .nesting_depth
            .saturating_add(self.levels())
            .saturating_add(usize::from(module.exports.has_table()))
            > MAX_NESTING_DEPTH
    }
}

impl<'ast> Plan<'ast> {
    pub(crate) fn new(graph: &DependencyGraph<'ast>, version: LuaVersion) -> Self {
        let modules = &graph.modules;
        let mut plan = Self {
            prefix: choose_prefix(modules),
            loader: LoaderShape {
                version,
                is_loader_global: modules.iter().any(|module| {
                    module.max_function_upvalues.saturating_add(LOADER_UPVALUES)
                        > LUA51_MAX_UPVALUES
                }),
                has_dynamic_entry: modules.iter().any(|module| !module.dynamic_callees.is_empty()),
                has_export_runtime: modules.iter().any(|module| module.exports.has_values),
            },
            slots: None,
            items: Vec::with_capacity(modules.len().saturating_mul(2)),
        };
        if version.is_luau() {
            plan.push_luau_modules(graph, version);
        } else {
            plan.push_lua_modules(graph, version);
        }
        plan
    }

    /// Each wrapped module, in bundle order.
    pub(crate) fn wrapped_modules(&self) -> impl Iterator<Item = &WrappedModule<'ast>> {
        self.items.iter().filter_map(|item| match item {
            Item::Module(wrapped) => Some(wrapped),
            Item::Registration(_) => None,
        })
    }

    /// The key of the module `dependency` loads. `None` when its file has no
    /// slot, which leaves the call as written.
    pub(crate) fn key(&self, dependency: &Dependency<'ast>) -> Option<Key<'ast>> {
        self.slots.as_ref().map_or_else(
            || Some(Key::Name(dependency.require_string)),
            |slots| slots.get(dependency.target.0).copied().flatten().map(Key::Slot),
        )
    }

    fn push_module(
        &mut self,
        id: ModuleId,
        module: &ModuleInfo<'_>,
        version: LuaVersion,
        label: Option<Key<'ast>>,
        binding: Binding<'ast>,
    ) {
        let wrapper = Wrapper::of(module, version);
        self.items.push(Item::Module(WrappedModule { id, label, binding, wrapper }));
    }

    /// Luau caches per resolved file: one numeric slot per module, including
    /// the entry when something requires it.
    fn push_luau_modules(&mut self, graph: &DependencyGraph<'ast>, version: LuaVersion) {
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
            let key = Key::Slot(slot);
            self.push_module(id, module, version, Some(key), Binding::Slot(key));
            if module.exports.has_values {
                self.items.push(Item::Registration(Registration::Exporting { slot }));
            }
        }

        let Some(entry) = graph.modules.get(graph.entry_id.0) else {
            return;
        };
        self.push_module(graph.entry_id, entry, version, None, Binding::Entry);
        if let Some(slot) = entry_slot {
            let exporting_slot = entry.exports.has_values.then_some(slot);
            let registration = Registration::Entry { key: Key::Slot(slot), exporting_slot };
            self.items.push(Item::Registration(registration));
        }
    }

    /// Lua caches per module name: one slot per unique require string.
    /// Strings reaching the same file share its function but keep separate
    /// cache entries, so the file runs once per name, as with
    /// `package.loaded`.
    fn push_lua_modules(&mut self, graph: &DependencyGraph<'ast>, version: LuaVersion) {
        let mut seen_strings: FxHashSet<&str> = FxHashSet::default();
        let mut strings_by_module: Vec<Vec<&'ast str>> = vec![Vec::new(); graph.modules.len()];
        let ordered_dependencies = graph
            .topological_order
            .iter()
            .filter_map(|id| graph.modules.get(id.0))
            .flat_map(|module| &module.dependencies);
        for dependency in ordered_dependencies {
            if seen_strings.insert(dependency.require_string)
                && let Some(strings) = strings_by_module.get_mut(dependency.target.0)
            {
                strings.push(dependency.require_string);
            }
        }

        for (id, module) in non_entry_modules(graph) {
            let Some((&primary, aliases)) =
                strings_by_module.get(id.0).and_then(|strings| strings.split_first())
            else {
                continue;
            };
            let first = Key::Name(primary);
            self.push_module(id, module, version, Some(first), Binding::Slot(first));
            for &alias in aliases {
                let registration = Registration::Alias { alias: Key::Name(alias), primary: first };
                self.items.push(Item::Registration(registration));
            }
            self.push_path_registrations(id, strings_by_module.get(id.0), version);
        }

        let Some(entry) = graph.modules.get(graph.entry_id.0) else {
            return;
        };
        self.push_module(graph.entry_id, entry, version, None, Binding::Entry);
        let entry_strings = strings_by_module.get(graph.entry_id.0);
        for &string in entry_strings.into_iter().flatten() {
            let registration = Registration::Entry { key: Key::Name(string), exporting_slot: None };
            self.items.push(Item::Registration(registration));
        }
        self.push_path_registrations(graph.entry_id, entry_strings, version);
    }

    fn push_path_registrations(
        &mut self,
        module: ModuleId,
        strings: Option<&Vec<&'ast str>>,
        version: LuaVersion,
    ) {
        if !version.has_require_filepath_arg() {
            return;
        }
        for &string in strings.into_iter().flatten() {
            let registration = Registration::Path { name: Key::Name(string), module };
            self.items.push(Item::Registration(registration));
        }
    }
}

/// One splice of a module's source.
pub(crate) enum Edit<'ast> {
    /// A require becomes a call of the loader with the module's key, padded
    /// with the line breaks the call spans.
    Require {
        key: Key<'ast>,
        line_breaks: usize,
    },
    /// A `require(expr)` callee becomes the loader's dynamic entry point.
    Dynamic,
    Text(String),
}

/// The splices `module` takes in its source, by where they start.
fn module_edits<'ast>(
    module: &ModuleInfo<'ast>,
    plan: &Plan<'ast>,
    version: LuaVersion,
) -> Vec<(Span, Edit<'ast>)> {
    let source = module.source;
    let mut edits: Vec<(Span, Edit<'ast>)> = Vec::with_capacity(
        module.dependencies.len().saturating_add(module.dynamic_callees.len()).saturating_add(1),
    );
    edits.extend(module.dependencies.iter().filter_map(|dependency| {
        let call = dependency.call_span.text(source);
        if call.is_empty() {
            return None;
        }
        let key = plan.key(dependency)?;
        Some((
            dependency.call_span,
            Edit::Require { key, line_breaks: line_breaks(call, version).count() },
        ))
    }));
    // Only the callee token is retargeted, so a static require inside the
    // argument still gets its own splice.
    edits.extend(
        module
            .dynamic_callees
            .iter()
            .filter(|callee| !callee.text(source).is_empty())
            .map(|callee| (*callee, Edit::Dynamic)),
    );
    if version.has_value_exports() {
        module.exports.text_edits(&module.block, source, version, &plan.prefix, &mut edits);
    }
    // Only a chunk may open with a shebang or a BOM, and this body is
    // inside a function. The shebang's line break stays for the line map.
    edits.extend(module.shebang.map(|span| (span, Edit::Text(String::new()))));
    edits.sort_by_key(|(span, _)| span.start);
    edits
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
/// Returns an E013 for each construct a wrapper would nest past
/// [`MAX_NESTING_DEPTH`], at its place in its module's source, since luck
/// couldn't read that bundle back. Measuring one rewrites its module's tree,
/// whose new nodes go in `allocator`.
pub(crate) fn emit<'ast>(
    allocator: &'ast Allocator,
    graph: &mut DependencyGraph<'ast>,
    version: LuaVersion,
) -> Result<(Vec<u8>, Vec<LineMapEntry>), Vec<FileDiagnostics>> {
    let Some(entry) = graph.modules.get(graph.entry_id.0) else {
        return Ok((Vec::new(), Vec::new()));
    };

    if graph.modules.len() == 1 && entry.dependencies.is_empty() {
        let line_map = vec![LineMapEntry {
            bundle_start_line: 1,
            // A final line break closes its line rather than opening an empty one.
            bundle_end_line: line_breaks(entry.source, version)
                .count()
                .saturating_add(usize::from(!ends_with_line_break(entry.source, version))),
            path: entry.path.clone(),
        }];
        return Ok((entry.source.to_owned(), line_map));
    }

    let plan = Plan::new(graph, version);
    // Taken before measuring a module rewrites its tree.
    let edits: Vec<Vec<(Span, Edit<'ast>)>> = plan
        .wrapped_modules()
        .map(|wrapped| {
            graph
                .modules
                .get(wrapped.id.0)
                .map_or_else(Vec::new, |module| module_edits(module, &plan, version))
        })
        .collect();
    let names = LoaderNames::new(allocator, &plan.prefix);
    let mut errors = Vec::new();
    for wrapped in plan.wrapped_modules() {
        if let Some(module) = graph.modules.get_mut(wrapped.id.0)
            && wrapped.wrapper.may_overflow(module)
        {
            let mut block = module.block.take();
            rewrite_module(allocator, &mut block, module, &plan, &names, version);
            errors.extend(overflow_errors(&block, module, wrapped.wrapper));
            module.block = block;
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // The sources plus rough room for each module's wrapper and the loader.
    let capacity = graph
        .modules
        .iter()
        .map(|module| module.source.len().saturating_add(96))
        .sum::<usize>()
        .saturating_add(512);
    let writer = write(graph, &plan, version, capacity, Bodies::Spliced(edits));
    Ok((writer.output, writer.line_map))
}

/// What a module's body is in the text [`skeleton`] writes: a call of this,
/// after the bundle's prefix, which no module's source holds.
pub(crate) const HOLE: &str = "body";

/// The bundle's text with each module's body written as a call of the
/// prefixed [`HOLE`], so the tree that holds the modules' own trees can take
/// everything else from the text's parse.
pub(crate) fn skeleton(
    graph: &DependencyGraph<'_>,
    plan: &Plan<'_>,
    version: LuaVersion,
) -> Vec<u8> {
    let capacity = graph.modules.len().saturating_mul(128).saturating_add(4096);
    write(graph, plan, version, capacity, Bodies::Holes).output
}

/// How [`write()`] writes each module's body.
enum Bodies<'ast> {
    /// Each wrapped module's source with its splices, in bundle order.
    Spliced(Vec<Vec<(Span, Edit<'ast>)>>),
    Holes,
}

fn write<'plan, 'ast>(
    graph: &DependencyGraph<'ast>,
    plan: &'plan Plan<'ast>,
    version: LuaVersion,
    capacity: usize,
    bodies: Bodies<'ast>,
) -> BundleWriter<'plan, 'ast> {
    let mut writer = BundleWriter {
        output: Vec::with_capacity(capacity),
        text: String::new(),
        next_line: 1,
        line_map: Vec::with_capacity(graph.modules.len()),
        plan,
        version,
    };
    let prefix = plan.prefix.as_str();

    if let Some(entry) = graph.modules.get(graph.entry_id.0) {
        writer.push(&chunk_header(entry.source, entry.shebang, entry.hot_comments.iter().copied()));
    }

    writer.generate(|output| write_prelude(prefix, plan.loader, output));

    let mut spliced = match bodies {
        Bodies::Spliced(edits) => Some(edits.into_iter()),
        Bodies::Holes => None,
    };
    for item in &plan.items {
        match item {
            Item::Module(wrapped) => {
                let module_edits = spliced.as_mut().map(|edits| edits.next().unwrap_or_default());
                if let Some(module) = graph.modules.get(wrapped.id.0) {
                    writer.push_wrapped(wrapped, module, module_edits);
                }
            }
            Item::Registration(registration) => {
                writer.generate(|output| registration.write(prefix, &graph.modules, output));
            }
        }
    }

    writer.generate(|output| output.extend(["return ", prefix, "entry(...)\n"]));
    writer
}

struct BundleWriter<'plan, 'ast> {
    /// Bytes, as the modules' sources are, which it copies as written.
    output: Vec<u8>,
    /// Where [`Self::generate`] writes the text it adds, which the output
    /// then takes, so every generated piece shares one buffer.
    text: String,
    /// 1-based line the next pushed character lands on.
    next_line: usize,
    line_map: Vec<LineMapEntry>,
    plan: &'plan Plan<'ast>,
    version: LuaVersion,
}

impl BundleWriter<'_, '_> {
    fn push(&mut self, fragment: &[u8]) {
        self.output.extend_from_slice(fragment);
        self.next_line = self.next_line.saturating_add(line_breaks(fragment, self.version).count());
    }

    /// Writes generated text with `write`, counting its line breaks.
    fn generate(&mut self, write: impl FnOnce(&mut String)) {
        let mut text = mem::take(&mut self.text);
        text.clear();
        write(&mut text);
        self.push(text.as_bytes());
        self.text = text;
    }

    /// `edits` is `None` for a hole.
    fn push_wrapped(
        &mut self,
        wrapped: &WrappedModule<'_>,
        module: &ModuleInfo<'_>,
        edits: Option<Vec<(Span, Edit<'_>)>>,
    ) {
        let prefix = self.plan.prefix.as_str();
        self.generate(|output| {
            if let Some(label) = wrapped.label {
                write_provenance(label, &module.relative_path, output);
            }
            write_wrapper_opening(&wrapped.binding, wrapped.wrapper, prefix, output);
            module.exports.write_table_declaration(prefix, output);
            output.push('\n');
        });
        if let Some(body_edits) = edits {
            self.push_body(module, &body_edits);
        } else {
            self.generate(|output| {
                output.extend([prefix, HOLE, "()\n"]);
                module.exports.write_closing(prefix, output);
            });
        }
        self.push(wrapper_closing(wrapped.wrapper).as_bytes());
    }

    /// A loader call keeps the newline count of the call it replaces, so the
    /// line map stays 1:1 for multiline requires too.
    fn push_body(&mut self, module: &ModuleInfo<'_>, edits: &[(Span, Edit<'_>)]) {
        let source = module.source;
        let first_line = self.next_line;
        let body_start = self.output.len();
        let mut cursor = chunk_start(source, self.version);
        // A lone CR opening the body would join the generated LF above it into
        // one LFCR line break, losing the body's first line.
        let body = source.get(cursor..).unwrap_or_default();
        if self.version.has_lfcr_line_break()
            && body.starts_with(b"\r")
            && !body.starts_with(b"\r\n")
        {
            self.push(b" ");
        }
        for (span, edit) in edits {
            let range = Range::<usize>::from(*span);
            self.push(source.get(cursor..range.start).unwrap_or_default());
            self.push_edit(edit);
            cursor = range.end;
        }
        self.push(source.get(cursor..).unwrap_or_default());

        if module.exports.has_values {
            if self.output.len() == body_start || !ends_with_line_break(&self.output, self.version)
            {
                self.push(b"\n");
            }
            let prefix = self.plan.prefix.as_str();
            self.generate(|output| module.exports.write_closing(prefix, output));
        }
        if self.output.len() > body_start && !ends_with_line_break(&self.output, self.version) {
            self.push(b"\n");
        }

        self.line_map.push(LineMapEntry {
            bundle_start_line: first_line,
            bundle_end_line: self.next_line.saturating_sub(1),
            path: module.path.clone(),
        });
    }

    fn push_edit(&mut self, edit: &Edit<'_>) {
        let prefix = self.plan.prefix.as_str();
        match edit {
            &Edit::Require { key, line_breaks } => self.generate(|output| {
                output.extend([prefix, "require("]);
                key.write(output);
                output.extend(iter::repeat_n('\n', line_breaks));
                output.push(')');
            }),
            Edit::Dynamic => self.generate(|output| output.extend([prefix, "dynamic"])),
            Edit::Text(text) => self.push(text.as_bytes()),
        }
    }
}

/// The comment above a wrapper. A line break in a file name would end the
/// comment and turn the rest of the name into code, so it's written as an
/// escape.
fn write_provenance(label: Key<'_>, relative_path: &str, output: &mut String) {
    output.push_str("-- module ");
    label.write_label(output);
    output.push_str(": ");
    let mut rest = relative_path;
    while let Some(at) = rest.find(['\n', '\r']) {
        let (before, after) = rest.split_at(at);
        output.extend([before, if after.starts_with('\n') { "\\n" } else { "\\r" }]);
        rest = after.get(1..).unwrap_or_default();
    }
    output.extend([rest, "\n"]);
}

/// What opens a wrapped body. A statement the wrapper puts before the body
/// ends in `;`, so a body that opens with `(` can't continue it as a call.
fn write_wrapper_opening(
    binding: &Binding<'_>,
    wrapper: Wrapper,
    prefix: &str,
    output: &mut String,
) {
    let params = if wrapper.has_vararg_param { "..." } else { "" };
    let is_entry = matches!(binding, Binding::Entry);
    let write_target = |text: &mut String| match binding {
        Binding::Slot(key) => {
            text.extend([prefix, "modules["]);
            key.write(text);
            text.push(']');
        }
        Binding::Entry => text.extend([prefix, "entry"]),
    };
    if wrapper.has_private_env {
        if is_entry {
            output.push_str("local ");
            write_target(output);
            output.push(' ');
        }
        output.push_str("do local _ENV=_ENV ");
        write_target(output);
        output.extend(["=function(", params, ")_ENV=_ENV;"]);
        return;
    }
    if is_entry {
        output.push_str("local ");
    }
    write_target(output);
    output.extend(["=function(", params, ")"]);
    if wrapper.restores_arg {
        output.push_str("arg=_G.arg;");
    }
}

const fn wrapper_closing(wrapper: Wrapper) -> &'static str {
    if wrapper.has_private_env { "end end\n" } else { "end\n" }
}

/// Every module but the entry, in the order the bundle registers them.
fn non_entry_modules<'graph, 'ast>(
    graph: &'graph DependencyGraph<'ast>,
) -> impl Iterator<Item = (ModuleId, &'graph ModuleInfo<'ast>)> {
    graph
        .topological_order
        .iter()
        .filter(|id| **id != graph.entry_id)
        .filter_map(|id| graph.modules.get(id.0).map(|module| (*id, module)))
}

/// The first of `__luck_`, `__luck1_`, `__luck2_`, and so on that no module's
/// source holds.
fn choose_prefix(modules: &[ModuleInfo<'_>]) -> String {
    let mut candidate = format!("{PREFIX_STEM}_");
    let mut counter = 0usize;
    while modules.iter().any(|module| {
        module.has_prefix_stem && memmem::find(module.source, candidate.as_bytes()).is_some()
    }) {
        counter = counter.saturating_add(1);
        candidate = format!("{PREFIX_STEM}{counter}_");
    }
    candidate
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rustc_hash::FxHashMap;

    use super::*;
    use crate::discovery::{ParsedModule, parse_module};
    use crate::dynamic_require::DynamicRequire;

    struct ModuleFixture<'ast> {
        info: ModuleInfo<'ast>,
        resolved_paths: Vec<PathBuf>,
    }

    fn module<'ast>(
        allocator: &'ast Allocator,
        path: &str,
        source: &str,
        resolutions: &[(&str, &str)],
        version: LuaVersion,
    ) -> ModuleFixture<'ast> {
        let ParsedModule { mut info, sites, errors, .. } = parse_module(
            allocator,
            PathBuf::from(path),
            path.to_owned(),
            allocator.alloc_str(source).as_bytes(),
            version,
            DynamicRequire::default(),
        );
        assert!(errors.is_empty(), "fixture must parse: {errors:?}");
        let mut resolved_paths = Vec::new();
        info.dependencies = sites
            .into_iter()
            .map(|site| {
                let resolved = resolutions
                    .iter()
                    .find(|(require_string, _)| *require_string == site.require_string)
                    .unwrap_or_else(|| panic!("no resolution for {:?}", site.require_string));
                resolved_paths.push(PathBuf::from(resolved.1));
                Dependency {
                    require_string: site.require_string,
                    target: ModuleId(usize::MAX),
                    call_span: site.call_span,
                }
            })
            .collect();
        ModuleFixture { info, resolved_paths }
    }

    /// The graph of `fixtures`, given in topological order with the last one
    /// as entry, laid out as `DependencyGraph::build` lays it out, entry first.
    fn graph(mut fixtures: Vec<ModuleFixture<'_>>) -> DependencyGraph<'_> {
        fixtures.rotate_right(1);
        let ids_by_path: FxHashMap<PathBuf, ModuleId> = fixtures
            .iter()
            .enumerate()
            .map(|(i, fixture)| (fixture.info.path.clone(), ModuleId(i)))
            .collect();
        let modules: Vec<ModuleInfo<'_>> = fixtures
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

    fn emitted(
        allocator: &Allocator,
        fixtures: Vec<ModuleFixture<'_>>,
        version: LuaVersion,
    ) -> (String, Vec<LineMapEntry>) {
        let (output, line_map) = emit(allocator, &mut graph(fixtures), version)
            .unwrap_or_else(|errors| panic!("bundle failed: {errors:?}"));
        (String::from_utf8(output).unwrap(), line_map)
    }

    fn bundle(
        allocator: &Allocator,
        fixtures: Vec<ModuleFixture<'_>>,
        version: LuaVersion,
    ) -> String {
        emitted(allocator, fixtures, version).0
    }

    fn assert_reparses(output: &str, version: LuaVersion) {
        let allocator = Allocator::new();
        let result = luck_parser::parse(&allocator, output.as_bytes(), version);
        assert!(result.errors.is_empty(), "bundle must reparse: {:?}\n{output}", result.errors);
    }

    #[test]
    fn basic_bundle_uses_lazy_loader() {
        let allocator = Allocator::new();
        let modules = vec![
            module(
                &allocator,
                "src/utils.lua",
                "\
local M = {}
return M
",
                &[],
                LuaVersion::Lua54,
            ),
            module(
                &allocator,
                "src/main.lua",
                "\
local utils = require(\"utils\")
print(utils)
",
                &[("utils", "src/utils.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
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
        let allocator = Allocator::new();
        // One `local` per module would break past Lua's
        // 200-locals-per-function limit at ~199 modules.
        let mut modules = Vec::new();
        let mut entry_source = String::new();
        let mut resolutions: Vec<(String, String)> = Vec::new();
        for i in 0..500usize {
            modules.push(module(
                &allocator,
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
            &allocator,
            "src/main.lua",
            &entry_source,
            &borrowed_resolutions,
            LuaVersion::Lua54,
        ));
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert!(
            output.contains("__luck_modules[\"mod_499\"]=function(...)"),
            "all modules registered"
        );
        assert!(!output.contains("local __luck_src_mod"), "no per-module locals");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn required_entry_registers_under_its_require_string() {
        let allocator = Allocator::new();
        // The entry function registers under the require string, so a
        // module requiring the entry leaves no raw require() in the
        // output. The extra load runs a fresh instance, as real Lua does
        // when the main file is required a second time as a module.
        let modules = vec![
            module(
                &allocator,
                "src/b.lua",
                "\
local a = require(\"a\")
return { a = a }
",
                &[("a", "src/a.lua")],
                LuaVersion::Lua54,
            ),
            module(
                &allocator,
                "src/a.lua",
                "\
local b = require(\"b\")
return { b = b }
",
                &[("b", "src/b.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
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
        let allocator = Allocator::new();
        // Real Lua caches by name, so require("foo") and require("foo.init")
        // reaching one file run it twice. The slots share one function but
        // not a cache entry.
        let source = "\
local a = require(\"foo\")
local b = require(\"foo.init\")
print(a, b)
";
        let modules = vec![
            module(&allocator, "src/foo/init.lua", "return {}\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("foo", "src/foo/init.lua"), ("foo.init", "src/foo/init.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert!(output.contains("__luck_modules[\"foo\"]=function(...)"), "{output}");
        assert!(
            output.contains("__luck_modules[\"foo.init\"]=__luck_modules[\"foo\"]"),
            "{output}"
        );
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn generated_prefix_avoids_user_identifiers() {
        let allocator = Allocator::new();
        let source = "\
local __luck_require = 1
local d = require(\"dep\")
print(__luck_require, d)
";
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
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
        let allocator = Allocator::new();
        let source = "\
local d = require(
\"dep\"
)
print(d)
";
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let (output, line_map) = emitted(&allocator, modules, LuaVersion::Lua54);
        assert!(
            output.contains(
                "\
local d = __luck_require(\"dep\"

)"
            ),
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
        let allocator = Allocator::new();
        let source = "\
#!/usr/bin/env lua
local d = require(\"dep\")
print(d)
";
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let (output, line_map) = emitted(&allocator, modules, LuaVersion::Lua54);
        assert!(
            output.starts_with(
                "\
#!/usr/bin/env lua
local __luck_modules={}
"
            ),
            "{output}"
        );
        assert_eq!(output.matches("#!").count(), 1, "{output}");
        let entry = find_line_entry(&line_map, "src/main.lua");
        let output_lines: Vec<&str> = output.lines().collect();
        assert_eq!(output_lines[entry.bundle_start_line - 1], "");
        assert_eq!(output_lines[entry.bundle_start_line - 1 + 2], "print(d)");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn lua51_loader_uses_sentinel_and_no_paths() {
        let allocator = Allocator::new();
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua51),
            module(
                &allocator,
                "src/main.lua",
                "\
local d = require(\"dep\")
print(d)
",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua51,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua51);
        assert!(output.contains("loop or previous error loading module"), "{output}");
        assert!(
            !output.contains("__luck_paths"),
            "5.1 chunks receive only the module name: {output}"
        );
        assert_reparses(&output, LuaVersion::Lua51);
    }

    #[test]
    fn lua51_bundle_writes_no_global_by_default() {
        let allocator = Allocator::new();
        let source = "\
local d = require(\"dep\")
local name = \"dep\"
print(d, require(name))
";
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua51),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua51,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua51);
        assert!(output.contains("local function __luck_require(name)"), "{output}");
        assert!(output.contains("local function __luck_dynamic(name)"), "{output}");
        assert!(!output.contains("\nfunction __luck_"), "{output}");
        assert_reparses(&output, LuaVersion::Lua51);
    }

    fn lua51_bundle_capturing(captures: usize) -> String {
        let allocator = Allocator::new();
        let locals =
            (0..captures).map(|i| format!("local u{i} = {i}\n")).collect::<Vec<_>>().concat();
        let sum = (0..captures).map(|i| format!("u{i}")).collect::<Vec<_>>().join(" + ");
        let source = format!(
            "\
{locals}local function f()
    return require(\"dep\"), {sum}
end
print(f())
"
        );
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua51),
            module(
                &allocator,
                "src/main.lua",
                &source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua51,
            ),
        ];
        bundle(&allocator, modules, LuaVersion::Lua51)
    }

    #[test]
    fn lua51_loader_goes_global_only_without_room_for_its_upvalues() {
        let with_room = lua51_bundle_capturing(58);
        assert!(with_room.contains("local function __luck_require(name)"), "{with_room}");

        let without_room = lua51_bundle_capturing(59);
        assert!(
            without_room.contains("\n;(function()function __luck_require(name)"),
            "{without_room}"
        );
        assert!(!without_room.contains("local function __luck_require"), "{without_room}");
        assert_reparses(&without_room, LuaVersion::Lua51);
    }

    #[test]
    fn lua52_loader_retries_after_error_and_passes_path() {
        let allocator = Allocator::new();
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua52),
            module(
                &allocator,
                "src/main.lua",
                "\
local d = require(\"dep\")
print(d)
",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua52,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua52);
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
        let allocator = Allocator::new();
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                "\
local d = require(\"dep\")
print(d)
",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert!(output.contains(":preload:"), "{output}");
        assert!(output.contains("return value,loaderdata"), "{output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn luau_loader_checks_single_return_and_uses_numeric_slots() {
        let allocator = Allocator::new();
        let modules = vec![
            module(&allocator, "src/dep.luau", "return {}\n", &[], LuaVersion::Luau),
            module(
                &allocator,
                "src/main.luau",
                "\
local m = require(\"./dep\") :: any
print(m)
",
                &[("./dep", "src/dep.luau")],
                LuaVersion::Luau,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Luau);
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
        let allocator = Allocator::new();
        let source = "\
local d = require(\"dep\")
local function f()
    local require = print
    require(\"dep\")
end
f()
print(d)
";
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert!(output.contains("local d = __luck_require(\"dep\")"), "{output}");
        assert!(
            output.contains("    require(\"dep\")"),
            "shadowed call keeps user semantics: {output}"
        );
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn luau_value_exports_are_lowered_inside_bundle_wrappers() {
        let allocator = Allocator::new();
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
            module(&allocator, "src/dep.luau", dependency, &[], LuaVersion::Luau),
            module(
                &allocator,
                "src/main.luau",
                "\
local dep = require(\"./dep\")
print(dep.get())
",
                &[("./dep", "src/dep.luau")],
                LuaVersion::Luau,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Luau);

        assert!(!output.contains("export "), "{output}");
        assert!(output.contains("local value = 129312i"), "{output}");
        assert!(output.contains("const LIMIT = 0xffffffffffffffffi"), "{output}");
        assert!(
            output.contains(
                "\
@native
local function get()"
            ),
            "{output}"
        );
        assert!(
            output.contains("return __luck_freeze({value=value,LIMIT=LIMIT,get=get})"),
            "{output}"
        );
        assert_reparses(&output, LuaVersion::Luau);
    }

    #[test]
    fn nested_and_field_access_requires_are_rewritten() {
        let allocator = Allocator::new();
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
            module(&allocator, "src/dep.lua", "return { value = 1 }\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert!(output.contains("local field = __luck_require(\"dep\").value"), "{output}");
        assert!(output.contains("return __luck_require(\"dep\")"), "{output}");
        assert!(output.contains("local inner = __luck_require(\"dep\")"), "{output}");
        assert!(!output.contains("= require(\"dep\")"), "no raw require may survive: {output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    #[test]
    fn require_inside_string_literal_is_untouched() {
        let allocator = Allocator::new();
        let source = "\
local d = require(\"dep\")
local msg = 'use require(\"dep\") to load'
print(d, msg)
";
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert!(output.contains("local d = __luck_require(\"dep\")"), "{output}");
        assert!(output.contains("require(\"dep\") to load"), "string literal corrupted: {output}");
    }

    #[test]
    fn escaped_require_string_is_rewritten_to_decoded_name() {
        let allocator = Allocator::new();
        // require("a\46b") is require("a.b"); both spellings share one
        // cache entry keyed by the decoded name.
        let source = "\
local x = require(\"a\\46b\")
local y = require(\"a.b\")
print(x, y)
";
        let modules = vec![
            module(&allocator, "src/a/b.lua", "return {}\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                source,
                &[("a.b", "src/a/b.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert_eq!(output.matches("__luck_require(\"a.b\")").count(), 2, "{output}");
        assert!(output.contains("__luck_modules[\"a.b\"]=function(...)"), "{output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }

    fn find_line_entry<'map>(line_map: &'map [LineMapEntry], path: &str) -> &'map LineMapEntry {
        line_map
            .iter()
            .find(|entry| entry.path == Path::new(path))
            .unwrap_or_else(|| panic!("no line map entry for {path}"))
    }

    #[test]
    fn line_map_maps_module_lines_to_original_source_lines() {
        let allocator = Allocator::new();
        let utils_source = "\
local M = {}
function M.foo()
    return 42
end
return M
";
        let entry_source = "\
local utils = require(\"utils\")
local helper = require(\"helper\")
print(utils, helper)
";
        // Leading comments and a blank line make the offsets nontrivial.
        let helper_source = "\
-- helper module
-- second comment line

local helper = { ready = true }
return helper
";
        let modules = vec![
            module(&allocator, "src/utils.lua", utils_source, &[], LuaVersion::Lua54),
            module(&allocator, "src/helper.lua", helper_source, &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                entry_source,
                &[("utils", "src/utils.lua"), ("helper", "src/helper.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let (output, line_map) = emitted(&allocator, modules, LuaVersion::Lua54);
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
        let allocator = Allocator::new();
        let source = "\
print(\"a\")
print(\"b\")
print(\"c\")
";
        let modules = vec![module(&allocator, "src/main.lua", source, &[], LuaVersion::Lua54)];
        let (output, line_map) = emitted(&allocator, modules, LuaVersion::Lua54);
        assert_eq!(output, source);
        assert_eq!(
            line_map,
            [LineMapEntry {
                bundle_start_line: 1,
                bundle_end_line: 3,
                path: PathBuf::from("src/main.lua"),
            }]
        );
    }

    #[test]
    fn line_break_in_a_module_path_stays_inside_the_provenance_comment() {
        let allocator = Allocator::new();
        let path = "src/a\nerror(1)--.lua";
        let modules = vec![
            module(&allocator, path, "return 1\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                "\
local d = require(\"dep\")
print(d)
",
                &[("dep", path)],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert!(
            !output.lines().any(|line| line.starts_with("error(1)")),
            "a module path must not escape its comment:\n{output}"
        );
    }

    #[test]
    fn long_bracket_require_is_rewritten() {
        let allocator = Allocator::new();
        let modules = vec![
            module(&allocator, "src/dep.lua", "return 1\n", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "src/main.lua",
                "\
local d = require [[dep]]
print(d)
",
                &[("dep", "src/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        assert!(output.contains("local d = __luck_require(\"dep\")"), "{output}");
        assert!(!output.contains("require [[dep]]"), "{output}");
    }

    #[test]
    fn loader_keys_escape_with_encode_string() {
        let allocator = Allocator::new();
        let modules = vec![
            module(&allocator, "/p/dep.lua", "return 1", &[], LuaVersion::Lua54),
            module(
                &allocator,
                "/p/main.lua",
                "local d = require(\"a\\tb\\ac\\u{e9}\\0015\")",
                &[("a\tb\u{7}c\u{e9}\u{1}5", "/p/dep.lua")],
                LuaVersion::Lua54,
            ),
        ];
        let output = bundle(&allocator, modules, LuaVersion::Lua54);
        // `\t` and `\a` stay symbolic, UTF-8 stays literal, and a control
        // byte before a digit stays padded.
        assert!(output.contains("__luck_require(\"a\\tb\\ac\u{e9}\\0015\")"), "{output}");
        assert_reparses(&output, LuaVersion::Lua54);
    }
}
