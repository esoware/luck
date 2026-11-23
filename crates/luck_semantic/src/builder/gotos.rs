//! Lua 5.2+: each `goto` resolved to the label it jumps to, as `lparser.c`
//! resolves them.
//!
//! Each function body resolves its own labels. A label is visible in its
//! whole block and the blocks inside it, and a `goto` takes the innermost
//! visible label of its name. Duplicate visible labels and unresolved gotos
//! are errors, and a forward goto may not jump into the scope of a variable
//! unless its label ends the block: only labels follow it, and the block
//! isn't a repeat body, whose `until` still sees the locals.

use core::mem;

use luck_ast::NameIndex;
use luck_diagnostics::Diagnostic;
use luck_token::{LuaVersion, Span};
use rustc_hash::FxHashMap;

use crate::checker::{duplicate_label, goto_into_local_scope, unresolved_goto};
use crate::id::{LabelId, NameId};
use crate::scoping::{NameRole, Scoping};

struct PendingGoto {
    index: Option<NameIndex>,
    span: Span,
    /// Active variables at the goto, lowered to the enclosing block's count
    /// each time the goto leaves a block.
    active_vars: usize,
}

/// Labels and gotos are looked up by name, so a block's cost follows the
/// number of them and not the square of it.
#[derive(Default)]
struct BlockGotos {
    labels: FxHashMap<NameId, LabelId>,
    /// Gotos no label has taken yet, each name's in source order.
    pending: FxHashMap<NameId, Vec<PendingGoto>>,
}

/// The enclosing block's labels and gotos, which wait here while a block is
/// open.
#[derive(Default)]
pub(super) struct BlockStart {
    active_vars: usize,
    enclosing: Option<Box<BlockGotos>>,
}

/// One function body's labels and gotos, as `dyd->label` and `dyd->gt` hold
/// them between `firstlabel` and `firstgoto`.
#[derive(Default)]
pub(super) struct FunctionGotos {
    /// The innermost open block's, or the function's own once every block
    /// has closed, where a goto that left every block waits to be reported.
    /// `None` for a block with neither, as most are, so opening and closing
    /// one costs nothing.
    block: Option<Box<BlockGotos>>,
    /// For each name a label of an open block has, the first such label and
    /// how many open blocks have one.
    visible: FxHashMap<NameId, (LabelId, u32)>,
}

/// Where resolution records what it finds: the labels a goto reaches, and,
/// when checking, the errors.
pub(super) struct Sink<'sink> {
    pub scoping: &'sink mut Scoping,
    pub errors: Option<&'sink mut Vec<Diagnostic>>,
}

impl Sink<'_> {
    fn resolve(&mut self, index: Option<NameIndex>, label: LabelId) {
        if let Some(entry) = self.scoping.label_mut(label) {
            entry.is_used = true;
        }
        self.scoping.set_role(index, NameRole::Label(label));
    }

    fn report(&mut self, error: impl FnOnce(&str) -> Diagnostic, name: NameId) {
        if let Some(errors) = self.errors.as_deref_mut() {
            errors.push(error(self.scoping.name(name)));
        }
    }
}

#[derive(Default)]
pub(super) struct Gotos {
    function: FunctionGotos,
    /// Closed blocks' tables, emptied, which keep their room for the next
    /// block with a label or a goto, so tables are made only as deep as the
    /// blocks that need them nest.
    #[expect(
        clippy::vec_box,
        reason = "a spare table moves to a block's boxed slot and back without allocating"
    )]
    spare: Vec<Box<BlockGotos>>,
}

impl Gotos {
    pub(super) const fn open_block(&mut self, active_vars: usize) -> BlockStart {
        BlockStart { active_vars, enclosing: self.function.block.take() }
    }

    /// Close the current block. A goto still pending leaves it: a label of the
    /// enclosing block already seen takes it as a backward jump, and otherwise
    /// it waits there with the enclosing block's variable count.
    pub(super) fn close_block(&mut self, start: BlockStart, sink: &mut Sink<'_>) {
        let function = &mut self.function;
        let Some(mut closed) = mem::replace(&mut function.block, start.enclosing) else {
            return;
        };
        #[expect(clippy::iter_over_hash_type, reason = "each name's count updates on its own")]
        for name in closed.labels.keys() {
            if let Some((_, count)) = function.visible.get_mut(name) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    function.visible.remove(name);
                }
            }
        }
        closed.labels.clear();
        if !closed.pending.is_empty() {
            let spare = &mut self.spare;
            let enclosing = function.block.get_or_insert_with(|| spare.pop().unwrap_or_default());
            #[expect(clippy::iter_over_hash_type, reason = "each name's gotos merge on their own")]
            for (name, gotos) in closed.pending.drain() {
                if let Some(&label) = enclosing.labels.get(&name) {
                    for goto in gotos {
                        sink.resolve(goto.index, label);
                    }
                    continue;
                }
                // The enclosing block's own gotos of this name came first.
                enclosing.pending.entry(name).or_default().extend(
                    gotos
                        .into_iter()
                        .map(|goto| PendingGoto { active_vars: start.active_vars, ..goto }),
                );
            }
        }
        self.spare.push(closed);
    }

    /// Record `label`, named `name`, whose scope starts at `active_vars`,
    /// solving the gotos pending in this block with its name.
    pub(super) fn add_label(
        &mut self,
        name: NameId,
        label: LabelId,
        active_vars: usize,
        version: LuaVersion,
        sink: &mut Sink<'_>,
    ) {
        let function = &mut self.function;
        let spare = &mut self.spare;
        let block = function.block.get_or_insert_with(|| spare.pop().unwrap_or_default());
        let defined = if version.has_label_shadowing() {
            block.labels.get(&name).copied()
        } else {
            function.visible.get(&name).map(|&(first, _)| first)
        };
        if let Some(first) = defined {
            let (span, first_span) =
                (sink.scoping.label(label).span, sink.scoping.label(first).span);
            sink.report(|text| duplicate_label(span, text, first_span), name);
        }
        for goto in block.pending.remove(&name).unwrap_or_default() {
            if goto.active_vars < active_vars {
                sink.report(|text| goto_into_local_scope(goto.span, text), name);
            }
            sink.resolve(goto.index, label);
        }
        if let Some(previous) = block.labels.insert(name, label) {
            block.labels.insert(name, previous);
        } else {
            let (_, count) = function.visible.entry(name).or_insert((label, 0));
            *count = count.saturating_add(1);
        }
    }

    /// Record a goto to `name` at `active_vars` active variables, a backward
    /// jump if this block already has its label.
    pub(super) fn add_goto(
        &mut self,
        name: NameId,
        index: Option<NameIndex>,
        span: Span,
        active_vars: usize,
        sink: &mut Sink<'_>,
    ) {
        let spare = &mut self.spare;
        let block = self.function.block.get_or_insert_with(|| spare.pop().unwrap_or_default());
        if let Some(&label) = block.labels.get(&name) {
            sink.resolve(index, label);
        } else {
            block.pending.entry(name).or_default().push(PendingGoto { index, span, active_vars });
        }
    }

    pub(super) fn enter_function(&mut self) -> FunctionGotos {
        mem::take(&mut self.function)
    }

    /// Report the gotos no label in this function took, in source order, then
    /// return to the enclosing function's state.
    pub(super) fn leave_function(&mut self, enclosing: FunctionGotos, sink: &mut Sink<'_>) {
        let function = mem::replace(&mut self.function, enclosing);
        let Some(mut block) = function.block else {
            return;
        };
        if sink.errors.is_some() {
            let mut unresolved: Vec<(Span, NameId)> = block
                .pending
                .iter()
                .flat_map(|(&name, gotos)| gotos.iter().map(move |goto| (goto.span, name)))
                .collect();
            unresolved.sort_unstable_by_key(|&(span, _)| span.start);
            for (span, name) in unresolved {
                sink.report(|text| unresolved_goto(span, text), name);
            }
        }
        block.labels.clear();
        block.pending.clear();
        self.spare.push(block);
    }
}
