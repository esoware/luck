use std::collections::HashSet;

use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_ast::visitor::Visitor;
use luck_token::Span;
use luck_token::token::{Token, TokenKind};

use crate::expr::ident_name_string;
use crate::tokens::default_span as sp;

/// Lift local declarations to function scope, eliminating redundant `local` keywords.
/// Runs post-rename: the renamer guarantees non-overlapping lifetimes for same-named
/// bindings, but parent-child slot reuse means a child local can share a name with a
/// parent local. We only lift when the name doesn't appear in any ancestor scope.
pub fn lift(block: Block) -> Block {
    let mut lifter = Lifter;
    lifter.transform_block(block)
}

struct Lifter;

impl AstTransform for Lifter {
    fn walk_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        let param_names: HashSet<String> = body
            .params
            .iter()
            .map(|p| ident_name_string(&p.name))
            .collect();

        // A captured local's lifetime is pinned by the closure holding it:
        // merging it into a shared function-scope slot makes closures from
        // sibling scopes (or loop iterations) observe each other's writes.
        // Any captured name is ineligible everywhere, not just in loops.
        let captured = collect_closure_captures(&body.block);

        let mut liftable_set = HashSet::new();
        let mut liftable_ordered = Vec::new();
        let mut scope_names = vec![param_names.clone()];
        collect_liftable(
            &body.block,
            &mut scope_names,
            false,
            &captured,
            &mut liftable_set,
            &mut liftable_ordered,
        );

        if liftable_set.is_empty() {
            body.block = self.transform_block(body.block);
            return body;
        }

        body.block = rewrite_block(body.block, &liftable_set, &mut Vec::new());
        body.block = self.transform_block(body.block);

        prepend_declaration(&mut body.block, &liftable_ordered);

        body
    }
}

/// Walk a block and determine which locals can be lifted.
/// A local is liftable if:
/// - its name doesn't appear in any ancestor scope (avoids clobbering parent bindings)
/// - it's not inside a loop where a closure captures it
/// - it has no attributes (<const>, <close>)
fn collect_liftable(
    block: &Block,
    scope_names: &mut Vec<HashSet<String>>,
    in_loop: bool,
    captured: &HashSet<String>,
    liftable: &mut HashSet<String>,
    liftable_ordered: &mut Vec<String>,
) {
    for stmt in &block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                // Const bindings cannot be lifted: the hoisted bare
                // declaration would lack the mandatory initializer and
                // the later write would assign to a const.
                let has_attribs = local.names.iter().any(|n| n.attrib.is_some()) || local.is_const;
                // A bare `local x` inside a loop resets to nil each
                // iteration; lifted to function scope it would keep the
                // previous iteration's value.
                let bare_in_loop = in_loop && local.equal_and_exprs.is_none();
                if has_attribs || bare_in_loop {
                    for name in local.names.iter() {
                        let n = ident_name_string(&name.name);
                        scope_names.last_mut().unwrap().insert(n);
                    }
                    continue;
                }
                let names: Vec<String> = local
                    .names
                    .iter()
                    .map(|attributed| ident_name_string(&attributed.name))
                    .collect();
                let mut all_safe = true;
                for name in &names {
                    if is_in_parent(scope_names, name) {
                        all_safe = false;
                        break;
                    }
                    if captured.contains(name) {
                        all_safe = false;
                        break;
                    }
                }
                if all_safe {
                    for name in &names {
                        if liftable.insert(name.clone()) {
                            liftable_ordered.push(name.clone());
                        }
                    }
                }
                for name in names {
                    scope_names.last_mut().unwrap().insert(name);
                }
            }
            Statement::LocalFunction(lf) => {
                let name = ident_name_string(&lf.name);
                if !lf.is_const
                    && !is_in_parent(scope_names, &name)
                    && !captured.contains(&name)
                    && liftable.insert(name.clone())
                {
                    liftable_ordered.push(name.clone());
                }
                scope_names.last_mut().unwrap().insert(name);
                // don't recurse into function body - it gets its own lift pass
            }
            Statement::DoBlock(do_block) => {
                scope_names.push(HashSet::new());
                collect_liftable(
                    &do_block.block,
                    scope_names,
                    in_loop,
                    captured,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            Statement::WhileLoop(while_loop) => {
                scope_names.push(HashSet::new());
                collect_liftable(
                    &while_loop.block,
                    scope_names,
                    true,
                    captured,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            Statement::RepeatLoop(repeat_loop) => {
                scope_names.push(HashSet::new());
                collect_liftable(
                    &repeat_loop.block,
                    scope_names,
                    true,
                    captured,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            Statement::IfStatement(if_stmt) => {
                scope_names.push(HashSet::new());
                collect_liftable(
                    &if_stmt.block,
                    scope_names,
                    in_loop,
                    captured,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
                for clause in &if_stmt.elseif_clauses {
                    scope_names.push(HashSet::new());
                    collect_liftable(
                        &clause.block,
                        scope_names,
                        in_loop,
                        captured,
                        liftable,
                        liftable_ordered,
                    );
                    scope_names.pop();
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    scope_names.push(HashSet::new());
                    collect_liftable(
                        &else_clause.block,
                        scope_names,
                        in_loop,
                        captured,
                        liftable,
                        liftable_ordered,
                    );
                    scope_names.pop();
                }
            }
            Statement::NumericFor(nf) => {
                scope_names.push(HashSet::new());
                let var_name = ident_name_string(&nf.name);
                scope_names.last_mut().unwrap().insert(var_name);
                collect_liftable(
                    &nf.block,
                    scope_names,
                    true,
                    captured,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            Statement::GenericFor(gf) => {
                scope_names.push(HashSet::new());
                for binding in gf.names.iter() {
                    scope_names
                        .last_mut()
                        .unwrap()
                        .insert(ident_name_string(&binding.name));
                }
                collect_liftable(
                    &gf.block,
                    scope_names,
                    true,
                    captured,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            // No `local` binding to lift at this level. Function declarations
            // (including Lua 5.5 `global function`) get their own lift pass via
            // walk_function_body, so their bodies are not traversed here.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
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
    }
}

fn is_in_parent(scope_names: &[HashSet<String>], name: &str) -> bool {
    scope_names.iter().any(|frame| frame.contains(name))
}

/// Collect all variable names referenced by function definitions (closures) in a block.
/// Used to detect closure captures inside loops.
fn collect_closure_captures(block: &Block) -> HashSet<String> {
    let mut captures = HashSet::new();
    let mut collector = ClosureCaptureCollector {
        captures: &mut captures,
    };
    collector.visit_block(block);
    captures
}

struct ClosureCaptureCollector<'a> {
    captures: &'a mut HashSet<String>,
}

impl<'ast> Visitor<'ast> for ClosureCaptureCollector<'_> {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionDef(func_def) = expr {
            let mut name_collector = NameCollector {
                names: self.captures,
            };
            name_collector.visit_block(&func_def.body.block);
            return;
        }
        self.walk_expression(expr);
    }

    fn visit_statement(&mut self, stmt: &'ast Statement) {
        // Statement-level function bodies capture too - `local function f()
        // return m end` holds `m` exactly like an expression closure does.
        match stmt {
            Statement::LocalFunction(func) => {
                let mut name_collector = NameCollector {
                    names: self.captures,
                };
                name_collector.visit_block(&func.body.block);
            }
            Statement::FunctionDecl(decl) => {
                let mut name_collector = NameCollector {
                    names: self.captures,
                };
                name_collector.visit_block(&decl.body.block);
            }
            _ => self.walk_statement(stmt),
        }
    }
}

struct NameCollector<'a> {
    names: &'a mut HashSet<String>,
}

impl<'ast> Visitor<'ast> for NameCollector<'_> {
    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var {
            self.names.insert(ident_name_string(name));
        }
        self.walk_var(var);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        self.walk_expression(expr);
    }
}

/// Rewrite a block: convert lifted `local X=Y` to `X=Y`, remove bare `local X`.
/// `shadowed` carries names bound by NON-lifted declarations (loop control
/// variables, kept locals) on the path here: a declaration whose name is
/// currently shadowed must stay a `local` - rewriting it into an
/// assignment would rebind it to the shadowing binding, not the hoisted
/// one (e.g. a renamed shadow inside `for l = ...` assigning the loop var).
fn rewrite_block(
    block: Block,
    liftable: &HashSet<String>,
    shadowed: &mut Vec<HashSet<String>>,
) -> Block {
    let mut new_stmts: Vec<Statement> = Vec::new();
    shadowed.push(HashSet::new());

    for stmt in block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                let names: Vec<String> = local
                    .names
                    .iter()
                    .map(|attributed| ident_name_string(&attributed.name))
                    .collect();
                let all_lifted = names.iter().all(|n| liftable.contains(n))
                    && !names
                        .iter()
                        .any(|n| shadowed.iter().any(|frame| frame.contains(n)));

                if all_lifted {
                    if let Some((_, exprs)) = local.equal_and_exprs {
                        // local a,b=X,Y -> a,b=X,Y
                        let targets = Punctuated {
                            items: local
                                .names
                                .items
                                .into_iter()
                                .map(|(attributed, sep)| (Var::Name(attributed.name), sep))
                                .collect(),
                        };
                        new_stmts.push(Statement::Assignment(Box::new(Assignment {
                            span: local.span,
                            targets,
                            equal: sp(),
                            values: exprs,
                        })));
                    }
                    // bare `local X` with no values: just drop it
                } else {
                    for name in names {
                        shadowed
                            .last_mut()
                            .expect("frame pushed above")
                            .insert(name);
                    }
                    new_stmts.push(Statement::LocalAssignment(local));
                }
            }
            Statement::LocalFunction(lf) => {
                let name = ident_name_string(&lf.name);
                if liftable.contains(&name) && !shadowed.iter().any(|frame| frame.contains(&name)) {
                    // local function f(x)...end -> f=function(x)...end
                    let func_expr = Expression::FunctionDef(Box::new(FunctionDef {
                        span: lf.span,
                        attributes: lf.attributes,
                        function_token: lf.function_token,
                        body: lf.body,
                    }));
                    let body_block = rewrite_block_in_funcdef(func_expr, liftable);
                    new_stmts.push(Statement::Assignment(Box::new(Assignment {
                        span: lf.span,
                        targets: Punctuated::from_item(Var::Name(lf.name)),
                        equal: sp(),
                        values: Punctuated::from_item(body_block),
                    })));
                } else {
                    shadowed
                        .last_mut()
                        .expect("frame pushed above")
                        .insert(name);
                    new_stmts.push(Statement::LocalFunction(lf));
                }
            }
            Statement::DoBlock(mut do_block) => {
                do_block.block = rewrite_block(do_block.block, liftable, shadowed);
                new_stmts.push(Statement::DoBlock(do_block));
            }
            Statement::WhileLoop(mut wl) => {
                wl.block = rewrite_block(wl.block, liftable, shadowed);
                new_stmts.push(Statement::WhileLoop(wl));
            }
            Statement::RepeatLoop(mut rl) => {
                rl.block = rewrite_block(rl.block, liftable, shadowed);
                new_stmts.push(Statement::RepeatLoop(rl));
            }
            Statement::IfStatement(mut if_stmt) => {
                if_stmt.block = rewrite_block(if_stmt.block, liftable, shadowed);
                if_stmt.elseif_clauses = if_stmt
                    .elseif_clauses
                    .into_iter()
                    .map(|mut clause| {
                        clause.block = rewrite_block(clause.block, liftable, shadowed);
                        clause
                    })
                    .collect();
                if_stmt.else_clause = if_stmt.else_clause.map(|mut ec| {
                    ec.block = rewrite_block(ec.block, liftable, shadowed);
                    ec
                });
                new_stmts.push(Statement::IfStatement(if_stmt));
            }
            Statement::NumericFor(mut nf) => {
                let mut frame = HashSet::new();
                frame.insert(ident_name_string(&nf.name));
                shadowed.push(frame);
                nf.block = rewrite_block(nf.block, liftable, shadowed);
                shadowed.pop();
                new_stmts.push(Statement::NumericFor(nf));
            }
            Statement::GenericFor(mut gf) => {
                let mut frame = HashSet::new();
                for binding in gf.names.iter() {
                    frame.insert(ident_name_string(&binding.name));
                }
                shadowed.push(frame);
                gf.block = rewrite_block(gf.block, liftable, shadowed);
                shadowed.pop();
                new_stmts.push(Statement::GenericFor(gf));
            }
            // No lifted local to rewrite here. Function declarations (including
            // Lua 5.5 `global function`) get their own lift pass, so their
            // bodies pass through unchanged.
            stmt @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
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
            | Statement::Error(_)) => new_stmts.push(stmt),
        }
    }

    shadowed.pop();
    Block {
        span: block.span,
        stmts: new_stmts,
        last_stmt: block.last_stmt,
    }
}

fn rewrite_block_in_funcdef(expr: Expression, _liftable: &HashSet<String>) -> Expression {
    // don't rewrite inside nested function defs - they get their own lift pass
    expr
}

fn prepend_declaration(block: &mut Block, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let name_tokens: Vec<AttributedName> = names
        .iter()
        .map(|n| AttributedName {
            name: Token::new(TokenKind::Identifier(n.into()), Span::default()),
            type_annotation: None,
            attrib: None,
        })
        .collect();
    let total = name_tokens.len();
    let names = Punctuated {
        items: name_tokens
            .into_iter()
            .enumerate()
            .map(|(idx, attributed)| {
                let sep = (idx + 1 < total).then(sp);
                (attributed, sep)
            })
            .collect(),
    };

    block.stmts.insert(
        0,
        Statement::LocalAssignment(Box::new(LocalAssignment {
            span: sp(),
            local_token: sp(),
            names,
            equal_and_exprs: None,
            is_const: false,
        })),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_core::types::LuaTarget;

    fn minify(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = crate::transforms::rename_locals::rename(result.block, LuaTarget::Lua54, true);
        let block = lift(block);
        luck_codegen::compact(&block, source)
    }

    fn reparses(source: &str) -> bool {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        result.errors.is_empty()
    }

    #[test]
    fn lifts_nested_locals() {
        let result = minify(concat!(
            "local function f(x)\n",
            "  local a = 1\n",
            "  if x then\n",
            "    local b = 2\n",
            "    print(a, b)\n",
            "  end\n",
            "  return a\n",
            "end\n",
            "return f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        // Should have fewer `local` keywords than original
    }

    #[test]
    fn does_not_lift_parent_shadowed() {
        // After rename, if inner and outer share a name, inner must keep `local`
        let result = minify(concat!(
            "local function f()\n",
            "  local a = 1\n",
            "  do\n",
            "    local b = 2\n", // might get same name as a after rename
            "    print(b)\n",
            "  end\n",
            "  return a\n", // a is still needed after do-block
            "end\n",
            "return f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
    }

    #[test]
    fn does_not_lift_closure_in_loop() {
        let result = minify(concat!(
            "local t = {}\n",
            "for i = 1, 10 do\n",
            "  local x = i\n",
            "  t[i] = function() return x end\n",
            "end\n",
            "return t\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        // x must keep its `local` to preserve per-iteration binding
        assert!(
            result.contains("local"),
            "closure-captured loop local must keep local: {result}"
        );
    }

    #[test]
    fn lifts_loop_local_without_closure() {
        let result = minify(concat!(
            "local s = 0\n",
            "for i = 1, 10 do\n",
            "  local x = i * 2\n",
            "  s = s + x\n",
            "end\n",
            "return s\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
    }

    #[test]
    fn shadowed_declaration_is_never_rewritten_to_assignment() {
        // The lift rewrite matches by NAME: a shadowing declaration that
        // shares a lifted name must stay a `local`, or its rewrite would
        // rebind to the shadowing binding (here, the for control var).
        let result = minify(concat!(
            "function W.new(c)\n",
            "  for l = 1, 3 do\n",
            "    if c then\n",
            "      local l = f()\n",
            "      print(l)\n",
            "    end\n",
            "  end\n",
            "  while c do\n",
            "    local l = g()\n",
            "    print(l)\n",
            "  end\n",
            "end\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        // The declaration under the for-var shadow must survive as a
        // `local` (the while-loop one may be lifted).
        assert!(
            result.contains("local l=f()") || result.contains("local l = f()"),
            "shadowed decl must stay local: {result}"
        );
    }
}
