//! `function t:m(x)` becomes `function t.m(self, x)` where its body reads
//! `self` often enough that renaming the parameter pays for the comma.

use luck_allocator::Allocator;
use luck_ast::{
    Block, Expression, FunctionBody, Name, Parameter, Statement, Var, Visitor, VisitorMut,
};

use crate::binding::METHOD_RECEIVER_NAME;

pub(crate) fn rewrite<'ast>(allocator: &'ast Allocator, block: &mut Block<'ast>) -> bool {
    let mut rewriter = ExplicitSelf { allocator, has_changed: false };
    rewriter.visit_block(block);
    rewriter.has_changed
}

struct ExplicitSelf<'ast> {
    allocator: &'ast Allocator,
    has_changed: bool,
}

impl<'ast> VisitorMut<'ast> for ExplicitSelf<'ast> {
    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        if let Statement::FunctionDecl(function_declaration) = stmt
            && function_declaration.name.method.is_some()
            // The new `X,` parameter costs 2 bytes, and each `self` saves 3
            // once renamed to a single character.
            && count_self_references(&function_declaration.body) >= 2
            && let Some(method) = function_declaration.name.method.take()
        {
            function_declaration.name.names.push(method, self.allocator);
            let name = Name { text: METHOD_RECEIVER_NAME, span: method.span };
            let receiver = Parameter { name, type_annotation: None };
            function_declaration.body.params.insert(0, receiver, self.allocator);
            self.has_changed = true;
        }
        self.walk_stmt(stmt);
    }
}

fn count_self_references(body: &FunctionBody<'_>) -> usize {
    let mut counter = SelfCounter(0);
    counter.visit_block(&body.block);
    counter.0
}

struct SelfCounter(usize);

impl SelfCounter {
    fn count(&mut self, name: &Name<'_>) {
        if name.text == METHOD_RECEIVER_NAME {
            self.0 = self.0.saturating_add(1);
        }
    }
}

impl<'tree> Visitor<'tree> for SelfCounter {
    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if let Expression::Name(name) = expr {
            self.count(name);
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &'tree Var<'tree>) {
        if let Var::Name(name) = target {
            self.count(name);
        }
        self.walk_assignment_target(target);
    }

    // Skips nested functions. A nested method binds its own `self`, and
    // missing a closure's uses of the outer `self` only undercounts.
    fn visit_function_body(&mut self, _body: &'tree FunctionBody<'tree>) {}
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const RENAMING: TransformConfig =
        TransformConfig { rename_locals: true, rename_globals: true, ..NO_PASSES };

    /// Rewrites, then renames, so an explicit `self` shows up renamed.
    fn rewrite_and_rename(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &RENAMING)
    }

    #[test]
    fn rewrites_method_with_many_self_references() {
        let result = rewrite_and_rename(
            "\
local t = {}
function t:method(x)
  self.a = 1
  self.b = 2
  return self.c + x
end
return t
",
        );
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }

    #[test]
    fn keeps_method_syntax_with_one_self_reference() {
        let result = rewrite_and_rename(
            "\
local t = {}
function t:method(x)
  return self.a + x
end
return t
",
        );
        assert!(result.contains("self"), "self should be kept (only 1 ref): {result}");
    }

    #[test]
    fn rewrites_method_without_params() {
        let result = rewrite_and_rename(
            "\
local t = {}
function t:method()
  self.a = 1
  self.b = 2
end
return t
",
        );
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }

    #[test]
    fn vararg_only_method_keeps_comma() {
        let result = rewrite_and_rename(
            "\
local t = {}
function t:method(...)
  self.a = 1
  self.b = 2
  return ...
end
return t
",
        );
        assert!(!result.contains("self"), "self should be renamed: {result}");
    }

    #[test]
    fn nested_method_keeps_its_own_self_after_the_rewrite() {
        let result = rewrite_and_rename(
            "\
function M:outer()
  print(self, self)
  function t:inner() return self end
end
",
        );
        assert!(
            result.contains(":inner()return self"),
            "the nested method's `self` is its own receiver, not the rewritten parameter: {result}"
        );
    }
}
