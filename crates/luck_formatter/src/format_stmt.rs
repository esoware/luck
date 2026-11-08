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
use crate::comments::emit_inline_comments_after;
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

/// Comments between a header's last expression and its closing keyword, as in
/// `if cond --[[ why ]] then`.
///
/// No other emitter owns that gap, and a comment left unprinted there would
/// send the whole statement, body included, to the verbatim fallback.
fn header_comments(after: &Expression) -> impl Format {
    let position = after.span().end;
    format_with(move |formatter| emit_inline_comments_after(formatter, position))
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
            Self::LocalAssignment(local) => write_local_assignment(formatter, local),
            Self::Assignment(assignment) => write_assignment(formatter, assignment),
            Self::FunctionCall(call) => call.fmt(formatter),
            Self::FunctionDecl(function_decl) => write_function_decl(formatter, function_decl),
            Self::LocalFunction(local_function) => write_local_function(formatter, local_function),
            // Lua 5.5
            Self::GlobalFunction(global_function) => {
                write_global_function(formatter, global_function);
            }
            Self::DoBlock(do_block) => write_do_block(formatter, do_block),
            Self::WhileLoop(while_loop) => write_while_loop(formatter, while_loop),
            Self::RepeatLoop(repeat_loop) => write_repeat_loop(formatter, repeat_loop),
            Self::IfStatement(if_stmt) => write_if_statement(formatter, if_stmt),
            Self::NumericFor(numeric_for) => write_numeric_for(formatter, numeric_for),
            Self::GenericFor(generic_for) => write_generic_for(formatter, generic_for),
            Self::Goto(goto) => write_goto(formatter, goto), // Lua 5.2+
            Self::Label(label) => write_label(formatter, label), // Lua 5.2+
            Self::GlobalDeclaration(global) => write_global_declaration(formatter, global), // Lua 5.5
            Self::GlobalStar(global_star) => write_global_star(formatter, global_star), // Lua 5.5
            Self::CompoundAssignment(compound) => write_compound_assignment(formatter, compound), // Luau
            Self::TypeDeclaration(declaration) => write_type_declaration(formatter, declaration), // Luau
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
                    group(format_with(|grouped| {
                        (token("return"), space()).fmt(grouped);
                        write_punctuated_exprs(grouped, &return_stmt.exprs);
                    }))
                    .fmt(formatter);
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

fn write_local_assignment(formatter: &mut Formatter, local: &LocalAssignment) {
    if local.is_exported {
        (token("export"), space()).fmt(formatter);
    }
    let keyword = if local.is_const { "const" } else { "local" };
    (
        token(keyword),
        space(),
        // Names get their own group so a long binding list breaks
        // independently of the value list.
        group(punctuated(&local.names, (token(","), soft_line_or_space()))),
    )
        .fmt(formatter);

    if let Some(exprs) = &local.exprs {
        (space(), token("="), space()).fmt(formatter);
        write_punctuated_exprs(formatter, exprs);
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

fn write_assignment(formatter: &mut Formatter, assignment: &Assignment) {
    // Targets are never broken onto multiple lines.
    group(format_with(|grouped| {
        (punctuated(&assignment.targets, (token(","), space())), space(), token("="), space())
            .fmt(grouped);
        write_punctuated_exprs(grouped, &assignment.values);
    }))
    .fmt(formatter);
}

/// Luau compound assignment (`x += 1`).
fn write_compound_assignment(formatter: &mut Formatter, compound: &CompoundAssignment) {
    group((&compound.var, space(), token(compound.op.static_text()), space(), &compound.expr))
        .fmt(formatter);
}

fn write_do_block(formatter: &mut Formatter, do_block: &DoBlock) {
    (token("do"), indented_body_then_end(&do_block.block)).fmt(formatter);
}

fn write_while_loop(formatter: &mut Formatter, while_loop: &WhileLoop) {
    (
        group((
            token("while"),
            indent((
                soft_line_or_space(),
                Condition(&while_loop.condition),
                header_comments(&while_loop.condition),
            )),
            soft_line_or_space(),
            token("do"),
        )),
        indented_body_then_end(&while_loop.block),
    )
        .fmt(formatter);
}

fn write_repeat_loop(formatter: &mut Formatter, repeat_loop: &RepeatLoop) {
    // `until <cond>` closes the loop, so no `end` and no line-suffix flush.
    (
        token("repeat"),
        indent((hard_line(), &repeat_loop.block)),
        hard_line(),
        token("until"),
        space(),
        Condition(&repeat_loop.condition),
    )
        .fmt(formatter);
}

fn write_if_statement(formatter: &mut Formatter, if_stmt: &IfStatement) {
    // Collapses to `if cond then stmt end` when the option allows and there's
    // one branch holding one statement.
    let can_collapse = matches!(
        formatter.options().collapse_simple_statement,
        CollapseSimpleStatement::ConditionalOnly | CollapseSimpleStatement::Always
    ) && if_stmt.elseif_clauses.is_empty()
        && if_stmt.else_clause.is_none()
        && is_simple_block(&if_stmt.block);

    if can_collapse {
        group((
            token("if"),
            space(),
            Condition(&if_stmt.condition),
            header_comments(&if_stmt.condition),
            space(),
            token("then"),
            space(),
            &if_stmt.block,
            space(),
            token("end"),
        ))
        .fmt(formatter);
        return;
    }

    // `if cond then` is a group so a long condition can break onto its own
    // indented line before `then`.
    (
        group((
            token("if"),
            indent((
                soft_line_or_space(),
                Condition(&if_stmt.condition),
                header_comments(&if_stmt.condition),
            )),
            soft_line_or_space(),
            token("then"),
        )),
        indent((hard_line(), &if_stmt.block)),
    )
        .fmt(formatter);

    for clause in &if_stmt.elseif_clauses {
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

    if let Some(else_clause) = &if_stmt.else_clause {
        (hard_line(), token("else"), indent((hard_line(), &else_clause.block))).fmt(formatter);
    }

    (hard_line(), line_suffix_boundary(), token("end")).fmt(formatter);
}

/// Whether a block is simple enough to collapse onto one line: a single
/// statement with no terminator, or a lone terminator (return/break/continue).
fn is_simple_block(block: &Block) -> bool {
    let stmt_count =
        block.stmts.iter().filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_))).count();
    matches!((stmt_count, &block.last_stmt), (0, Some(_)) | (1, None))
}

fn write_numeric_for(formatter: &mut Formatter, numeric_for: &NumericFor) {
    (token("for"), space(), &numeric_for.name).fmt(formatter);
    // Luau: `: T` on the loop variable.
    if let Some(annotation) = &numeric_for.type_annotation {
        (token(":"), space(), annotation).fmt(formatter);
    }
    (space(), token("="), space(), &numeric_for.start, token(","), space(), &numeric_for.limit)
        .fmt(formatter);
    if let Some(step) = &numeric_for.step {
        (token(","), space(), step).fmt(formatter);
    }
    (
        header_comments(numeric_for.step.as_ref().unwrap_or(&numeric_for.limit)),
        space(),
        token("do"),
        indented_body_then_end(&numeric_for.block),
    )
        .fmt(formatter);
}

fn write_generic_for(formatter: &mut Formatter, generic_for: &GenericFor) {
    // The bindings are never broken onto multiple lines.
    (
        token("for"),
        space(),
        punctuated(&generic_for.names, (token(","), space())),
        space(),
        token("in"),
        space(),
    )
        .fmt(formatter);
    write_punctuated_exprs(formatter, &generic_for.exprs);
    generic_for.exprs.items.last().map(header_comments).fmt(formatter);
    (space(), token("do"), indented_body_then_end(&generic_for.block)).fmt(formatter);
}

/// Lua 5.2+ `goto name`.
fn write_goto(formatter: &mut Formatter, goto: &GotoStatement) {
    (token("goto"), space(), &goto.name).fmt(formatter);
}

/// Lua 5.2+ `::name::`.
fn write_label(formatter: &mut Formatter, label: &LabelStatement) {
    (token("::"), &label.name, token("::")).fmt(formatter);
}

/// Lua 5.5 `global name, ... [= exprs]`.
fn write_global_declaration(formatter: &mut Formatter, global: &GlobalDeclaration) {
    (token("global"), space(), punctuated(&global.names, (token(","), space()))).fmt(formatter);
    if let Some(exprs) = &global.exprs {
        (space(), token("="), space()).fmt(formatter);
        write_punctuated_exprs(formatter, exprs);
    }
}

/// Lua 5.5 `global *`, optionally with an attribute before the `*`.
fn write_global_star(formatter: &mut Formatter, global_star: &GlobalStar) {
    (token("global"), space()).fmt(formatter);
    if let Some(attribute) = &global_star.attribute {
        (token("<"), &attribute.name, token(">"), space()).fmt(formatter);
    }
    token("*").fmt(formatter);
}

fn write_function_decl(formatter: &mut Formatter, function_decl: &FunctionDecl) {
    write_function_attributes(formatter, &function_decl.attributes);
    (token("function"), space(), &function_decl.name, &function_decl.body).fmt(formatter);
}

fn write_local_function(formatter: &mut Formatter, local_function: &LocalFunction) {
    write_function_attributes(formatter, &local_function.attributes);
    let keyword = if local_function.is_exported {
        "export"
    } else if local_function.is_const {
        "const"
    } else {
        "local"
    };
    (
        token(keyword),
        space(),
        token("function"),
        space(),
        &local_function.name,
        &local_function.body,
    )
        .fmt(formatter);
}

/// Lua 5.5 `global function name(...) ... end`.
fn write_global_function(formatter: &mut Formatter, global_function: &GlobalFunction) {
    (
        token("global"),
        space(),
        token("function"),
        space(),
        &global_function.name,
        &global_function.body,
    )
        .fmt(formatter);
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
fn write_type_declaration(formatter: &mut Formatter, declaration: &TypeDeclaration) {
    if declaration.is_exported {
        (token("export"), space()).fmt(formatter);
    }
    token("type").fmt(formatter);
    match &declaration.type_value {
        TypeDeclarationValue::Alias(annotation) => (
            space(),
            &declaration.name,
            declaration.generics.as_deref(),
            space(),
            token("="),
            space(),
            annotation,
        )
            .fmt(formatter),
        TypeDeclarationValue::TypeFunction(body) => {
            (space(), token("function"), space(), &declaration.name, &**body).fmt(formatter);
        }
    }
}

/// Comma-separated expressions, such as assignment values and `return` lists.
///
/// Two or more share one indented group so they wrap together. A lone item
/// breaks on its own.
pub(crate) fn write_punctuated_exprs(formatter: &mut Formatter, exprs: &Punctuated<Expression>) {
    let list = punctuated(exprs, (token(","), soft_line_or_space()));
    if exprs.len() > 1 {
        group(indent(list)).fmt(formatter);
    } else {
        list.fmt(formatter);
    }
}
