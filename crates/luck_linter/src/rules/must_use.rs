use luck_ast::expr::FunctionCall;
use luck_semantic::SemanticAnalysis;
use luck_semantic::stdlib_model::StdlibEntry;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

/// Warns when a stdlib call that is marked `must_use` (because it
/// returns a value with no observable side effects) is invoked in
/// statement position - i.e. the return value is discarded.
pub struct MustUse;

impl Rule for MustUse {
    fn name(&self) -> &'static str {
        "must_use"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "return value of must-use function is discarded"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn must_use_name(semantic: &SemanticAnalysis, call: &FunctionCall) -> Option<String> {
    let (display_name, resolved) = semantic.resolve_callee(call)?;
    if let StdlibEntry::Function(func) = resolved.entry
        && func.must_use
    {
        Some(display_name)
    } else {
        None
    }
}

impl NodeRule for MustUse {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::FunctionCallStmt]);
        Some(&TYPES)
    }
    // Only function-call *statements* discard the return; expression
    // contexts (assignments, args, returns, etc.) are fine.
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt
            && let Some(name) = must_use_name(ctx.semantic, &call_stmt.call)
        {
            out.push(
                LintDiagnostic::new(
                    "must_use",
                    format!("return value of `{name}` is discarded"),
                    call_stmt.span,
                )
                .with_help("assign the result or remove the call".to_string()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::MustUse;

    fn warnings(source: &str, version: LuaVersion) -> Vec<String> {
        crate::test_support::run_rule(&MustUse, source, version)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn flags_discarded_tostring() {
        let messages = warnings("tostring(1)", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("tostring")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_used_tostring() {
        let messages = warnings("local s = tostring(1)\nprint(s)", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_discarded_pcall() {
        // pcall is newly marked must_use - discarding it usually loses
        // the success bool, which is the whole point of pcall.
        let messages = warnings("pcall(f)", LuaVersion::Lua54);
        assert!(messages.iter().any(|m| m.contains("pcall")), "{messages:?}");
    }

    #[test]
    fn flags_discarded_coroutine_create() {
        let messages = warnings("coroutine.create(f)", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("coroutine.create")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_discarded_table_clone_in_luau() {
        let messages = warnings("table.clone(t)", LuaVersion::Luau);
        assert!(
            messages.iter().any(|m| m.contains("table.clone")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_table_insert() {
        // table.insert mutates - discarding the result is fine.
        let messages = warnings("table.insert(t, 1)", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_method_call() {
        let messages = warnings("obj:tostring()", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_discarded_string_method() {
        let messages = warnings("local s = 'x'\ns:upper()", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("s:upper")),
            "{messages:?}"
        );
    }

    // A bare f:seek('set') rewind is idiomatic; seek is deliberately
    // not must_use.
    #[test]
    fn ignores_discarded_file_seek() {
        let messages = warnings("local f = io.open('x')\nf:seek('set')", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }
}
