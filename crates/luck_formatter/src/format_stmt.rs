//! Statement emission: `impl Format for Statement` plus a writer per
//! statement kind. Block-bearing statements share the shape
//! `keyword`, `indent((hard_line, block))`, `hard_line`, `end`; the
//! comment/verbatim protocol around each statement lives in `format_block`.

use luck_ast::expr::{Expression, Var};
use luck_ast::shared::{Parameter, Punctuated};
use luck_ast::stmt::*;

use crate::CollapseSimpleStatement;
use crate::format_function::FormatFunctionBody;
use crate::ir::*;
use crate::tokens::FormatToken;

/// A statement condition with redundant parens stripped. Conditions are
/// boolean contexts, so `(f())` truncating multiple values to one changes
/// nothing observable.
struct Condition<'a>(&'a Expression);

impl Format for Condition<'_> {
    fn fmt(&self, f: &mut Formatter) {
        let mut inner = self.0;
        while let Expression::Parenthesized(paren) = inner {
            inner = &paren.expr;
        }
        inner.fmt(f);
    }
}

impl Format for Statement {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Statement::LocalAssignment(local) => write_local_assignment(f, local),
            Statement::Assignment(assign) => write_assignment(f, assign),
            // A call statement's callee/args are ordinary expressions.
            Statement::FunctionCall(call) => call.call.fmt(f),
            Statement::FunctionDecl(decl) => write_function_decl(f, decl),
            Statement::LocalFunction(func) => write_local_function(f, func),
            Statement::GlobalFunction(func) => write_global_function(f, func), // Lua 5.5
            Statement::DoBlock(do_block) => write_do_block(f, do_block),
            Statement::WhileLoop(while_loop) => write_while_loop(f, while_loop),
            Statement::RepeatLoop(repeat_loop) => write_repeat_loop(f, repeat_loop),
            Statement::IfStatement(if_stmt) => write_if_statement(f, if_stmt),
            Statement::NumericFor(num_for) => write_numeric_for(f, num_for),
            Statement::GenericFor(gen_for) => write_generic_for(f, gen_for),
            Statement::Goto(goto) => write_goto(f, goto), // Lua 5.2+
            Statement::Label(label) => write_label(f, label), // Lua 5.2+
            Statement::GlobalDeclaration(global) => write_global_declaration(f, global), // Lua 5.5
            Statement::GlobalStar(global_star) => write_global_star(f, global_star), // Lua 5.5
            Statement::CompoundAssignment(compound) => write_compound_assignment(f, compound), // Luau
            Statement::TypeDeclaration(type_decl) => write_type_declaration(f, type_decl), // Luau
            // Lua 5.2+: `break` as a mid-block statement.
            Statement::Break(_) => {
                crate::write!(f, [token("break")]);
            }
            // A bare `;` carries no layout; the block loop drops it.
            Statement::EmptyStatement(_) => {}
            // Parse-recovery placeholder: nothing to print.
            Statement::Error(_) => {}
        }
    }
}

fn write_local_assignment(f: &mut Formatter, local: &LocalAssignment) {
    crate::write!(
        f,
        [
            token(if local.is_const { "const" } else { "local" }),
            space()
        ]
    );

    // Names get their own group so a long binding list breaks independently
    // of the value list.
    crate::write!(
        f,
        [group(format_with(|f| {
            for (idx, attributed) in local.names.items.iter().enumerate() {
                if idx > 0 {
                    crate::write!(f, [soft_line_or_space()]);
                }
                write_attributed_name(f, attributed);
                if idx + 1 < local.names.items.len() || local.names.has_trailing_separator {
                    crate::write!(f, [token(",")]);
                }
            }
        }))]
    );

    if let Some(exprs) = &local.exprs {
        crate::write!(f, [space(), token("="), space()]);
        write_punctuated_exprs(f, exprs);
    }
}

/// A declared name with its optional Luau `: T` annotation and its optional
/// Lua 5.4 `<const>`/`<close>` attribute.
fn write_attributed_name(f: &mut Formatter, attributed: &AttributedName) {
    crate::write!(f, [FormatToken(&attributed.name)]);
    if let Some(annotation) = &attributed.type_annotation {
        crate::write!(f, [token(":"), space(), annotation]);
    }
    if let Some(attrib) = &attributed.attrib {
        crate::write!(
            f,
            [space(), token("<"), FormatToken(&attrib.name), token(">"),]
        );
    }
}

fn write_assignment(f: &mut Formatter, assign: &Assignment) {
    crate::write!(
        f,
        [group(format_with(|f| {
            write_punctuated_vars(f, &assign.targets);
            crate::write!(f, [space(), token("="), space()]);
            write_punctuated_exprs(f, &assign.values);
        }))]
    );
}

/// Luau compound assignment (`x += 1`).
fn write_compound_assignment(f: &mut Formatter, compound: &CompoundAssignment) {
    crate::write!(
        f,
        [group(format_with(|f| {
            compound.var.fmt(f);
            crate::write!(f, [space(), token(compound.op.static_text()), space()]);
            compound.expr.fmt(f);
        }))]
    );
}

fn write_do_block(f: &mut Formatter, do_block: &DoBlock) {
    crate::write!(
        f,
        [
            token("do"),
            indent((hard_line(), &do_block.block)),
            hard_line(),
            line_suffix_boundary(),
            token("end"),
        ]
    );
}

fn write_while_loop(f: &mut Formatter, while_loop: &WhileLoop) {
    crate::write!(
        f,
        [group((
            token("while"),
            indent((soft_line_or_space(), Condition(&while_loop.condition))),
            soft_line_or_space(),
            token("do"),
        ))]
    );
    crate::write!(
        f,
        [
            indent((hard_line(), &while_loop.block)),
            hard_line(),
            line_suffix_boundary(),
            token("end"),
        ]
    );
}

fn write_repeat_loop(f: &mut Formatter, repeat_loop: &RepeatLoop) {
    // `until <cond>` closes the loop, so no `end` and no line-suffix flush.
    crate::write!(
        f,
        [
            token("repeat"),
            indent((hard_line(), &repeat_loop.block)),
            hard_line(),
            token("until"),
            space(),
            Condition(&repeat_loop.condition),
        ]
    );
}

fn write_if_statement(f: &mut Formatter, if_stmt: &IfStatement) {
    // `if cond then stmt end` collapses to one line when the option allows and
    // there is a single non-branching body.
    let can_collapse = matches!(
        f.options.collapse_simple_statement,
        CollapseSimpleStatement::ConditionalOnly | CollapseSimpleStatement::Always
    ) && if_stmt.elseif_clauses.is_empty()
        && if_stmt.else_clause.is_none()
        && crate::format_block::is_simple_block(&if_stmt.block);

    if can_collapse {
        crate::write!(
            f,
            [group((
                token("if"),
                space(),
                Condition(&if_stmt.condition),
                space(),
                token("then"),
                space(),
                &if_stmt.block,
                space(),
                token("end"),
            ))]
        );
        return;
    }

    // `if cond then` is a group so a long condition can break onto its own
    // indented line before `then`.
    crate::write!(
        f,
        [
            group((
                token("if"),
                indent((soft_line_or_space(), Condition(&if_stmt.condition))),
                soft_line_or_space(),
                token("then"),
            )),
            indent((hard_line(), &if_stmt.block)),
        ]
    );

    for clause in &if_stmt.elseif_clauses {
        crate::write!(
            f,
            [
                hard_line(),
                group((
                    token("elseif"),
                    indent((soft_line_or_space(), Condition(&clause.condition))),
                    soft_line_or_space(),
                    token("then"),
                )),
                indent((hard_line(), &clause.block)),
            ]
        );
    }

    if let Some(else_clause) = &if_stmt.else_clause {
        crate::write!(
            f,
            [
                hard_line(),
                token("else"),
                indent((hard_line(), &else_clause.block)),
            ]
        );
    }

    crate::write!(f, [hard_line(), line_suffix_boundary(), token("end")]);
}

fn write_numeric_for(f: &mut Formatter, num_for: &NumericFor) {
    crate::write!(f, [token("for"), space(), FormatToken(&num_for.name)]);
    // Luau: `: T` on the loop variable.
    if let Some(annotation) = &num_for.type_annotation {
        crate::write!(f, [token(":"), space(), annotation]);
    }
    crate::write!(
        f,
        [
            space(),
            token("="),
            space(),
            &num_for.start,
            token(","),
            space(),
            &num_for.limit,
        ]
    );
    if let Some(step) = &num_for.step {
        crate::write!(f, [token(","), space(), step]);
    }
    crate::write!(
        f,
        [
            space(),
            token("do"),
            indent((hard_line(), &num_for.block)),
            hard_line(),
            line_suffix_boundary(),
            token("end"),
        ]
    );
}

fn write_generic_for(f: &mut Formatter, gen_for: &GenericFor) {
    crate::write!(f, [token("for"), space()]);
    write_punctuated_params(f, &gen_for.names);
    crate::write!(f, [space(), token("in"), space()]);
    write_punctuated_exprs(f, &gen_for.exprs);
    crate::write!(
        f,
        [
            space(),
            token("do"),
            indent((hard_line(), &gen_for.block)),
            hard_line(),
            line_suffix_boundary(),
            token("end"),
        ]
    );
}

/// Lua 5.2+ `goto name`.
fn write_goto(f: &mut Formatter, goto: &GotoStatement) {
    crate::write!(f, [token("goto"), space(), FormatToken(&goto.name)]);
}

/// Lua 5.2+ `::name::`.
fn write_label(f: &mut Formatter, label: &LabelStatement) {
    crate::write!(f, [token("::"), FormatToken(&label.name), token("::"),]);
}

/// Lua 5.5 `global name, ... [= exprs]`.
fn write_global_declaration(f: &mut Formatter, global: &GlobalDeclaration) {
    crate::write!(f, [token("global"), space()]);
    for (idx, attributed) in global.names.items.iter().enumerate() {
        if idx > 0 {
            crate::write!(f, [space()]);
        }
        write_attributed_name(f, attributed);
        if idx + 1 < global.names.items.len() || global.names.has_trailing_separator {
            crate::write!(f, [token(",")]);
        }
    }
    if let Some(exprs) = &global.exprs {
        crate::write!(f, [space(), token("="), space()]);
        write_punctuated_exprs(f, exprs);
    }
}

/// Lua 5.5 `global *` - rebuilt from tokens (never sliced) so synthetic ASTs
/// format too.
fn write_global_star(f: &mut Formatter, global_star: &GlobalStar) {
    crate::write!(f, [token("global"), space()]);
    if let Some(attrib) = &global_star.attrib {
        crate::write!(
            f,
            [token("<"), FormatToken(&attrib.name), token(">"), space(),]
        );
    }
    crate::write!(f, [token("*")]);
}

fn write_function_decl(f: &mut Formatter, decl: &FunctionDecl) {
    write_function_attributes(f, &decl.attributes);
    crate::write!(f, [token("function"), space()]);
    write_func_name(f, &decl.name);
    crate::write!(f, [FormatFunctionBody { body: &decl.body }]);
}

fn write_local_function(f: &mut Formatter, func: &LocalFunction) {
    write_function_attributes(f, &func.attributes);
    crate::write!(
        f,
        [
            token(if func.is_const { "const" } else { "local" }),
            space(),
            token("function"),
            space(),
            FormatToken(&func.name),
            FormatFunctionBody { body: &func.body },
        ]
    );
}

/// Lua 5.5 `global function name(...) ... end`.
fn write_global_function(f: &mut Formatter, func: &GlobalFunction) {
    crate::write!(
        f,
        [
            token("global"),
            space(),
            token("function"),
            space(),
            FormatToken(&func.name),
            FormatFunctionBody { body: &func.body },
        ]
    );
}

/// Luau `@native`/`@checked`/etc attributes, each on its own line above the
/// declaration. Dropping them would change runtime behavior.
fn write_function_attributes(f: &mut Formatter, attributes: &[FunctionAttribute]) {
    for attr in attributes {
        match &attr.args {
            // Arguments only exist on the bracketed form.
            Some(args) => {
                crate::write!(
                    f,
                    [token("@"), token("["), FormatToken(&attr.name), token("(")]
                );
                for (idx, expr) in args.items.iter().enumerate() {
                    if idx > 0 {
                        crate::write!(f, [token(","), space()]);
                    }
                    expr.fmt(f);
                }
                crate::write!(f, [token(")"), token("]"), hard_line()]);
            }
            None => {
                crate::write!(f, [token("@"), FormatToken(&attr.name), hard_line()]);
            }
        }
    }
}

/// Dotted name path with an optional `:method` suffix.
fn write_func_name(f: &mut Formatter, name: &FuncName) {
    for (idx, name_token) in name.names.iter().enumerate() {
        if idx > 0 {
            crate::write!(f, [token(".")]);
        }
        crate::write!(f, [FormatToken(name_token)]);
    }
    if let Some(method_name) = &name.method {
        crate::write!(f, [token(":"), FormatToken(method_name)]);
    }
}

/// Luau type declaration: `type Name = T`, or `type function Name funcbody`.
fn write_type_declaration(f: &mut Formatter, decl: &TypeDeclaration) {
    if decl.is_exported {
        crate::write!(f, [token("export"), space()]);
    }
    crate::write!(f, [token("type"), space()]);

    // `type function Name funcbody`: no `=`; the body is ordinary Luau.
    if let TypeDeclarationValue::TypeFunction(body) = &decl.type_value {
        crate::write!(f, [token("function"), space(), FormatToken(&decl.name)]);
        crate::write!(f, [FormatFunctionBody { body }]);
        return;
    }

    crate::write!(f, [FormatToken(&decl.name)]);
    if let Some(generics) = &decl.generics {
        crate::write!(f, [&**generics]);
    }
    match &decl.type_value {
        TypeDeclarationValue::Alias(annotation) => {
            crate::write!(f, [space(), token("="), space(), annotation]);
        }
        // Defensive: the alias form should always carry an Alias value.
        TypeDeclarationValue::TypeFunction(body) => {
            crate::write!(f, [space(), FormatFunctionBody { body }]);
        }
    }
}

/// Assignment targets: comma-separated, never broken onto multiple lines.
fn write_punctuated_vars(f: &mut Formatter, vars: &Punctuated<Var>) {
    for (idx, var) in vars.items.iter().enumerate() {
        if idx > 0 {
            crate::write!(f, [space()]);
        }
        var.fmt(f);
        if idx + 1 < vars.items.len() || vars.has_trailing_separator {
            crate::write!(f, [token(",")]);
        }
    }
}

/// Generic-for bindings: typed names, comma-separated, never broken.
fn write_punctuated_params(f: &mut Formatter, params: &Punctuated<Parameter>) {
    for (idx, param) in params.items.iter().enumerate() {
        if idx > 0 {
            crate::write!(f, [space()]);
        }
        crate::write!(f, [FormatToken(&param.name)]);
        if let Some(annotation) = &param.type_annotation {
            crate::write!(f, [token(":"), space(), annotation]);
        }
        if idx + 1 < params.items.len() || params.has_trailing_separator {
            crate::write!(f, [token(",")]);
        }
    }
}

/// Comma-separated expressions (assignment/local values, `return`, generic-for
/// iterators). Two or more items share a group + indent so they wrap together;
/// a lone item breaks on its own.
pub(crate) fn write_punctuated_exprs(f: &mut Formatter, exprs: &Punctuated<Expression>) {
    if exprs.len() > 1 {
        crate::write!(
            f,
            [group(indent(format_with(|f| {
                for (idx, expr) in exprs.items.iter().enumerate() {
                    if idx > 0 {
                        crate::write!(f, [soft_line_or_space()]);
                    }
                    expr.fmt(f);
                    if idx + 1 < exprs.items.len() || exprs.has_trailing_separator {
                        crate::write!(f, [token(",")]);
                    }
                }
            })))]
        );
    } else {
        for expr in &exprs.items {
            expr.fmt(f);
            if exprs.has_trailing_separator {
                crate::write!(f, [token(",")]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::comments::Comments;
    use crate::ir::{Format, Formatter};
    use crate::printer::{self, PrinterOptions};

    fn format(source: &str, version: LuaVersion) -> String {
        let parsed = luck_parser::parse(source, version);
        assert!(
            parsed.errors.is_empty(),
            "parse errors: {:?}",
            parsed.errors
        );
        let comments = Comments::from_source(&parsed.comments, source);
        let mut formatter = Formatter::with_context(crate::FormatOptions::default(), comments);
        formatter.emit_shebang();
        parsed.block.fmt(&mut formatter);
        formatter.emit_remaining_comments(true);
        let group_count = formatter.group_count();
        let elements = formatter.into_elements();
        let options = PrinterOptions {
            line_width: 100,
            use_tabs: true,
            indent_width: 4,
        };
        printer::print(&elements, group_count, &options)
    }

    #[test]
    fn local_assignment_round_trips() {
        let output = format("local a, b = 1, 2", LuaVersion::Lua54);
        assert!(output.contains("local"));
        assert!(output.contains('='));
    }

    #[test]
    fn do_block_indents_body() {
        let output = format("do local x = 1 end", LuaVersion::Lua54);
        assert!(output.starts_with("do"));
        assert!(output.contains('\n'));
        assert!(output.trim_end().ends_with("end"));
    }

    #[test]
    fn numeric_for_has_step() {
        let output = format("for i = 1, 10, 2 do end", LuaVersion::Lua54);
        assert!(output.contains("for"));
        assert!(output.contains("do"));
        assert!(output.trim_end().ends_with("end"));
    }

    #[test]
    fn generic_for_binds_names() {
        let output = format("for k, v in pairs(t) do end", LuaVersion::Lua54);
        assert!(output.contains("in"));
        assert!(output.contains("pairs"));
    }

    #[test]
    fn goto_and_label() {
        let output = format("::top::\ngoto top", LuaVersion::Lua54);
        assert!(output.contains("::top::"));
        assert!(output.contains("goto top"));
    }

    #[test]
    fn if_else_chain() {
        let output = format(
            "if a then x() elseif b then y() else z() end",
            LuaVersion::Lua54,
        );
        assert!(output.contains("if"));
        assert!(output.contains("elseif"));
        assert!(output.contains("else"));
        assert!(output.trim_end().ends_with("end"));
    }
}
