//! Goto/label resolution (5.2+), following the reference implementation.
//!
//! Each function body resolves its own labels. Duplicate visible labels and
//! unresolved gotos are errors, and a forward goto may not jump into the scope
//! of a local unless its label ends the block: only labels and `;` follow it,
//! and the block isn't a repeat body, whose `until` still sees the locals.

use luck_ast::{Block, FunctionBody, Statement, Visitor};
use luck_token::Span;

use crate::ParseError;

pub(super) fn check(block: &Block) -> Vec<ParseError> {
    let mut checker = GotoChecker { errors: Vec::new() };
    // Every function body's gotos, in walk order, then the main chunk's.
    checker.visit_block(block);
    resolve_gotos(block, &mut checker.errors);
    checker.errors
}

struct PendingGoto<'ast> {
    name: &'ast str,
    span: Span,
    /// Locals declared in the current block before this goto. Rebased to the
    /// enclosing block's count when the goto moves outward.
    locals_below: usize,
}

/// One block's goto bookkeeping while [`resolve_block_gotos`] walks it.
struct GotoScope<'ast, 'state> {
    /// Labels of the enclosing blocks and of this block so far.
    visible_labels: &'state mut Vec<&'ast str>,
    errors: &'state mut Vec<ParseError>,
    pending_gotos: Vec<PendingGoto<'ast>>,
    locals_count: usize,
}

impl<'ast> GotoScope<'ast, '_> {
    /// Resolve a nested block's gotos and fold the unresolved ones into this
    /// block's pending list, rebasing their local count to this block's.
    fn absorb(&mut self, nested: &'ast Block, is_repeat_block: bool) {
        for mut goto in
            resolve_block_gotos(nested, self.visible_labels, is_repeat_block, self.errors)
        {
            goto.locals_below = self.locals_count;
            self.pending_gotos.push(goto);
        }
    }
}

/// Finds every function body so each gets its own label scope. The caller
/// resolves the main chunk's gotos itself.
struct GotoChecker {
    errors: Vec<ParseError>,
}

impl<'ast> Visitor<'ast> for GotoChecker {
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        resolve_gotos(&body.block, &mut self.errors);
        self.walk_function_body(body);
    }
}

/// Resolve the gotos of a function body or the main chunk, reporting each
/// one that no label in it takes.
fn resolve_gotos(block: &Block, errors: &mut Vec<ParseError>) {
    for goto in resolve_block_gotos(block, &mut Vec::new(), false, errors) {
        errors.push(ParseError {
            span: goto.span,
            message: format!("no visible label `{}` for `goto`", goto.name),
        });
    }
}

/// Resolve one block's gotos, returning those that must resolve in an
/// enclosing block.
///
/// `visible_labels` holds the enclosing blocks' labels, for backward jumps and
/// duplicate detection, and is restored to its original length on return.
fn resolve_block_gotos<'ast>(
    block: &'ast Block,
    visible_labels: &mut Vec<&'ast str>,
    is_repeat_block: bool,
    errors: &mut Vec<ParseError>,
) -> Vec<PendingGoto<'ast>> {
    let visible_labels_base = visible_labels.len();
    let mut scope =
        GotoScope { visible_labels, errors, pending_gotos: Vec::new(), locals_count: 0 };

    // A repeat body's `until` still sees the body's locals, so its trailing
    // labels don't count as the end of the block.
    let trailing_void_start = if is_repeat_block {
        block.stmts.len()
    } else {
        block
            .stmts
            .iter()
            .rposition(|stmt| !matches!(stmt, Statement::Label(_) | Statement::EmptyStatement(_)))
            .map_or(0, |index| index.saturating_add(1))
    };

    for (stmt_index, stmt) in block.stmts.iter().enumerate() {
        match stmt {
            // Lua 5.2+
            Statement::Label(label) => {
                if let Some(name) = label.name.identifier() {
                    if scope.visible_labels.contains(&name) {
                        scope.errors.push(ParseError {
                            span: label.span,
                            message: format!("label `{name}` already defined"),
                        });
                    }
                    // Pending gotos with this name are forward jumps to this
                    // label. Each one fails if a local was declared in
                    // between, unless the label ends the block.
                    let locals_count = scope.locals_count;
                    scope.pending_gotos.retain(|goto| {
                        if goto.name != name {
                            return true;
                        }
                        if goto.locals_below < locals_count && stmt_index < trailing_void_start {
                            scope.errors.push(ParseError {
                                span: goto.span,
                                message: format!("`goto {name}` jumps into the scope of a local"),
                            });
                        }
                        false
                    });
                    scope.visible_labels.push(name);
                }
            }
            // Lua 5.2+
            Statement::Goto(goto) => {
                // A visible label makes this a backward jump, resolved now.
                if let Some(name) = goto.name.identifier()
                    && !scope.visible_labels.contains(&name)
                {
                    scope.pending_gotos.push(PendingGoto {
                        name,
                        span: goto.span,
                        locals_below: scope.locals_count,
                    });
                }
            }
            Statement::LocalAssignment(local) => {
                scope.locals_count = scope.locals_count.saturating_add(local.names.len());
            }
            Statement::LocalFunction(_) => {
                scope.locals_count = scope.locals_count.saturating_add(1);
            }
            Statement::DoBlock(do_block) => scope.absorb(&do_block.block, false),
            Statement::WhileLoop(while_loop) => scope.absorb(&while_loop.block, false),
            Statement::RepeatLoop(repeat_loop) => scope.absorb(&repeat_loop.block, true),
            Statement::IfStatement(if_stmt) => {
                scope.absorb(&if_stmt.block, false);
                for clause in &if_stmt.elseif_clauses {
                    scope.absorb(&clause.block, false);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    scope.absorb(&else_clause.block, false);
                }
            }
            Statement::NumericFor(numeric_for) => scope.absorb(&numeric_for.block, false),
            Statement::GenericFor(generic_for) => scope.absorb(&generic_for.block, false),
            // Function bodies get their own label scope via `GotoChecker`.
            Statement::Assignment(_)
            | Statement::CompoundAssignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::TypeDeclaration(_)
            | Statement::EmptyStatement(_)
            | Statement::Break(_)
            | Statement::Error(_) => {}
        }
    }

    scope.visible_labels.truncate(visible_labels_base);
    scope.pending_gotos
}
