use std::collections::HashMap;

use crate::expr::ident_name_string;
use crate::tokens::default_span as sp;
use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_ast::visitor::Visitor;
use luck_token::token::TokenKind;

/// Inline single-use local variables whose initializer is a CLOSED
/// LITERAL expression, removing the declaration.
///
/// This pass works on names, not bindings, so every candidate must be
/// position-insensitive:
/// - the name must not be bound by ANY other binder in the file (another
///   local, a parameter, a loop variable, a function name) - otherwise a
///   use of the shadowing binding gets the wrong value;
/// - the value must carry no identity (no tables, no closures - moving
///   one into a loop mints a fresh object per iteration), capture nothing,
///   and not be `...` (whose meaning changes across function boundaries).
pub fn inline(mut block: Block) -> Block {
    loop {
        let candidates = find_inline_candidates(&block);
        if candidates.is_empty() {
            break;
        }
        let mut inliner = Inliner { candidates };
        block = inliner.transform_block(block);
    }
    block
}

struct InlineCandidate {
    expr: Expression,
}

/// Closed literal expression: literals and operators over literals.
/// No vars, no varargs, no calls, no tables, no closures - nothing whose
/// value or identity depends on WHERE it is evaluated.
fn is_closed_literal_expr(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::Parenthesized(paren) => is_closed_literal_expr(&paren.expr),
        Expression::UnaryOp(unop) => is_closed_literal_expr(&unop.operand),
        Expression::BinaryOp(binop) => {
            is_closed_literal_expr(&binop.left) && is_closed_literal_expr(&binop.right)
        }
        // Luau type casts are transparent wrappers.
        Expression::TypeCast(cast) => is_closed_literal_expr(&cast.expr),
        _ => false,
    }
}

fn find_inline_candidates(block: &Block) -> HashMap<String, InlineCandidate> {
    // One walk gathers everything the candidate filter needs:
    // declarations, disqualifying binders, and reference counts.
    let mut scanner = CandidateScanner {
        declarations: HashMap::new(),
        declared_names: std::collections::HashSet::new(),
        shadowed: std::collections::HashSet::new(),
        ref_counts: HashMap::new(),
    };
    scanner.visit_block(block);

    let mut candidates = HashMap::new();
    for (name, expr) in scanner.declarations {
        if scanner.shadowed.contains(&name) {
            continue;
        }
        let count = scanner.ref_counts.get(&name).copied().unwrap_or(0);
        if count != 1 {
            continue;
        }
        candidates.insert(name, InlineCandidate { expr });
    }

    candidates
}

/// Single-walk scanner behind `find_inline_candidates`: single-name
/// literal declarations, every disqualifying binder (a parameter, loop
/// variable, or function name that shadows a candidate would receive
/// the candidate's value at its use sites - Lua 5.5 `global function`
/// bodies included), and per-name reference counts.
struct CandidateScanner {
    declarations: HashMap<String, Expression>,
    // Every declared single-name local, literal or not: a SECOND
    // declaration of a name disqualifies it even when only one of the
    // two initializers is a literal.
    declared_names: std::collections::HashSet<String>,
    shadowed: std::collections::HashSet<String>,
    ref_counts: HashMap<String, usize>,
}

impl Visitor for CandidateScanner {
    fn visit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                let is_single = local
                    .equal_and_exprs
                    .as_ref()
                    .is_some_and(|(_, exprs)| local.names.len() == 1 && exprs.len() == 1);
                if is_single {
                    let (_, exprs) = local.equal_and_exprs.as_ref().expect("checked above");
                    let name_token = local.names.iter().next().expect("len checked above");
                    let expr = exprs.iter().next().expect("len checked above");
                    let name = ident_name_string(&name_token.name);
                    if self.declared_names.contains(&name) {
                        self.shadowed.insert(name);
                    } else {
                        self.declared_names.insert(name.clone());
                        // Only closed literals can ever inline - skip the
                        // clone for everything else.
                        if is_closed_literal_expr(expr) {
                            self.declarations.insert(name, expr.clone());
                        }
                    }
                } else {
                    // Bare and multi-name locals are binders too: a
                    // reference between `local x` and a later `local x = 1`
                    // resolves to THIS binding, so the name can't inline.
                    for attributed in local.names.iter() {
                        self.shadowed.insert(ident_name_string(&attributed.name));
                    }
                }
            }
            Statement::LocalFunction(func) => {
                self.shadowed.insert(ident_name_string(&func.name));
            }
            Statement::NumericFor(numeric_for) => {
                self.shadowed.insert(ident_name_string(&numeric_for.name));
            }
            Statement::GenericFor(generic_for) => {
                for binding in generic_for.names.iter() {
                    self.shadowed.insert(ident_name_string(&binding.name));
                }
            }
            // No declaration and no binder at this statement itself; the
            // walk below still descends into any nested blocks.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::FunctionDecl(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_function_body(&mut self, body: &FunctionBody) {
        for param in body.params.iter() {
            if let TokenKind::Identifier(name) = &param.name.kind {
                self.shadowed.insert(name.to_string());
            }
        }
        self.walk_function_body(body);
    }

    fn visit_var(&mut self, var: &Var) {
        if let Var::Name(name) = var {
            *self.ref_counts.entry(ident_name_string(name)).or_insert(0) += 1;
        }
        self.walk_var(var);
    }
}

struct Inliner {
    candidates: HashMap<String, InlineCandidate>,
}

impl AstTransform for Inliner {
    fn transform_block(&mut self, block: Block) -> Block {
        let stmts: Vec<_> = block
            .stmts
            .into_iter()
            .filter_map(|stmt| {
                let stmt = self.transform_statement(stmt);
                if self.is_inlined_declaration(&stmt) {
                    None
                } else {
                    Some(stmt)
                }
            })
            .collect();

        let last_stmt = block
            .last_stmt
            .map(|last| Box::new(self.transform_last_statement(*last)));

        Block {
            span: block.span,
            stmts,
            last_stmt,
        }
    }

    fn transform_expression(&mut self, expr: Expression) -> Expression {
        let expr = self.walk_expression(expr);
        if let Expression::Var(ref var) = expr
            && let Var::Name(ref name) = **var
        {
            let var_name = ident_name_string(name);
            if let Some(candidate) = self.candidates.get(&var_name) {
                let replacement = candidate.expr.clone();
                // Bare FunctionDef can't be a call prefix without parens:
                // function() end() is invalid, needs (function() end)()
                if matches!(replacement, Expression::FunctionDef(_)) {
                    return Expression::Parenthesized(Box::new(ParenExpression {
                        span: sp(),
                        parens: ContainedSpan {
                            open: sp(),
                            close: sp(),
                        },
                        expr: replacement,
                    }));
                }
                return replacement;
            }
        }
        expr
    }
}

impl Inliner {
    fn is_inlined_declaration(&self, stmt: &Statement) -> bool {
        if let Statement::LocalAssignment(local) = stmt {
            let names: Vec<_> = local.names.iter().collect();
            if names.len() == 1 {
                let name = ident_name_string(&names[0].name);
                return self.candidates.contains_key(&name);
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = inline(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn inlines_single_use_number() {
        let r = apply("local x = 42\nreturn x\n");
        assert!(!r.contains("local"), "Declaration not removed: {r}");
        assert!(r.contains("42"), "Value not inlined: {r}");
    }

    #[test]
    fn no_inline_multi_use() {
        let r = apply("local x = 42\nprint(x)\nreturn x\n");
        assert!(r.contains("local"), "Multi-use local was inlined: {r}");
    }

    #[test]
    fn no_inline_function_call() {
        let r = apply("local x = foo()\nreturn x\n");
        assert!(r.contains("local"), "Side-effectful init was inlined: {r}");
    }

    #[test]
    fn no_inline_variable_read() {
        let r = apply("local x = y\nreturn x\n");
        assert!(r.contains("local"), "Variable read was inlined: {r}");
    }

    #[test]
    fn inlines_in_nested_function_body() {
        let r = apply("local function foo()\n  local x = 42\n  return x\nend\nreturn foo\n");
        assert!(
            !r.contains("local x"),
            "Declaration inside function body not inlined: {r}"
        );
    }

    #[test]
    fn inlines_in_if_block() {
        let r = apply("if true then\n  local x = 42\n  print(x)\nend\n");
        assert!(
            !r.contains("local x"),
            "Declaration inside if block not inlined: {r}"
        );
    }

    #[test]
    fn no_inline_table_constructor() {
        // Tables carry identity: moved into a loop, `{1,2,3}` would mint
        // a fresh table per iteration.
        let r = apply("local t = {1, 2, 3}\nreturn t\n");
        assert!(r.contains("local"), "Identity value was inlined: {r}");
    }

    #[test]
    fn no_inline_function_def() {
        // Closures carry identity AND captures; both are position-
        // sensitive. `local a=1 local f=function() return a end local a=2`
        // must never see f's body rebind to the second `a`.
        let r = apply("local f = function() return 1 end\nreturn f\n");
        assert!(r.contains("local"), "Closure was inlined: {r}");
    }

    #[test]
    fn inlines_chained_single_use() {
        // Pass 1: x is inlined into y's expr -> local y = 42 + 1; return y
        // Pass 2: y = 42 + 1 is now pure -> inlined -> return 42 + 1
        let r = apply("local x = 42\nlocal y = x + 1\nreturn y\n");
        assert!(!r.contains("local"), "Both should be inlined: {r}");
    }

    #[test]
    fn no_inline_vararg() {
        // `...` changes meaning across function boundaries; substituting
        // it into a nested closure captures the WRONG vararg (or errors).
        let r = apply("local function f(...)\n  local x = ...\n  return x\nend\n");
        assert!(r.contains("local x"), "VarArg was inlined: {r}");
    }

    #[test]
    fn no_inline_vararg_multi_use() {
        let r = apply("local function f(...)\n  local x = ...\n  print(x)\n  return x\nend\n");
        assert!(
            r.contains("local x"),
            "Multi-use vararg should not be inlined: {r}"
        );
    }

    fn apply_luau(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Luau);
        assert!(
            result.errors.is_empty(),
            "parse failed: {:?}",
            result.errors
        );
        let block = inline(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn no_inline_typecast_var_read() {
        // TypeCast is transparent - inner expression (variable read) is impure
        let r = apply_luau("local x = foo :: Bar\nreturn x\n");
        assert!(
            r.contains("local"),
            "TypeCast with var read should not be inlined: {r}"
        );
    }

    fn apply_lua55(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua55);
        assert!(
            result.errors.is_empty(),
            "parse failed: {:?}",
            result.errors
        );
        let block = inline(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn no_inline_across_global_function_shadow() {
        // The outer `local x` has zero reads at top level; the inner
        // `global function` body declares its own `local x` with one read.
        // The inner shadow must be recorded so the outer x is NOT inlined into
        // the inner scope. Regression: collect_declarations used to skip the
        // global-function body, producing `global function g()return 1 end`.
        let r = apply_lua55(
            "local x = 1\nglobal function g()\n  local x = 2\n  return x\nend\nreturn g\n",
        );
        assert!(
            r.contains("local x=2") || r.contains("local x =2") || r.contains("return 2"),
            "Inner shadowed local must be preserved, not replaced by outer value: {r}"
        );
        assert!(
            !r.contains("return 1"),
            "Outer value must not leak into inner scope: {r}"
        );
    }
}
