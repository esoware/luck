use luck_ast::{Block, FunctionBody, Parameter, Statement, Transform, Var, Visitor};
use luck_token::Span;

use crate::binding::{identifier_name, identifier_token};

pub(crate) fn rewrite(block: Block) -> Block {
    ExplicitSelfRewriter.transform_block(block)
}

struct ExplicitSelfRewriter;

fn count_self_references(body: &FunctionBody) -> usize {
    let mut counter = SelfCounter(0);
    counter.visit_block(&body.block);
    counter.0
}

struct SelfCounter(usize);

impl<'ast> Visitor<'ast> for SelfCounter {
    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var
            && identifier_name(name) == "self"
        {
            self.0 = self.0.saturating_add(1);
        }
        self.walk_var(var);
    }

    // Skips nested functions. A nested method binds its own `self`, and
    // missing a closure's uses of the outer `self` only undercounts.
    fn visit_function_body(&mut self, _body: &'ast FunctionBody) {}
}

impl Transform for ExplicitSelfRewriter {
    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::FunctionDecl(mut function_declaration) => {
                // The new `X,` parameter costs 2 bytes, and each `self` saves
                // 3 once renamed to a single character.
                if function_declaration.name.method.is_some()
                    && count_self_references(&function_declaration.body) >= 2
                    && let Some(method_name) = function_declaration.name.method.take()
                {
                    function_declaration.name.names.push(method_name);
                    let self_param = Parameter {
                        span: Span::default(),
                        name: identifier_token("self".into()),
                        type_annotation: None,
                    };
                    function_declaration.body.params.items.insert(0, self_param);
                }
                function_declaration.body = self.walk_function_body(function_declaration.body);
                Statement::FunctionDecl(function_declaration)
            }
            other @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_)) => self.walk_stmt(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};
    use crate::transforms::rename_locals::rename;

    /// Rewrites, then renames, so an explicit `self` shows up renamed.
    fn rewrite_and_rename(source: &str) -> String {
        let output = run_pass(source, LuaVersion::Lua54, |block| {
            rename(rewrite(block), LuaVersion::Lua54, true)
        });
        assert_reparses(&output, LuaVersion::Lua54);
        output
    }

    #[test]
    fn rewrites_method_with_many_self_references() {
        let result = rewrite_and_rename(
            "local t = {}\nfunction t:method(x)\n  self.a = 1\n  self.b = 2\n  return self.c + x\nend\nreturn t\n",
        );
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }

    #[test]
    fn keeps_method_syntax_with_one_self_reference() {
        let result = rewrite_and_rename(
            "local t = {}\nfunction t:method(x)\n  return self.a + x\nend\nreturn t\n",
        );
        assert!(result.contains("self"), "self should be kept (only 1 ref): {result}");
    }

    #[test]
    fn rewrites_method_without_params() {
        let result = rewrite_and_rename(
            "local t = {}\nfunction t:method()\n  self.a = 1\n  self.b = 2\nend\nreturn t\n",
        );
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }

    #[test]
    fn vararg_only_method_keeps_comma() {
        let result = rewrite_and_rename(
            "local t = {}\nfunction t:method(...)\n  self.a = 1\n  self.b = 2\n  return ...\nend\nreturn t\n",
        );
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }

    #[test]
    fn nested_method_keeps_its_own_self_after_the_rewrite() {
        let result = rewrite_and_rename(
            "function M:outer()\n  print(self, self)\n  function t:inner() return self end\nend\n",
        );
        assert!(
            result.contains(":inner()return self"),
            "the nested method's `self` is its own receiver, not the rewritten parameter: {result}"
        );
    }
}
