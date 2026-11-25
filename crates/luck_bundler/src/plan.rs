use luck_ast::MAX_NESTING_DEPTH;
use memchr::memmem;

use crate::graph::DependencyGraph;
use crate::module::{Dependency, ModuleId, ModuleInfo};

/// What every generated name's prefix starts with: `__luck_`, unless a
/// module's source holds that, then `__luck1_`, `__luck2_`, and so on.
pub(crate) const PREFIX_STEM: &str = "__luck";

/// How many levels deeper a wrapped body nests than the module's own chunk,
/// as [`MAX_NESTING_DEPTH`] counts them.
pub(crate) const WRAPPER_LEVELS: usize = 2;

/// What a bundle holds, which its text and its tree both follow.
///
/// Modules register as functions in a table, each in a numeric slot, and
/// load lazily on first require, through a loader that mirrors Luau's own
/// `require`: it keys the cache by resolved file, calls chunks with no
/// arguments, and errors unless a module returns exactly one value. A
/// value-export module in a cycle hands out a placeholder.
///
/// The entry body is wrapped in a function called with the chunk's
/// varargs, so CLI args pass straight through, and a module that requires
/// the entry gets a fresh instance, as when the main file is also required
/// as a module.
///
/// Every generated identifier shares a prefix that appears nowhere in any
/// module source, so user code can't capture or shadow loader internals.
///
/// Each module registers in a top-level statement, so the only nesting a
/// module gains is its own wrapper's.
pub(crate) struct Plan {
    pub prefix: String,
    /// A module exports a value, so the bundle needs the export runtime.
    pub has_export_runtime: bool,
    /// Each module's slot, by `ModuleId`. The entry's, after every other
    /// module's, holds it only when something requires it.
    slots: Vec<usize>,
    /// In bundle order.
    pub items: Vec<Item>,
}

pub(crate) enum Item {
    Module(WrappedModule),
    Registration(Registration),
}

pub(crate) struct WrappedModule {
    pub id: ModuleId,
    pub binding: Binding,
}

/// What a wrapped module's function is bound to.
#[derive(Clone, Copy)]
pub(crate) enum Binding {
    /// A slot of the module table, such as `__luck_modules[1]`, which the
    /// comment above the wrapper names the module by.
    Slot(usize),
    /// The entry's own local, which the bundle calls last.
    Entry,
}

/// A statement that registers a module's function under a slot.
pub(crate) enum Registration {
    /// A value-export module's function, wrapped so a cycle gets a
    /// placeholder: `P_modules[slot]=P_exporting(slot,P_modules[slot])`.
    Exporting { slot: usize },
    /// The entry's function, wrapped like a value-export module's when it
    /// exports a value.
    Entry { slot: usize, is_exporting: bool },
}

/// Whether `module`'s body could nest past [`MAX_NESTING_DEPTH`] in its
/// wrapper, measured as its parse counted it. Lowering an export to a field
/// of the export table reads it one level deeper.
pub(crate) fn may_overflow(module: &ModuleInfo<'_>) -> bool {
    module.nesting_depth + WRAPPER_LEVELS + usize::from(module.exports.has_table())
        > MAX_NESTING_DEPTH
}

impl Plan {
    /// One slot per module, in topological order, and the entry's last.
    pub(crate) fn new(graph: &DependencyGraph<'_>) -> Self {
        let modules = &graph.modules;
        let mut slots = vec![0; modules.len()];
        let mut items = Vec::with_capacity(modules.len() * 2);
        let non_entry_modules = graph
            .topological_order
            .iter()
            .filter(|id| **id != graph.entry_id)
            .map(|id| (*id, &graph[*id]));
        for (slot, (id, module)) in (1..).zip(non_entry_modules) {
            slots[id.0] = slot;
            items.push(Item::Module(WrappedModule { id, binding: Binding::Slot(slot) }));
            if module.exports.has_values {
                items.push(Item::Registration(Registration::Exporting { slot }));
            }
        }

        let entry_slot = modules.len();
        slots[graph.entry_id.0] = entry_slot;
        items.push(Item::Module(WrappedModule { id: graph.entry_id, binding: Binding::Entry }));
        let is_entry_required = modules
            .iter()
            .flat_map(|module| &module.dependencies)
            .any(|dependency| dependency.target == graph.entry_id);
        if is_entry_required {
            let is_exporting = graph[graph.entry_id].exports.has_values;
            items.push(Item::Registration(Registration::Entry { slot: entry_slot, is_exporting }));
        }

        Self {
            prefix: choose_prefix(modules),
            has_export_runtime: modules.iter().any(|module| module.exports.has_values),
            slots,
            items,
        }
    }

    /// Each wrapped module, in bundle order.
    pub(crate) fn wrapped_modules(&self) -> impl Iterator<Item = &WrappedModule> {
        self.items.iter().filter_map(|item| match item {
            Item::Module(wrapped) => Some(wrapped),
            Item::Registration(_) => None,
        })
    }

    /// The slot of the module `dependency` loads.
    pub(crate) fn slot(&self, dependency: &Dependency<'_>) -> usize {
        self.slots[dependency.target.0]
    }
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
