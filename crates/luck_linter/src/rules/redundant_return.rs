use luck_ast::{AstKind, AstType, Statement};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RedundantReturn;

declare_lint_rule!(
    RedundantReturn,
    Style,
    Warning,
    "trailing bare `return` has no effect",
    fix = Fix
);

/// Only the chunk and function bodies are checked, since in a nested block a
/// bare `return` is an early exit.
impl Rule for RedundantReturn {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        if !matches!(
            context.nodes.parent_kind(node.id()),
            Some(AstKind::FunctionBody(_) | AstKind::Chunk(_))
        ) {
            return;
        }
        let Some(Statement::Return(return_stmt)) = block.stmts.last() else {
            return;
        };
        if !return_stmt.exprs.is_empty() {
            return;
        }
        let span = return_stmt.span;
        context.diagnostic(
            Finding::new("bare `return` at the end of the function has no effect", span)
                .with_tag(Tag::Unnecessary)
                .with_fix(context, FixKind::Fix, |fixer| {
                    fixer.delete("remove the redundant `return`", span)
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::{fix_and_reparse, run_rule};
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::RedundantReturn, source, LuaVersion::Lua54)
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
