//! Lua 5.2+: each `goto` resolved to the label it jumps to, as `lparser.c`
//! resolves them.
//!
//! Each function body resolves its own labels. A label is visible in its
//! whole block and the blocks inside it, and a `goto` takes the innermost
//! visible label of its name. Duplicate visible labels and unresolved gotos
//! are errors, and a forward goto may not jump into the scope of a variable
//! unless its label ends the block: only labels follow it, and the block
//! isn't a repeat body, whose `until` still sees the locals.
//!
//! Every open block's labels share one stack, and the gotos still waiting
//! for a label another, which each block and function marks where its own
//! start in, as `lparser.c` keeps them in `dyd->label` and `dyd->gt` and
//! `oxc_semantic` keeps its labels, so no block, function, label, or goto
//! allocates. Each name links its innermost open label and its last waiting
//! goto to the ones of its name before them, so a block's cost follows the
//! number of its labels and gotos and not the square of it.

use luck_ast::NameIndex;
use luck_diagnostics::Diagnostic;
use luck_token::{LuaVersion, Span};

use crate::checker::{duplicate_label, goto_into_local_scope, unresolved_goto};
use crate::id::{LabelId, NameId};
use crate::scoping::{NameRole, Scoping};

struct OpenLabel {
    name: NameId,
    label: LabelId,
    /// Where the open label of its name it hides is.
    shadowed: Option<u32>,
}

#[derive(Clone, Copy)]
struct PendingGoto {
    name: NameId,
    index: Option<NameIndex>,
    span: Span,
    /// Active variables at the goto, lowered to the enclosing block's count
    /// each time the goto leaves a block.
    active_vars: usize,
    /// Where the waiting goto of its name before it is.
    older: Option<u32>,
    /// A label of its block took it, and it goes when the block closes.
    is_resolved: bool,
}

/// Where a name's innermost open label and its last waiting goto are.
#[derive(Clone, Copy, Default)]
struct Heads {
    label: Option<u32>,
    goto: Option<u32>,
}

/// Where the enclosing block's labels and gotos start, which it gets back
/// when the block inside it closes.
#[derive(Clone, Copy, Default)]
pub(super) struct BlockStart {
    active_vars: usize,
    labels: usize,
    pending: usize,
}

/// Where the enclosing function's labels, and its current block's labels
/// and gotos, start, which it gets back when the function inside it ends.
#[derive(Clone, Copy, Default)]
pub(super) struct FunctionStart {
    block_labels: usize,
    block_pending: usize,
    function_labels: usize,
}

/// Where resolution records what it finds: the labels a goto reaches, and,
/// when checking, the errors.
pub(super) struct Sink<'sink> {
    pub scoping: &'sink mut Scoping,
    pub errors: Option<&'sink mut Vec<Diagnostic>>,
}

impl Sink<'_> {
    fn resolve(&mut self, index: Option<NameIndex>, label: LabelId) {
        self.scoping.label_mut(label).is_used = true;
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
    /// The labels of every open block, outermost first.
    labels: Vec<OpenLabel>,
    /// The gotos of every open block that no label has taken yet, in source
    /// order.
    pending: Vec<PendingGoto>,
    /// By name.
    heads: Vec<Heads>,
    block_labels: usize,
    block_pending: usize,
    function_labels: usize,
}

impl Gotos {
    pub(super) const fn open_block(&mut self, active_vars: usize) -> BlockStart {
        let start =
            BlockStart { active_vars, labels: self.block_labels, pending: self.block_pending };
        self.block_labels = self.labels.len();
        self.block_pending = self.pending.len();
        start
    }

    /// Close the current block. A goto still pending leaves it: a label of the
    /// enclosing block already seen takes it as a backward jump, and otherwise
    /// it waits there with the enclosing block's variable count.
    pub(super) fn close_block(&mut self, start: BlockStart, sink: &mut Sink<'_>) {
        let first_goto = self.block_pending;
        for open in self.labels.drain(self.block_labels..).rev() {
            self.heads[open.name.index()].label = open.shadowed;
        }
        self.block_labels = start.labels;
        self.block_pending = start.pending;
        let mut kept = first_goto;
        for i in first_goto..self.pending.len() {
            let goto = self.pending[i];
            if goto.is_resolved {
                continue;
            }
            let heads = self.heads_of(goto.name);
            if let Some(label) = self.label_from(heads, start.labels) {
                // A label already seen takes it backward, into no variable's
                // scope.
                self.take_pending(goto.name, first_goto, label, 0, sink);
                continue;
            }
            let older = match goto.older {
                // Each goto of its name before it in the block moved first.
                Some(at) if at as usize >= first_goto => heads.goto,
                older => older,
            };
            self.pending[kept] = PendingGoto { active_vars: start.active_vars, older, ..goto };
            let kept_at = u32::try_from(kept).expect("a chunk has fewer gotos than `u32::MAX`");
            self.set_heads(goto.name, Heads { goto: Some(kept_at), ..heads });
            kept += 1;
        }
        self.pending.truncate(kept);
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
        let heads = self.heads_of(name);
        let visible_from =
            if version.has_label_shadowing() { self.block_labels } else { self.function_labels };
        if let Some(mut first) = heads.label.filter(|&at| at as usize >= visible_from) {
            while let Some(shadowed) =
                self.labels[first as usize].shadowed.filter(|&at| at as usize >= visible_from)
            {
                first = shadowed;
            }
            let open = &self.labels[first as usize];
            let (span, first_span) =
                (sink.scoping.label(label).span, sink.scoping.label(open.label).span);
            sink.report(|text| duplicate_label(span, text, first_span), name);
        }
        self.take_pending(name, self.block_pending, label, active_vars, sink);
        // A second label of the name in one block leaves the first visible.
        if self.label_from(heads, self.block_labels).is_none() {
            let at = Some(
                u32::try_from(self.labels.len()).expect("a chunk has fewer labels than `u32::MAX`"),
            );
            self.labels.push(OpenLabel { name, label, shadowed: heads.label });
            self.set_heads(name, Heads { label: at, ..self.heads_of(name) });
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
        let heads = self.heads_of(name);
        if let Some(label) = self.label_from(heads, self.block_labels) {
            sink.resolve(index, label);
            return;
        }
        let at = Some(
            u32::try_from(self.pending.len()).expect("a chunk has fewer gotos than `u32::MAX`"),
        );
        self.pending.push(PendingGoto {
            name,
            index,
            span,
            active_vars,
            older: heads.goto,
            is_resolved: false,
        });
        self.set_heads(name, Heads { goto: at, ..heads });
    }

    pub(super) const fn enter_function(&mut self) -> FunctionStart {
        let enclosing = FunctionStart {
            block_labels: self.block_labels,
            block_pending: self.block_pending,
            function_labels: self.function_labels,
        };
        self.block_labels = self.labels.len();
        self.block_pending = self.pending.len();
        self.function_labels = self.labels.len();
        enclosing
    }

    /// Report the gotos no label in this function took, in source order, then
    /// return to the enclosing function's state.
    pub(super) fn leave_function(&mut self, enclosing: FunctionStart, sink: &mut Sink<'_>) {
        let first_goto = self.block_pending;
        if sink.errors.is_some() {
            for goto in &self.pending[first_goto..] {
                sink.report(|text| unresolved_goto(goto.span, text), goto.name);
            }
        }
        for goto in self.pending.drain(first_goto..).rev() {
            self.heads[goto.name.index()].goto = goto.older;
        }
        self.block_labels = enclosing.block_labels;
        self.block_pending = enclosing.block_pending;
        self.function_labels = enclosing.function_labels;
    }

    fn heads_of(&self, name: NameId) -> Heads {
        self.heads.get(name.index()).copied().unwrap_or_default()
    }

    fn set_heads(&mut self, name: NameId, heads: Heads) {
        let index = name.index();
        if self.heads.len() <= index {
            self.heads.resize(index + 1, Heads::default());
        }
        self.heads[index] = heads;
    }

    /// The innermost open label in `heads`, if it's at or after `from`.
    fn label_from(&self, heads: Heads, from: usize) -> Option<LabelId> {
        let at = heads.label? as usize;
        (at >= from).then(|| self.labels[at].label)
    }

    /// Hands `label` every goto to `name` waiting at or after `from`, each
    /// a jump into the scope of a variable when it has fewer active than
    /// `label_vars`.
    fn take_pending(
        &mut self,
        name: NameId,
        from: usize,
        label: LabelId,
        label_vars: usize,
        sink: &mut Sink<'_>,
    ) {
        let mut next = self.heads_of(name).goto;
        while let Some(at) = next.filter(|&at| at as usize >= from) {
            let slot = &mut self.pending[at as usize];
            slot.is_resolved = true;
            let goto = *slot;
            if goto.active_vars < label_vars {
                sink.report(|text| goto_into_local_scope(goto.span, text), name);
            }
            sink.resolve(goto.index, label);
            next = goto.older;
        }
        self.set_heads(name, Heads { goto: next, ..self.heads_of(name) });
    }
}
