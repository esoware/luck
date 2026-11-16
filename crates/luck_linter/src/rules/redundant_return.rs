use luck_ast::{Block, FunctionBody, LastStatement, Visitor};

use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit};
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RedundantReturn;

impl_rule!(
    RedundantReturn,
    "redundant_return",
    Style,
    Warning,
    "trailing bare `return` has no effect"
);

impl WholeRule for RedundantReturn {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut checker = ReturnChecker { findings: Vec::new() };
        checker.check_tail(context.block);
        checker.visit_block(context.block);
        checker.findings
    }
}

struct ReturnChecker {
    findings: Vec<Finding>,
}

impl ReturnChecker {
    /// Flags a bare `return` that ends `block`. Callers pass only function
    /// bodies and the chunk, since in a nested block it's an early exit.
    fn check_tail(&mut self, block: &Block) {
        let Some(LastStatement::Return(return_stmt)) = block.last_stmt.as_deref() else {
            return;
        };
        if !return_stmt.exprs.is_empty() {
            return;
        }
        self.findings.push(
            Finding::new(
                "bare `return` at the end of the function has no effect",
                return_stmt.span,
            )
            .with_fix(Fix {
                description: "remove the redundant `return`".to_owned(),
                edits: vec![TextEdit { span: return_stmt.span, replacement: String::new() }],
            }),
        );
    }
}

impl<'ast> Visitor<'ast> for ReturnChecker {
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.check_tail(&body.block);
        self.walk_function_body(body);
    }
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
