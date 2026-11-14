//! Goto/label resolution (5.2+), following the reference implementation.
//!
//! [`super::scope`] drives this as it walks the active-variable stack. Each
//! function body resolves its own labels. Duplicate visible labels and
//! unresolved gotos are errors, and a forward goto may not jump into the scope
//! of a variable unless its label ends the block: only labels and `;` follow
//! it, and the block isn't a repeat body, whose `until` still sees the locals.

use alloc::collections::{BTreeMap, BTreeSet};
use core::mem;

use luck_ast::{GotoStatement, LabelStatement};
use luck_token::{LuaVersion, Span};
use rustc_hash::FxHashMap;

use crate::ParseError;

struct PendingGoto {
    span: Span,
    /// Active variables at the goto, lowered to the enclosing block's count
    /// each time the goto leaves a block.
    active_vars: usize,
}

#[derive(Clone, Copy)]
pub(super) struct BlockStart {
    active_vars: usize,
}

impl BlockStart {
    pub(super) const fn active_vars(self) -> usize {
        self.active_vars
    }
}

/// Labels and gotos are looked up by name, so a block's cost follows the
/// number of them and not the square of it.
#[derive(Default)]
struct BlockGotos<'ast> {
    labels: BTreeSet<&'ast str>,
    /// Gotos no label has taken yet, each name's in source order.
    pending_gotos: BTreeMap<&'ast str, Vec<PendingGoto>>,
}

/// One function body's labels and gotos, as `dyd->label` and `dyd->gt` hold
/// them between `firstlabel` and `firstgoto`.
pub(super) struct FunctionGotos<'ast> {
    /// The open blocks, innermost last, over one for the function itself,
    /// where a goto that left every block waits to be reported.
    blocks: Vec<BlockGotos<'ast>>,
    /// How many open blocks hold a label of each name.
    visible_labels: FxHashMap<&'ast str, usize>,
}

impl Default for FunctionGotos<'_> {
    fn default() -> Self {
        Self { blocks: vec![BlockGotos::default()], visible_labels: FxHashMap::default() }
    }
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

    pub(super) fn open_block(&mut self, active_vars: usize) -> BlockStart {
        self.function.blocks.push(BlockGotos::default());
        BlockStart { active_vars }
    }

    /// Close the current block. A goto still pending leaves it: a label of the
    /// enclosing block already seen takes it as a backward jump, and otherwise
    /// it waits there with the enclosing block's variable count.
    pub(super) fn close_block(&mut self, start: BlockStart) {
        let function = &mut self.function;
        if function.blocks.len() < 2 {
            return;
        }
        let Some(closed) = function.blocks.pop() else {
            return;
        };
        for label in closed.labels {
            if let Some(count) = function.visible_labels.get_mut(label) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    function.visible_labels.remove(label);
                }
            }
        }
        let Some(enclosing) = function.blocks.last_mut() else {
            return;
        };
        for (name, gotos) in closed.pending_gotos {
            if enclosing.labels.contains(name) {
                continue;
            }
            // The enclosing block's own gotos of this name came first.
            enclosing.pending_gotos.entry(name).or_default().extend(
                gotos
                    .into_iter()
                    .map(|goto| PendingGoto { span: goto.span, active_vars: start.active_vars }),
            );
        }
    }

    /// Record a label whose scope starts at `active_vars`, solving the gotos
    /// pending in this block with its name.
    pub(super) fn add_label(&mut self, label: &'ast LabelStatement, active_vars: usize) {
        let Some(name) = label.name.identifier() else {
            return;
        };
        let function = &mut self.function;
        let Some(block) = function.blocks.last_mut() else {
            return;
        };
        let is_defined = if self.version.has_label_shadowing() {
            block.labels.contains(name)
        } else {
            function.visible_labels.contains_key(name)
        };
        if is_defined {
            self.errors.push(ParseError {
                span: label.span,
                message: format!("label `{name}` already defined"),
            });
        }
        for goto in block.pending_gotos.remove(name).unwrap_or_default() {
            if goto.active_vars < active_vars {
                self.errors.push(ParseError {
                    span: goto.span,
                    message: format!("`goto {name}` jumps into the scope of a local"),
                });
            }
        }
        if block.labels.insert(name) {
            let count = function.visible_labels.entry(name).or_default();
            *count = count.saturating_add(1);
        }
    }

    /// Record a goto at `active_vars` active variables, a backward jump if
    /// this block already has its label.
    pub(super) fn add_goto(&mut self, goto: &'ast GotoStatement, active_vars: usize) {
        let Some(name) = goto.name.identifier() else {
            return;
        };
        let Some(block) = self.function.blocks.last_mut() else {
            return;
        };
        if !block.labels.contains(name) {
            block
                .pending_gotos
                .entry(name)
                .or_default()
                .push(PendingGoto { span: goto.span, active_vars });
        }
    }

    pub(super) fn enter_function(&mut self) -> FunctionGotos<'ast> {
        mem::take(&mut self.function)
    }

    pub(super) fn into_errors(self) -> Vec<ParseError> {
        self.errors
    }

    /// Report the gotos no label in this function took, in source order, then
    /// return to the enclosing function's state.
    pub(super) fn leave_function(&mut self, enclosing: FunctionGotos<'ast>) {
        let function = mem::replace(&mut self.function, enclosing);
        let mut unresolved: Vec<(Span, &str)> = function
            .blocks
            .iter()
            .flat_map(|block| &block.pending_gotos)
            .flat_map(|(name, gotos)| gotos.iter().map(|goto| (goto.span, *name)))
            .collect();
        unresolved.sort_unstable_by_key(|(span, _)| span.start);
        for (span, name) in unresolved {
            self.errors.push(ParseError {
                span,
                message: format!("no visible label `{name}` for `goto`"),
            });
        }
    }
}
