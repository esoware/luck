//! `function t:m(x)` becomes `function t.m(self, x)` where its body reads
//! `self` often enough that renaming the parameter pays for the comma.
//!
//! The renamer makes the rewrite as its first walk reaches each method, and
//! merges the implicit `self` into the new parameter, so every reference to
//! it is the parameter's from the start.

use luck_allocator::Allocator;
use luck_ast::{Expression, FunctionBody, FunctionDecl, Name, Parameter, Var, Visit};

use crate::binding::{Bindings, METHOD_RECEIVER_NAME, RECEIVER_NAME_ID, local_index, local_of};

pub(super) fn rewrite<'ast>(
    allocator: &'ast Allocator,
    bindings: &mut Bindings,
    function_declaration: &mut FunctionDecl<'ast>,
) {
    if function_declaration.name.method.is_none() {
        return;
    }
    let mut counter = SelfCounter { bindings, receiver: None, count: 0 };
    counter.visit_block(&function_declaration.body.block);
    // The new `X,` parameter costs 2 bytes, and each `self` saves 3 once
    // renamed to a single character.
    if counter.count < 2 {
        return;
    }
    let (Some(receiver), Some(method)) =
        (counter.receiver, function_declaration.name.method.take())
    else {
        return;
    };
    function_declaration.name.names.push(method, allocator);
    let parameter = bindings.push(RECEIVER_NAME_ID, false, 0);
    if let Some(binding) = bindings.entries.get_mut(receiver) {
        binding.merged_into = u32::try_from(parameter).unwrap_or(binding.merged_into);
    }
    let name = Name::new(METHOD_RECEIVER_NAME.into(), Some(local_index(parameter)), method.span);
    function_declaration.body.params.insert(0, Parameter::new(name, None), allocator);
}

/// Counts the references to the method's own `self`.
struct SelfCounter<'bindings> {
    bindings: &'bindings Bindings,
    receiver: Option<usize>,
    count: usize,
}

impl SelfCounter<'_> {
    fn count(&mut self, name: &Name<'_>) {
        if let Some(id) = local_of(name)
            && let Some(binding) = self.bindings.entries.get(id)
            && binding.is_fixed
            && usize::try_from(binding.name).ok() == Some(RECEIVER_NAME_ID)
        {
            self.receiver = Some(id);
            self.count = self.count.saturating_add(1);
        }
    }
}

impl<'tree> Visit<'tree> for SelfCounter<'_> {
    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if let Expression::Name(name) = expr {
            self.count(name);
        }
        self.walk_expr(expr);
    }

    fn visit_var(&mut self, var: &'tree Var<'tree>) {
        if let Var::Name(name) = var {
            self.count(name);
        }
        self.walk_var(var);
    }

    // Skips nested functions. A nested method binds its own `self`, and
    // missing a closure's uses of the outer `self` only undercounts.
    fn visit_function_body(&mut self, _body: &'tree FunctionBody<'tree>) {}
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::fixtures::{RENAMING, minify_with};

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
