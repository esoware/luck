use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_ast::visitor::Visitor;
use luck_token::Span;
use luck_token::token::{Token, TokenKind};

use crate::expr::ident_name_string;

/// Rewrite `function X:Y(params)` -> `function X.Y(self, params)` when `self`
/// is referenced enough in the body. The renamer then shortens the explicit
/// parameter from 4 chars to 1.
pub fn rewrite(block: Block) -> Block {
    ExplicitSelfRewriter.transform_block(block)
}

struct ExplicitSelfRewriter;

fn count_self_refs(body: &FunctionBody) -> usize {
    let mut counter = SelfCounter(0);
    counter.visit_block(&body.block);
    counter.0
}

struct SelfCounter(usize);

impl<'ast> Visitor<'ast> for SelfCounter {
    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var {
            if ident_name_string(name) == "self" {
                self.0 += 1;
            }
        }
        self.walk_var(var);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        self.walk_expression(expr);
    }

    // don't descend into nested function bodies - their `self` is separate
    fn visit_function_body(&mut self, _body: &'ast FunctionBody) {}
}

impl AstTransform for ExplicitSelfRewriter {
    fn transform_statement(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::FunctionDecl(mut func_decl) => {
                if let Some((_colon, method_name)) = func_decl.name.method.take() {
                    let self_refs = count_self_refs(&func_decl.body);
                    // cost: adding "X," to params = 2 bytes
                    // saving: 3 bytes per self reference (4 -> 1 after rename)
                    if self_refs >= 2 {
                        func_decl.name.names.push(method_name);
                        func_decl.name.dots.push(Span::default());

                        let self_param = Parameter {
                            span: Span::default(),
                            name: Token::new(TokenKind::Identifier("self".into()), Span::default()),
                            type_annotation: None,
                        };

                        // `self` needs a comma when anything follows it -
                        // named params or a bare `...`.
                        let has_following =
                            !func_decl.body.params.is_empty() || func_decl.body.vararg.is_some();
                        let self_sep = has_following.then(Span::default);
                        func_decl
                            .body
                            .params
                            .items
                            .insert(0, (self_param, self_sep));
                    } else {
                        func_decl.name.method = Some((Span::default(), method_name));
                    }
                }
                func_decl.body = self.walk_function_body(func_decl.body);
                Statement::FunctionDecl(func_decl)
            }
            other => self.walk_statement(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_core::types::LuaTarget;

    fn minify(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = rewrite(result.block);
        let block = crate::transforms::rename_locals::rename(block, LuaTarget::Lua54, true);
        luck_codegen::compact(&block, source)
    }

    fn reparses(source: &str) -> bool {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        result.errors.is_empty()
    }

    #[test]
    fn rewrites_method_with_many_self_refs() {
        let result = minify(
            "local t = {}\nfunction t:method(x)\n  self.a = 1\n  self.b = 2\n  return self.c + x\nend\nreturn t\n",
        );
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }

    #[test]
    fn keeps_method_syntax_with_one_self_ref() {
        let result =
            minify("local t = {}\nfunction t:method(x)\n  return self.a + x\nend\nreturn t\n");
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            result.contains("self"),
            "self should be kept (only 1 ref): {result}"
        );
    }

    #[test]
    fn no_params_case() {
        let result = minify(
            "local t = {}\nfunction t:method()\n  self.a = 1\n  self.b = 2\nend\nreturn t\n",
        );
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }

    #[test]
    fn vararg_only_method_keeps_comma() {
        let result = minify(
            "local t = {}\nfunction t:method(...)\n  self.a = 1\n  self.b = 2\n  return ...\nend\nreturn t\n",
        );
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }
}
