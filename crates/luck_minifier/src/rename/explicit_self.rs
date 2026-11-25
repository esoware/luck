//! `function t:m(x)` becomes `function t.m(self, x)` where its body reads
//! `self`, since renaming the parameter pays for the comma.
//!
//! The renamer picks those methods from the references to each implicit
//! `self`, names the `self` as it names a parameter, and makes the rewrite
//! as its walk leaves each one.

use luck_allocator::Allocator;
use luck_ast::{FunctionDecl, Name, TypedName};

/// Makes `function t:m(...)` `function t.m(receiver, ...)`.
pub(super) fn take_receiver<'ast>(
    allocator: &'ast Allocator,
    function_declaration: &mut FunctionDecl<'ast>,
    receiver: Name<'ast>,
) {
    if let Some(method) = function_declaration.name.method.take() {
        function_declaration.name.names.push(method, allocator);
        function_declaration.body.params.insert(0, TypedName::new(receiver, None), allocator);
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::{RENAMING, minify_with};

    /// Rewrites, then renames, so an explicit `self` shows up renamed.
    fn rewrite_and_rename(source: &str) -> String {
        minify_with(source, &RENAMING)
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
    fn rewrites_method_with_one_self_reference() {
        let result = rewrite_and_rename(
            "\
local t = {}
function t:method(x)
  return self.a + x
end
return t
",
        );
        assert_eq!(result, "local e={}function e.method(e,t)return e.a+t end return e");
    }

    #[test]
    fn keeps_method_syntax_where_nothing_reads_self() {
        let result = rewrite_and_rename(
            "\
local t = {}
function t:method(x)
  return x
end
return t
",
        );
        assert_eq!(result, "local e={}function e:method(e)return e end return e");
        let typed = minify_with(
            "\
function t:method(x)
  local y: typeof(self) = x
  return y
end
",
            &RENAMING,
        );
        assert_eq!(typed, "function t:method(e)local t:typeof(self)=e return t end");
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
  local x = self
  function t:inner() return self, x end
  return x
end
",
        );
        assert_eq!(
            result,
            "function M.outer(e)local n=e function t.inner(e)return e,n end return n end"
        );
    }
}
