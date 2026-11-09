use luck_ast::FunctionCall;
use luck_semantic::{SemanticAnalysis, StdlibEntry};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

/// Flags a call statement to a stdlib function marked `must_use`, which
/// throws away its result.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MustUse;

impl_rule!(
    MustUse,
    "must_use",
    Suspicious,
    Warning,
    "return value of a must-use function is discarded"
);

impl NodeRule for MustUse {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt])
    }
    fn on_call(
        &self,
        call: &FunctionCall,
        position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if position == CallPosition::Statement
            && let Some(name) = must_use_name(context.semantic, call)
        {
            findings.push(
                Finding::new(format!("return value of `{name}` is discarded"), call.span)
                    .with_help("assign the result or remove the call"),
            );
        }
    }
}

fn must_use_name(semantic: &SemanticAnalysis, call: &FunctionCall) -> Option<String> {
    let callee = semantic.resolve_callee(call)?;
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
        run_rule(&MustUse, source, version).into_iter().map(|diag| diag.message).collect()
    }

    #[test]
    fn flags_discarded_tostring() {
        let messages = run("tostring(1)", LuaVersion::Lua54);
        assert!(messages.iter().any(|message| message.contains("tostring")), "{messages:?}");
    }

    #[test]
    fn ignores_used_tostring() {
        let messages = run("local s = tostring(1)\nprint(s)", LuaVersion::Lua54);
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
        let messages = run("local s = 'x'\ns:upper()", LuaVersion::Lua54);
        assert!(messages.iter().any(|message| message.contains("s:upper")), "{messages:?}");
    }

    // A bare f:seek('set') rewind is idiomatic, so seek isn't must_use.
    #[test]
    fn ignores_discarded_file_seek() {
        let messages = run("local f = io.open('x')\nf:seek('set')", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }
}
