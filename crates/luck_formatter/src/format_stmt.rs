//! Statement emission.
//!
//! Block statements mostly share the shape `keyword`,
//! `indent((hard_line, block))`, `hard_line`, `end`. Comment and verbatim
//! handling around each statement lives in `format_block`.

use luck_ast::{
    Assignment, AttributedName, Block, CompoundAssignment, DoBlock, Expression, FunctionAttribute,
    FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction, GlobalStar,
    GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment, LocalFunction,
    NumericFor, Punctuated, RepeatLoop, Statement, TypeDeclaration, TypeDeclarationValue,
    WhileLoop,
};

use crate::CollapseSimpleStatement;
use crate::comments::write_inline_comments_after;
use crate::ir::{
    Format, Formatter, format_with, group, hard_line, indent, line_suffix_boundary, separated,
    soft_line_or_space, space, token,
};

/// A statement condition with redundant parens stripped. Conditions are
/// boolean contexts, so `(f())` truncating multiple values to one changes
/// nothing observable.
struct Condition<'expr>(&'expr Expression);

impl Format for Condition<'_> {
    fn fmt(&self, formatter: &mut Formatter) {
        let mut inner = self.0;
        while let Expression::Parenthesized(parenthesized) = inner {
            inner = &parenthesized.expr;
        }
        inner.fmt(formatter);
    }
}

/// Comma-separated expressions, such as assignment values and `return` lists.
///
/// Two or more share one indented group so they wrap together. A lone item
/// breaks on its own.
struct ExprList<'list>(&'list Punctuated<Expression>);

impl Format for ExprList<'_> {
    fn fmt(&self, formatter: &mut Formatter) {
        let list = punctuated(self.0, (token(","), soft_line_or_space()));
        if self.0.len() > 1 {
            group(indent(list)).fmt(formatter);
        } else {
            list.fmt(formatter);
        }
    }
}

/// Comments between a header's last expression and its closing keyword, as in
/// `if cond --[[ why ]] then`.
///
/// No other emitter owns that gap, and a comment left unprinted there would
/// send the whole statement, body included, to the verbatim fallback.
fn header_comments(after: &Expression) -> impl Format {
    let position = after.span().end;
    format_with(move |formatter| write_inline_comments_after(formatter, position))
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
    (indent((hard_line(), block)), hard_line(), line_suffix_boundary(), token("end"))
}

impl Format for Statement {
    fn fmt(&self, formatter: &mut Formatter) {
        match self {
            Self::LocalAssignment(local) => local.fmt(formatter),
            Self::Assignment(assignment) => assignment.fmt(formatter),
            Self::FunctionCall(call) => call.fmt(formatter),
            Self::FunctionDecl(function_decl) => function_decl.fmt(formatter),
            Self::LocalFunction(local_function) => local_function.fmt(formatter),
            // Lua 5.5
            Self::GlobalFunction(global_function) => global_function.fmt(formatter),
            Self::DoBlock(do_block) => do_block.fmt(formatter),
            Self::WhileLoop(while_loop) => while_loop.fmt(formatter),
            Self::RepeatLoop(repeat_loop) => repeat_loop.fmt(formatter),
            Self::IfStatement(if_stmt) => if_stmt.fmt(formatter),
            Self::NumericFor(numeric_for) => numeric_for.fmt(formatter),
            Self::GenericFor(generic_for) => generic_for.fmt(formatter),
            // Lua 5.2+
            Self::Goto(goto) => goto.fmt(formatter),
            // Lua 5.2+
            Self::Label(label) => label.fmt(formatter),
            // Lua 5.5
            Self::GlobalDeclaration(global) => global.fmt(formatter),
            // Lua 5.5
            Self::GlobalStar(global_star) => global_star.fmt(formatter),
            // Luau
            Self::CompoundAssignment(compound) => compound.fmt(formatter),
            // Luau
            Self::TypeDeclaration(declaration) => declaration.fmt(formatter),
            // Lua 5.2+: `break` as a mid-block statement.
            Self::Break(_) => token("break").fmt(formatter),
            // The block loop drops `;`, and a parse-recovery placeholder has
            // no text to print.
            Self::EmptyStatement(_) | Self::Error(_) => {}
        }
    }
}

impl Format for LastStatement {
    fn fmt(&self, formatter: &mut Formatter) {
        match self {
            Self::Return(return_stmt) => {
                if return_stmt.exprs.is_empty() {
                    token("return").fmt(formatter);
                } else {
                    group((token("return"), space(), ExprList(&return_stmt.exprs))).fmt(formatter);
                }
            }
            Self::Break(_) => token("break").fmt(formatter),
            // Luau: `continue`.
            Self::Continue(_) => token("continue").fmt(formatter),
            // Parse-recovery placeholder: nothing to print.
            Self::Error(_) => {}
        }
    }
}

impl Format for LocalAssignment {
    fn fmt(&self, formatter: &mut Formatter) {
        if self.is_exported {
            (token("export"), space()).fmt(formatter);
        }
        let keyword = if self.is_const { "const" } else { "local" };
        (
            token(keyword),
            space(),
            // Names get their own group so a long binding list breaks
            // independently of the value list.
            group(punctuated(&self.names, (token(","), soft_line_or_space()))),
        )
            .fmt(formatter);

        if let Some(exprs) = &self.exprs {
            (space(), token("="), space(), ExprList(exprs)).fmt(formatter);
        }
    }
}

/// A declared name with its optional Luau `: T` annotation and its optional
/// Lua 5.4+ `<const>`/`<close>` attribute.
impl Format for AttributedName {
    fn fmt(&self, formatter: &mut Formatter) {
        self.name.fmt(formatter);
        if let Some(annotation) = &self.type_annotation {
            (token(":"), space(), annotation).fmt(formatter);
        }
        if let Some(attribute) = &self.attribute {
            (space(), token("<"), &attribute.name, token(">")).fmt(formatter);
        }
    }
}

impl Format for Assignment {
    fn fmt(&self, formatter: &mut Formatter) {
        // Targets are never broken onto multiple lines.
        group((
            punctuated(&self.targets, (token(","), space())),
            space(),
            token("="),
            space(),
            ExprList(&self.values),
        ))
        .fmt(formatter);
    }
}

/// Luau compound assignment (`x += 1`).
impl Format for CompoundAssignment {
    fn fmt(&self, formatter: &mut Formatter) {
        group((&self.var, space(), token(self.op.static_text()), space(), &self.expr))
            .fmt(formatter);
    }
}

impl Format for DoBlock {
    fn fmt(&self, formatter: &mut Formatter) {
        (token("do"), indented_body_then_end(&self.block)).fmt(formatter);
    }
}

impl Format for WhileLoop {
    fn fmt(&self, formatter: &mut Formatter) {
        (
            group((
                token("while"),
                indent((
                    soft_line_or_space(),
                    Condition(&self.condition),
                    header_comments(&self.condition),
                )),
                soft_line_or_space(),
                token("do"),
            )),
            indented_body_then_end(&self.block),
        )
            .fmt(formatter);
    }
}

impl Format for RepeatLoop {
    fn fmt(&self, formatter: &mut Formatter) {
        // `until <cond>` closes the loop, so no `end` and no line-suffix flush.
        (
            token("repeat"),
            indent((hard_line(), &self.block)),
            hard_line(),
            token("until"),
            space(),
            Condition(&self.condition),
        )
            .fmt(formatter);
    }
}

impl Format for IfStatement {
    fn fmt(&self, formatter: &mut Formatter) {
        // Collapses to `if cond then stmt end` when the option allows and
        // there's one branch holding one statement.
        let can_collapse = matches!(
            formatter.options().collapse_simple_statement,
            CollapseSimpleStatement::ConditionalOnly | CollapseSimpleStatement::Always
        ) && self.elseif_clauses.is_empty()
            && self.else_clause.is_none()
            && is_simple_block(&self.block);

        if can_collapse {
            group((
                token("if"),
                space(),
                Condition(&self.condition),
                header_comments(&self.condition),
                space(),
                token("then"),
                space(),
                &self.block,
                space(),
                token("end"),
            ))
            .fmt(formatter);
            return;
        }

        // `if cond then` is a group so a long condition can break onto its
        // own indented line before `then`.
        (
            group((
                token("if"),
                indent((
                    soft_line_or_space(),
                    Condition(&self.condition),
                    header_comments(&self.condition),
                )),
                soft_line_or_space(),
                token("then"),
            )),
            indent((hard_line(), &self.block)),
        )
            .fmt(formatter);

        for clause in &self.elseif_clauses {
            (
                hard_line(),
                group((
                    token("elseif"),
                    indent((
                        soft_line_or_space(),
                        Condition(&clause.condition),
                        header_comments(&clause.condition),
                    )),
                    soft_line_or_space(),
                    token("then"),
                )),
                indent((hard_line(), &clause.block)),
            )
                .fmt(formatter);
        }

        if let Some(else_clause) = &self.else_clause {
            (hard_line(), token("else"), indent((hard_line(), &else_clause.block))).fmt(formatter);
        }

        (hard_line(), line_suffix_boundary(), token("end")).fmt(formatter);
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
    fn fmt(&self, formatter: &mut Formatter) {
        (token("for"), space(), &self.name).fmt(formatter);
        // Luau: `: T` on the loop variable.
        if let Some(annotation) = &self.type_annotation {
            (token(":"), space(), annotation).fmt(formatter);
        }
        (space(), token("="), space(), &self.start, token(","), space(), &self.limit)
            .fmt(formatter);
        if let Some(step) = &self.step {
            (token(","), space(), step).fmt(formatter);
        }
        (
            header_comments(self.step.as_ref().unwrap_or(&self.limit)),
            space(),
            token("do"),
            indented_body_then_end(&self.block),
        )
            .fmt(formatter);
    }
}

impl Format for GenericFor {
    fn fmt(&self, formatter: &mut Formatter) {
        // The bindings are never broken onto multiple lines.
        (
            token("for"),
            space(),
            punctuated(&self.names, (token(","), space())),
            space(),
            token("in"),
            space(),
        )
            .fmt(formatter);
        (
            ExprList(&self.exprs),
            self.exprs.items.last().map(header_comments),
            space(),
            token("do"),
            indented_body_then_end(&self.block),
        )
            .fmt(formatter);
    }
}

/// Lua 5.2+ `goto name`.
impl Format for GotoStatement {
    fn fmt(&self, formatter: &mut Formatter) {
        (token("goto"), space(), &self.name).fmt(formatter);
    }
}

/// Lua 5.2+ `::name::`.
impl Format for LabelStatement {
    fn fmt(&self, formatter: &mut Formatter) {
        (token("::"), &self.name, token("::")).fmt(formatter);
    }
}

/// Lua 5.5 `global name, ... [= exprs]`.
impl Format for GlobalDeclaration {
    fn fmt(&self, formatter: &mut Formatter) {
        (token("global"), space(), punctuated(&self.names, (token(","), space()))).fmt(formatter);
        if let Some(exprs) = &self.exprs {
            (space(), token("="), space(), ExprList(exprs)).fmt(formatter);
        }
    }
}

/// Lua 5.5 `global *`, optionally with an attribute before the `*`.
impl Format for GlobalStar {
    fn fmt(&self, formatter: &mut Formatter) {
        (token("global"), space()).fmt(formatter);
        if let Some(attribute) = &self.attribute {
            (token("<"), &attribute.name, token(">"), space()).fmt(formatter);
        }
        token("*").fmt(formatter);
    }
}

impl Format for FunctionDecl {
    fn fmt(&self, formatter: &mut Formatter) {
        write_function_attributes(formatter, &self.attributes);
        (token("function"), space(), &self.name, &self.body).fmt(formatter);
    }
}

impl Format for LocalFunction {
    fn fmt(&self, formatter: &mut Formatter) {
        write_function_attributes(formatter, &self.attributes);
        let keyword = if self.is_exported {
            "export"
        } else if self.is_const {
            "const"
        } else {
            "local"
        };
        (token(keyword), space(), token("function"), space(), &self.name, &self.body)
            .fmt(formatter);
    }
}

/// Lua 5.5 `global function name(...) ... end`.
impl Format for GlobalFunction {
    fn fmt(&self, formatter: &mut Formatter) {
        (token("global"), space(), token("function"), space(), &self.name, &self.body)
            .fmt(formatter);
    }
}

/// Luau attributes such as `@native`, each on its own line above the
/// declaration.
fn write_function_attributes(formatter: &mut Formatter, attributes: &[FunctionAttribute]) {
    for attribute in attributes {
        (attribute, hard_line()).fmt(formatter);
    }
}

/// Dotted name path with an optional `:method` suffix.
impl Format for FunctionName {
    fn fmt(&self, formatter: &mut Formatter) {
        separated(&self.names, token(".")).fmt(formatter);
        if let Some(method_name) = &self.method {
            (token(":"), method_name).fmt(formatter);
        }
    }
}

/// Luau type declaration: `type Name = T`, or `type function Name funcbody`.
impl Format for TypeDeclaration {
    fn fmt(&self, formatter: &mut Formatter) {
        if self.is_exported {
            (token("export"), space()).fmt(formatter);
        }
        token("type").fmt(formatter);
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
                .fmt(formatter),
            TypeDeclarationValue::TypeFunction(body) => {
                (space(), token("function"), space(), &self.name, &**body).fmt(formatter);
            }
        }
    }
}
