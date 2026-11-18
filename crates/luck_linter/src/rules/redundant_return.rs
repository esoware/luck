use luck_ast::{Block, LastStatement};

use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RedundantReturn;

impl_rule!(
    RedundantReturn,
    "redundant_return",
    Style,
    Warning,
    "trailing bare `return` has no effect",
    Fix
);

/// Only the chunk and function bodies are checked, since in a nested block a
/// bare `return` is an early exit.
impl WholeRule for RedundantReturn {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        context
            .nodes
            .function_blocks()
            .iter()
            .filter_map(|block| check_tail(block, context))
            .collect()
    }
}

fn check_tail(block: &Block<'_>, context: &LintContext<'_>) -> Option<Finding> {
    let Some(LastStatement::Return(return_stmt)) = &block.last_stmt else {
        return None;
    };
    if !return_stmt.exprs.is_empty() {
        return None;
    }
    let span = return_stmt.span;
    Some(Finding::new("bare `return` at the end of the function has no effect", span).with_fix(
        context,
        FixKind::Fix,
        |fixer| fixer.delete("remove the redundant `return`", span),
    ))
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&RedundantReturn, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_trailing_bare_return_in_function() {
        let diags = run("\
local function f()
    print(1)
    return
end
f()");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_trailing_bare_return_in_chunk() {
        let diags = run("\
print(1)
return");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_return_only_body() {
        let diags = run("\
local function f()
    return
end
f()");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_trailing_return() {
        let source = "\
local function f()
    print(1)
    return
end
f()";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert!(!fixed.contains("return"), "{fixed}");
    }

    #[test]
    fn flags_and_fixes_trailing_return_with_semicolon() {
        let source = "\
local function f()
    print(1)
    return;
end
f()";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert!(!fixed.contains("return"), "{fixed}");
    }

    #[test]
    fn ignores_return_with_values() {
        let diags = run("\
local function f()
    return 1
end
f()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_early_return_in_nested_block() {
        let diags = run("\
local function f(x)
    if x then
        return
    end
    print(x)
end
f(1)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_nested_function_tail_return() {
        let diags = run("\
local f = function()
    local g = function()
        return
    end
    return g
end
f()");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }
}
