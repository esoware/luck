use luck_ast::{AstKind, AstType, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{ResolvedCallee, StdlibEntry};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct MustUse;

impl_rule!(
    MustUse,
    "must_use",
    Suspicious,
    Warning,
    "return value of a must-use function is discarded"
);

/// A call statement drops the results of its chain's last call.
impl RuleRunner for MustUse {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::SuffixedExpression])
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        if !context.is_statement(node) {
            return;
        }
        let Some(call) = chain.last_call() else {
            return;
        };
        let last = chain.suffixes.len().saturating_sub(1);
        if let Some(name) = context.callee(node, last).and_then(must_use_name) {
            context.diagnostic(
                Finding::new(format!("return value of `{name}` is discarded"), call.span())
                    .with_help("assign the result or remove the call"),
            );
        }
    }
}

fn must_use_name(callee: ResolvedCallee<'_>) -> Option<String> {
    if let StdlibEntry::Function(function) = callee.entry()
        && function.is_must_use
    {
        Some(callee.name())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::run_rule;

    fn run(source: &str, version: LuaVersion) -> Vec<String> {
        run_rule(&MustUse, source, version)
            .into_iter()
            .map(|diag| diag.message.to_string())
            .collect()
    }

    #[test]
    fn flags_discarded_tostring() {
        let messages = run("tostring(1)", LuaVersion::Lua54);
        assert!(messages.iter().any(|message| message.contains("tostring")), "{messages:?}");
    }

    #[test]
    fn ignores_used_tostring() {
        let messages = run(
            "\
local s = tostring(1)
print(s)",
            LuaVersion::Lua54,
        );
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_discarded_pcall() {
        // Discarding a pcall loses the success bool, which is the whole
        // point of calling it.
        let messages = run("pcall(f)", LuaVersion::Lua54);
        assert!(messages.iter().any(|message| message.contains("pcall")), "{messages:?}");
    }

    #[test]
    fn flags_discarded_coroutine_create() {
        let messages = run("coroutine.create(f)", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|message| message.contains("coroutine.create")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_discarded_table_clone_in_luau() {
        let messages = run("table.clone(t)", LuaVersion::Luau);
        assert!(messages.iter().any(|message| message.contains("table.clone")), "{messages:?}");
    }

    #[test]
    fn ignores_table_insert() {
        // table.insert mutates, so discarding the result is fine.
        let messages = run("table.insert(t, 1)", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_method_call() {
        let messages = run("obj:tostring()", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_discarded_string_method() {
        let messages = run(
            "\
local s = 'x'
s:upper()",
            LuaVersion::Lua54,
        );
        assert!(messages.iter().any(|message| message.contains("s:upper")), "{messages:?}");
    }

    // A bare f:seek('set') rewind is idiomatic, so seek isn't must_use.
    #[test]
    fn ignores_discarded_file_seek() {
        let messages = run(
            "\
local f = io.open('x')
f:seek('set')",
            LuaVersion::Lua54,
        );
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_discarded_collectgarbage_on_luau() {
        let messages = run("collectgarbage()", LuaVersion::Luau);
        assert!(messages.is_empty(), "the call runs a full collection: {messages:?}");
    }
}
