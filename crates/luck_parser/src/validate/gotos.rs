//! Goto/label resolution (5.2+), following the reference implementation.
//!
//! [`super::scope`] drives this as it walks the active-variable stack. Each
//! function body resolves its own labels. Duplicate visible labels and
//! unresolved gotos are errors, and a forward goto may not jump into the scope
//! of a variable unless its label ends the block: only labels and `;` follow
//! it, and the block isn't a repeat body, whose `until` still sees the locals.

use core::mem;

use luck_ast::{GotoStatement, LabelStatement};
use luck_token::{LuaVersion, Span};

use crate::ParseError;

struct PendingGoto<'ast> {
    name: &'ast str,
    span: Span,
    /// Active variables at the goto, lowered to the enclosing block's count
    /// each time the goto leaves a block.
    active_vars: usize,
}

#[derive(Clone, Copy)]
pub(super) struct BlockStart {
    active_vars: usize,
    enclosing_labels_start: usize,
    enclosing_gotos_start: usize,
}

impl BlockStart {
    pub(super) const fn active_vars(self) -> usize {
        self.active_vars
    }
}

/// One function body's labels and gotos, as `dyd->label` and `dyd->gt` hold
/// them between `firstlabel` and `firstgoto`.
#[derive(Default)]
pub(super) struct FunctionGotos<'ast> {
    /// Labels of the enclosing blocks and of the current block so far.
    labels: Vec<&'ast str>,
    /// Gotos no label has taken yet, in block order.
    pending_gotos: Vec<PendingGoto<'ast>>,
    block_labels_start: usize,
    block_gotos_start: usize,
}

pub(super) struct GotoTracker<'ast> {
    version: LuaVersion,
    function: FunctionGotos<'ast>,
    errors: Vec<ParseError>,
}

impl<'ast> GotoTracker<'ast> {
    pub(super) fn new(version: LuaVersion) -> Self {
        Self { version, function: FunctionGotos::default(), errors: Vec::new() }
    }

    pub(super) const fn open_block(&mut self, active_vars: usize) -> BlockStart {
        let function = &mut self.function;
        let start = BlockStart {
            active_vars,
            enclosing_labels_start: function.block_labels_start,
            enclosing_gotos_start: function.block_gotos_start,
        };
        function.block_labels_start = function.labels.len();
        function.block_gotos_start = function.pending_gotos.len();
        start
    }

    /// Close the current block. A goto still pending leaves it: a label of the
    /// enclosing block already seen takes it as a backward jump, and otherwise
    /// it waits there with the enclosing block's variable count.
    pub(super) fn close_block(&mut self, start: BlockStart) {
        let function = &mut self.function;
        function.labels.truncate(function.block_labels_start);
        let leaving = function.pending_gotos.split_off(function.block_gotos_start);
        function.block_labels_start = start.enclosing_labels_start;
        function.block_gotos_start = start.enclosing_gotos_start;
        for mut goto in leaving {
            let is_backward_jump = function
                .labels
                .iter()
                .skip(function.block_labels_start)
                .any(|name| *name == goto.name);
            if !is_backward_jump {
                goto.active_vars = start.active_vars;
                function.pending_gotos.push(goto);
            }
        }
    }

    /// Record a label whose scope starts at `active_vars`, solving the gotos
    /// pending in this block with its name.
    pub(super) fn add_label(&mut self, label: &'ast LabelStatement, active_vars: usize) {
        let Some(name) = label.name.identifier() else {
            return;
        };
        let function = &mut self.function;
        let first_visible =
            if self.version.has_label_shadowing() { function.block_labels_start } else { 0 };
        if function.labels.iter().skip(first_visible).any(|existing| *existing == name) {
            self.errors.push(ParseError {
                span: label.span,
                message: format!("label `{name}` already defined"),
            });
        }
        for goto in function.pending_gotos.split_off(function.block_gotos_start) {
            if goto.name != name {
                function.pending_gotos.push(goto);
                continue;
            }
            if goto.active_vars < active_vars {
                self.errors.push(ParseError {
                    span: goto.span,
                    message: format!("`goto {name}` jumps into the scope of a local"),
                });
            }
        }
        function.labels.push(name);
    }

    /// Record a goto at `active_vars` active variables, a backward jump if
    /// this block already has its label.
    pub(super) fn add_goto(&mut self, goto: &'ast GotoStatement, active_vars: usize) {
        let Some(name) = goto.name.identifier() else {
            return;
        };
        let function = &mut self.function;
        if !function.labels.iter().skip(function.block_labels_start).any(|label| *label == name) {
            function.pending_gotos.push(PendingGoto { name, span: goto.span, active_vars });
        }
    }

    pub(super) fn enter_function(&mut self) -> FunctionGotos<'ast> {
        mem::take(&mut self.function)
    }

    pub(super) fn into_errors(self) -> Vec<ParseError> {
        self.errors
    }

    /// Report the gotos no label in this function took, then return to the
    /// enclosing function's state.
    pub(super) fn leave_function(&mut self, enclosing: FunctionGotos<'ast>) {
        for goto in mem::replace(&mut self.function, enclosing).pending_gotos {
            self.errors.push(ParseError {
                span: goto.span,
                message: format!("no visible label `{}` for `goto`", goto.name),
            });
        }
    }
}
