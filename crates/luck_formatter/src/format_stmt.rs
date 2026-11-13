//! Block statements mostly share the shape `keyword`,
//! `indent((hard_line, block))`, `hard_line`, `end`. Comment and verbatim
//! handling around each statement lives in `format_block`.

use core::slice;

use luck_ast::{
    Assignment, AttributedName, Block, CompoundAssignment, DoBlock, Expression, FunctionAttribute,
    FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction, GlobalStar,
    GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment, LocalFunction,
    NumericFor, Punctuated, RepeatLoop, Statement, TypeDeclaration, TypeDeclarationValue,
    WhileLoop, unwrap_parentheses,
};

use crate::CollapseSimpleStatement;
use crate::comments::write_inline_comments_after;
use crate::format_block::{IndentedBlock, first_item_span};
use crate::format_expr::LevelChain;
use crate::ir::{
    Format, Formatter, format_with, group, hard_line, indent, line_suffix_boundary,
    next_line_group, separated, soft_line_or_space, space, token,
};

/// A statement condition with redundant parens stripped. Conditions are
/// boolean contexts, so `(f())` truncating multiple values to one changes
/// nothing observable.
struct Condition<'expr>(&'expr Expression);

impl Format for Condition<'_> {
    fn fmt(&self, f: &mut Formatter) {
        unwrap_parentheses(self.0).fmt(f);
    }
}

/// The condition between `if`, `elseif`, or `while` and its `then` or `do`,
/// which breaks onto an indented line of its own, so an operator chain in it
/// breaks level with its first operand instead of indenting again.
struct HeaderCondition<'expr>(&'expr Expression);

impl Format for HeaderCondition<'_> {
    fn fmt(&self, f: &mut Formatter) {
        if let Expression::BinaryOp(chain) = unwrap_parentheses(self.0) {
            LevelChain(chain).fmt(f);
        } else {
            Condition(self.0).fmt(f);
        }
        header_comments(self.0).fmt(f);
    }
}

/// Two or more share one indented group so they wrap together. A lone item
/// breaks on its own.
struct ExprList<'list>(&'list Punctuated<Expression>);

impl Format for ExprList<'_> {
    fn fmt(&self, f: &mut Formatter) {
        let list = punctuated(self.0, (token(","), soft_line_or_space()));
        if self.0.len() > 1 {
            group(indent(list)).fmt(f);
        } else {
            list.fmt(f);
        }
    }
}

/// What follows `=`.
///
/// A lone table, function, or call breaks inside its own brackets, so it stays
/// on the `=` line. Anything else that doesn't fit there goes on its own
/// indented line when it fits that, and hangs from `=` otherwise.
fn assigned<Values>(values: Values, items: &[Expression]) -> impl Format
where
    Values: Format,
{
    let is_bracketed = matches!(
        items,
        [Expression::TableConstructor(_)
            | Expression::FunctionDef(_)
            | Expression::FunctionCall(_)]
    );
    format_with(move |f| {
        if is_bracketed {
            values.fmt(f);
        } else {
            next_line_group(&values).fmt(f);
        }
    })
}

/// No other emitter owns that gap, and a comment left unprinted there would
/// send the whole statement, body included, to the verbatim fallback.
fn header_comments(after: &Expression) -> impl Format {
    let position = after.span().end;
    format_with(move |f| write_inline_comments_after(f, position))
}

/// The items of `list` with `separator` between them, then the source's
/// trailing separator, written as `,`.
fn punctuated<T, Separator>(list: &Punctuated<T>, separator: Separator) -> impl Format
where
    T: Format,
    Separator: Format,
{
    (separated(&list.items, separator), list.has_trailing_separator.then_some(token(",")))
}

fn indented_body_then_end(block: &Block) -> impl Format {
    (IndentedBlock(block), hard_line(), line_suffix_boundary(), token("end"))
}

impl Format for Statement {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::LocalAssignment(local) => local.fmt(f),
            Self::Assignment(assignment) => assignment.fmt(f),
            Self::FunctionCall(call) => call.fmt(f),
            Self::FunctionDecl(function_decl) => function_decl.fmt(f),
            Self::LocalFunction(local_function) => local_function.fmt(f),
            // Lua 5.5
            Self::GlobalFunction(global_function) => global_function.fmt(f),
            Self::DoBlock(do_block) => do_block.fmt(f),
            Self::WhileLoop(while_loop) => while_loop.fmt(f),
            Self::RepeatLoop(repeat_loop) => repeat_loop.fmt(f),
            Self::IfStatement(if_stmt) => if_stmt.fmt(f),
            Self::NumericFor(numeric_for) => numeric_for.fmt(f),
            Self::GenericFor(generic_for) => generic_for.fmt(f),
            // Lua 5.2+
            Self::Goto(goto) => goto.fmt(f),
            // Lua 5.2+
            Self::Label(label) => label.fmt(f),
            // Lua 5.5
            Self::GlobalDeclaration(global) => global.fmt(f),
            // Lua 5.5
            Self::GlobalStar(global_star) => global_star.fmt(f),
            // Luau
            Self::CompoundAssignment(compound) => compound.fmt(f),
            // Luau
            Self::TypeDeclaration(declaration) => declaration.fmt(f),
            // Lua 5.2+: `break` as a mid-block statement.
            Self::Break(_) => token("break").fmt(f),
            // The block loop drops `;`, and a parse-recovery placeholder has
            // no text to print.
            Self::EmptyStatement(_) | Self::Error(_) => {}
        }
    }
}

impl Format for LastStatement {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Return(return_stmt) => {
                if return_stmt.exprs.is_empty() {
                    token("return").fmt(f);
                } else {
                    group((token("return"), space(), ExprList(&return_stmt.exprs))).fmt(f);
                }
            }
            Self::Break(_) => token("break").fmt(f),
            // Luau: `continue`.
            Self::Continue(_) => token("continue").fmt(f),
            // Parse-recovery placeholder: nothing to print.
            Self::Error(_) => {}
        }
    }
}

impl Format for LocalAssignment {
    fn fmt(&self, f: &mut Formatter) {
        if self.is_exported {
            (token("export"), space()).fmt(f);
        }
        let keyword = if self.is_const { "const" } else { "local" };
        (
            token(keyword),
            space(),
            // Names get their own group so a long binding list breaks
            // independently of the value list.
            group(punctuated(&self.names, (token(","), soft_line_or_space()))),
        )
            .fmt(f);

        if let Some(exprs) = &self.exprs {
            (space(), token("="), space(), assigned(ExprList(exprs), &exprs.items)).fmt(f);
        }
    }
}

impl Format for AttributedName {
    fn fmt(&self, f: &mut Formatter) {
        self.name.fmt(f);
        if let Some(annotation) = &self.type_annotation {
            (token(":"), space(), annotation).fmt(f);
        }
        if let Some(attribute) = &self.attribute {
            (space(), token("<"), &attribute.name, token(">")).fmt(f);
        }
    }
}

impl Format for Assignment {
    fn fmt(&self, f: &mut Formatter) {
        // Targets are never broken onto multiple lines.
        group((
            punctuated(&self.targets, (token(","), space())),
            space(),
            token("="),
            space(),
            assigned(ExprList(&self.values), &self.values.items),
        ))
        .fmt(f);
    }
}

impl Format for CompoundAssignment {
    fn fmt(&self, f: &mut Formatter) {
        let value = assigned(&self.expr, slice::from_ref(&self.expr));
        group((&self.var, space(), token(self.op.static_text()), space(), value)).fmt(f);
    }
}

impl Format for DoBlock {
    fn fmt(&self, f: &mut Formatter) {
        (token("do"), indented_body_then_end(&self.block)).fmt(f);
    }
}

impl Format for WhileLoop {
    fn fmt(&self, f: &mut Formatter) {
        (
            group((
                token("while"),
                indent((soft_line_or_space(), HeaderCondition(&self.condition))),
                soft_line_or_space(),
                token("do"),
            )),
            indented_body_then_end(&self.block),
        )
            .fmt(f);
    }
}

impl Format for RepeatLoop {
    fn fmt(&self, f: &mut Formatter) {
        // `until <cond>` closes the loop, so no `end` and no line-suffix flush.
        (
            token("repeat"),
            IndentedBlock(&self.block),
            hard_line(),
            token("until"),
            space(),
            Condition(&self.condition),
        )
            .fmt(f);
    }
}

impl Format for IfStatement {
    fn fmt(&self, f: &mut Formatter) {
        // Collapses to `if cond then stmt end` when the option allows, there's
        // one branch holding one statement that no comment around it would
        // stop from sharing the line, and the whole `if` fits on one line.
        let can_collapse = f.options().collapse_simple_statement
            == CollapseSimpleStatement::ConditionalOnly
            && self.elseif_clauses.is_empty()
            && self.else_clause.is_none()
            && is_simple_block(&self.block)
            && first_item_span(&self.block).is_some_and(|body_stmt| {
                !f.comments().blocks_if_collapse(
                    self.condition.span().end,
                    body_stmt,
                    self.span.end,
                )
            });

        // `if cond then` is a group so a long condition can break onto its
        // own indented line before `then`.
        let header = group((
            token("if"),
            indent((soft_line_or_space(), HeaderCondition(&self.condition))),
            soft_line_or_space(),
            token("then"),
        ));

        if can_collapse {
            // The group stays flat only if the whole `if` fits, and a body
            // spanning lines forces it to break.
            group((
                &header,
                indent((soft_line_or_space(), &self.block)),
                soft_line_or_space(),
                line_suffix_boundary(),
                token("end"),
            ))
            .fmt(f);
            return;
        }

        (&header, IndentedBlock(&self.block)).fmt(f);

        for clause in &self.elseif_clauses {
            (
                hard_line(),
                group((
                    token("elseif"),
                    indent((soft_line_or_space(), HeaderCondition(&clause.condition))),
                    soft_line_or_space(),
                    token("then"),
                )),
                IndentedBlock(&clause.block),
            )
                .fmt(f);
        }

        if let Some(else_clause) = &self.else_clause {
            (hard_line(), token("else"), IndentedBlock(&else_clause.block)).fmt(f);
        }

        (hard_line(), line_suffix_boundary(), token("end")).fmt(f);
    }
}

/// Whether a block is simple enough to collapse onto one line: a single
/// statement with no terminator, or a lone terminator (return/break/continue).
fn is_simple_block(block: &Block) -> bool {
    let stmt_count =
        block.stmts.iter().filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_))).count();
    matches!((stmt_count, &block.last_stmt), (0, Some(_)) | (1, None))
}

impl Format for NumericFor {
    fn fmt(&self, f: &mut Formatter) {
        (token("for"), space(), &self.name).fmt(f);
        // Luau: `: T` on the loop variable.
        if let Some(annotation) = &self.type_annotation {
            (token(":"), space(), annotation).fmt(f);
        }
        (space(), token("="), space(), &self.start, token(","), space(), &self.limit).fmt(f);
        if let Some(step) = &self.step {
            (token(","), space(), step).fmt(f);
        }
        (
            header_comments(self.step.as_ref().unwrap_or(&self.limit)),
            space(),
            token("do"),
            indented_body_then_end(&self.block),
        )
            .fmt(f);
    }
}

impl Format for GenericFor {
    fn fmt(&self, f: &mut Formatter) {
        // The bindings are never broken onto multiple lines.
        (
            token("for"),
            space(),
            punctuated(&self.names, (token(","), space())),
            space(),
            token("in"),
            space(),
        )
            .fmt(f);
        (
            ExprList(&self.exprs),
            self.exprs.items.last().map(header_comments),
            space(),
            token("do"),
            indented_body_then_end(&self.block),
        )
            .fmt(f);
    }
}

impl Format for GotoStatement {
    fn fmt(&self, f: &mut Formatter) {
        (token("goto"), space(), &self.name).fmt(f);
    }
}

impl Format for LabelStatement {
    fn fmt(&self, f: &mut Formatter) {
        (token("::"), &self.name, token("::")).fmt(f);
    }
}

impl Format for GlobalDeclaration {
    fn fmt(&self, f: &mut Formatter) {
        (token("global"), space(), punctuated(&self.names, (token(","), space()))).fmt(f);
        if let Some(exprs) = &self.exprs {
            (space(), token("="), space(), assigned(ExprList(exprs), &exprs.items)).fmt(f);
        }
    }
}

impl Format for GlobalStar {
    fn fmt(&self, f: &mut Formatter) {
        (token("global"), space()).fmt(f);
        if let Some(attribute) = &self.attribute {
            (token("<"), &attribute.name, token(">"), space()).fmt(f);
        }
        token("*").fmt(f);
    }
}

impl Format for FunctionDecl {
    fn fmt(&self, f: &mut Formatter) {
        write_function_attributes(f, &self.attributes);
        (token("function"), space(), &self.name, &self.body).fmt(f);
    }
}

impl Format for LocalFunction {
    fn fmt(&self, f: &mut Formatter) {
        write_function_attributes(f, &self.attributes);
        let keyword = if self.is_exported {
            "export"
        } else if self.is_const {
            "const"
        } else {
            "local"
        };
        (token(keyword), space(), token("function"), space(), &self.name, &self.body).fmt(f);
    }
}

impl Format for GlobalFunction {
    fn fmt(&self, f: &mut Formatter) {
        (token("global"), space(), token("function"), space(), &self.name, &self.body).fmt(f);
    }
}

fn write_function_attributes(f: &mut Formatter, attributes: &[FunctionAttribute]) {
    for attribute in attributes {
        (attribute, hard_line()).fmt(f);
    }
}

impl Format for FunctionName {
    fn fmt(&self, f: &mut Formatter) {
        separated(&self.names, token(".")).fmt(f);
        if let Some(method_name) = &self.method {
            (token(":"), method_name).fmt(f);
        }
    }
}

impl Format for TypeDeclaration {
    fn fmt(&self, f: &mut Formatter) {
        if self.is_exported {
            (token("export"), space()).fmt(f);
        }
        token("type").fmt(f);
        match &self.type_value {
            TypeDeclarationValue::Alias(annotation) => (
                space(),
                &self.name,
                self.generics.as_deref(),
                space(),
                token("="),
                space(),
                annotation,
            )
                .fmt(f),
            TypeDeclarationValue::TypeFunction(body) => {
                (space(), token("function"), space(), &self.name, &**body).fmt(f);
            }
        }
    }
}
