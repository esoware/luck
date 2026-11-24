//! `function t:m(x)` becomes `function t.m(self, x)` where its body reads
//! `self` often enough that renaming the parameter pays for the comma.
//!
//! The renamer picks those methods from the references to each implicit
//! `self`, names the `self` as it names a parameter, and makes the rewrite
//! as its walk leaves each one.

use luck_allocator::Allocator;
use luck_ast::{FunctionDecl, Name, Parameter};

/// Makes `function t:m(...)` `function t.m(receiver, ...)`.
pub(super) fn take_receiver<'ast>(
    allocator: &'ast Allocator,
    function_declaration: &mut FunctionDecl<'ast>,
    receiver: Name<'ast>,
) {
    if let Some(method) = function_declaration.name.method.take() {
        function_declaration.name.names.push(method, allocator);
        function_declaration.body.params.insert(0, Parameter::new(receiver, None), allocator);
    }
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
