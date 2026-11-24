use luck_ast::MAX_NESTING_DEPTH;
use luck_token::LuaVersion;
use memchr::memmem;
use rustc_hash::FxHashSet;

use crate::graph::DependencyGraph;
use crate::loader::LoaderShape;
use crate::module::{Dependency, ModuleId, ModuleInfo};

/// What every generated name's prefix starts with: `__luck_`, unless a
/// module's source holds that, then `__luck1_`, `__luck2_`, and so on.
pub(crate) const PREFIX_STEM: &str = "__luck";

/// Upvalues a rewritten function can gain: the loader and its dynamic entry.
const LOADER_UPVALUES: usize = 2;

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
        module.nesting_depth + self.levels() + usize::from(module.exports.has_table())
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
                    module.max_function_upvalues + LOADER_UPVALUES > version.max_upvalues()
                }),
                has_dynamic_entry: modules.iter().any(|module| !module.dynamic_callees.is_empty()),
                has_export_runtime: modules.iter().any(|module| module.exports.has_values),
            },
            slots: None,
            items: Vec::with_capacity(modules.len() * 2),
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
        match &self.slots {
            Some(slots) => slots[dependency.target.0].map(Key::Slot),
            None => Some(Key::Name(dependency.require_string)),
        }
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
        for (slot, (id, _)) in (1..).zip(non_entry_modules(graph)) {
            slots[id.0] = Some(slot);
        }
        let is_entry_required = graph
            .modules
            .iter()
            .flat_map(|module| &module.dependencies)
            .any(|dependency| dependency.target == graph.entry_id);
        // The slot after every other module's.
        let entry_slot = is_entry_required.then_some(graph.modules.len());
        slots[graph.entry_id.0] = entry_slot;
        self.slots = Some(slots);

        for (slot, (id, module)) in (1..).zip(non_entry_modules(graph)) {
            let key = Key::Slot(slot);
            self.push_module(id, module, version, Some(key), Binding::Slot(key));
            if module.exports.has_values {
                self.items.push(Item::Registration(Registration::Exporting { slot }));
            }
        }

        let entry = &graph[graph.entry_id];
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
        let ordered_dependencies =
            graph.topological_order.iter().flat_map(|id| &graph[*id].dependencies);
        for dependency in ordered_dependencies {
            if seen_strings.insert(dependency.require_string) {
                strings_by_module[dependency.target.0].push(dependency.require_string);
            }
        }

        for (id, module) in non_entry_modules(graph) {
            let Some((&primary, aliases)) = strings_by_module[id.0].split_first() else {
                continue;
            };
            let first = Key::Name(primary);
            self.push_module(id, module, version, Some(first), Binding::Slot(first));
            for &alias in aliases {
                let registration = Registration::Alias { alias: Key::Name(alias), primary: first };
                self.items.push(Item::Registration(registration));
            }
            self.push_path_registrations(id, &strings_by_module[id.0], version);
        }

        let entry = &graph[graph.entry_id];
        self.push_module(graph.entry_id, entry, version, None, Binding::Entry);
        let entry_strings = &strings_by_module[graph.entry_id.0];
        for &string in entry_strings {
            let registration = Registration::Entry { key: Key::Name(string), exporting_slot: None };
            self.items.push(Item::Registration(registration));
        }
        self.push_path_registrations(graph.entry_id, entry_strings, version);
    }

    fn push_path_registrations(
        &mut self,
        module: ModuleId,
        strings: &[&'ast str],
        version: LuaVersion,
    ) {
        if !version.has_require_filepath_arg() {
            return;
        }
        for &string in strings {
            let registration = Registration::Path { name: Key::Name(string), module };
            self.items.push(Item::Registration(registration));
        }
    }
}

/// Every module but the entry, in the order the bundle registers them.
fn non_entry_modules<'graph, 'ast>(
    graph: &'graph DependencyGraph<'ast>,
) -> impl Iterator<Item = (ModuleId, &'graph ModuleInfo<'ast>)> {
    graph.topological_order.iter().filter(|id| **id != graph.entry_id).map(|id| (*id, &graph[*id]))
}

/// The first of `__luck_`, `__luck1_`, `__luck2_`, and so on that no module's
/// source holds.
fn choose_prefix(modules: &[ModuleInfo<'_>]) -> String {
    let mut candidate = format!("{PREFIX_STEM}_");
    let mut counter = 0;
    while modules.iter().any(|module| {
        module.has_prefix_stem && memmem::find(module.source, candidate.as_bytes()).is_some()
    }) {
        counter += 1;
        candidate = format!("{PREFIX_STEM}{counter}_");
    }
    candidate
}
