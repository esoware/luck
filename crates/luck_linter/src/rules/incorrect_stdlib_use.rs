use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall};
use luck_semantic::SemanticAnalysis;
use luck_semantic::stdlib_model::{StdlibArgKind, StdlibConstant, StdlibEntry, StdlibFunction};
use luck_token::Span;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct IncorrectStdlibUse;

impl Rule for IncorrectStdlibUse {
    fn name(&self) -> &'static str {
        "incorrect_stdlib_use"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "wrong argument count or invalid constant for known standard library function"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

struct StdlibChecker<'a, 'out> {
    source: &'a str,
    semantic: &'a SemanticAnalysis,
    out: &'out mut Vec<LintDiagnostic>,
}

impl StdlibChecker<'_, '_> {
    fn check_call(&mut self, call: &FunctionCall) {
        let Some((name, resolved)) = self.semantic.resolve_callee(call) else {
            return;
        };
        let StdlibEntry::Function(func) = resolved.entry else {
            return;
        };

        let positional_args: Vec<&Expression> = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => args.iter().collect(),
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => Vec::new(),
        };

        let arg_count = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => args.len(),
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => 1,
        };

        // A call or `...` in tail position expands to any number of
        // values, so the static count is a lower bound only - checks
        // that need an exact count can't fire.
        let has_multi_value_tail = positional_args.last().is_some_and(|last| {
            matches!(last, Expression::FunctionCall(_) | Expression::VarArg(_))
        });

        let min = func.min_args();
        if arg_count < min && !has_multi_value_tail {
            self.out.push(LintDiagnostic::new(
                "incorrect_stdlib_use",
                format!("'{name}' requires at least {min} argument(s), got {arg_count}"),
                call.span,
            ));
            return;
        }
        if let Some(max) = func.max_args()
            && arg_count > max
        {
            self.out.push(LintDiagnostic::new(
                "incorrect_stdlib_use",
                format!("'{name}' accepts at most {max} argument(s), got {arg_count}"),
                call.span,
            ));
            return;
        }
        // Between the overall bounds but in a gap between overloads
        // (e.g. an entry accepting exactly 0 or 2 arguments, called
        // with 1).
        if !func.accepts_arg_count(arg_count) && !has_multi_value_tail {
            self.out.push(LintDiagnostic::new(
                "incorrect_stdlib_use",
                format!("no overload of '{name}' accepts {arg_count} argument(s)"),
                call.span,
            ));
            return;
        }

        self.check_constant_params(&name, func, arg_count, &positional_args);
    }

    /// Constant-typed parameter check. Only applies to a literal string
    /// argument at a fixed parameter position - we can't statically
    /// resolve dynamic values. With overloads, a value passes if any
    /// signature accepting this arg count allows it (or leaves the
    /// position unconstrained).
    fn check_constant_params(
        &mut self,
        name: &str,
        func: &StdlibFunction,
        arg_count: usize,
        positional_args: &[&Expression],
    ) {
        for (idx, expr) in positional_args.iter().enumerate() {
            let mut allowed: Vec<&StdlibConstant> = Vec::new();
            let mut constrained = false;
            let mut unconstrained = false;
            for sig in func.matching_signatures(arg_count) {
                match sig.params.get(idx).map(|param| &param.kind) {
                    Some(StdlibArgKind::Constant(values)) => {
                        constrained = true;
                        allowed.extend(values.iter());
                    }
                    _ => unconstrained = true,
                }
            }
            if !constrained || unconstrained {
                continue;
            }
            let Some((value, span)) = string_literal_value(expr, self.source) else {
                continue;
            };
            if !allowed.iter().any(|constant| constant.value == value) {
                // Generated sets (Roblox service and class names) run to
                // hundreds of values; cap the rendered list.
                const LIST_LIMIT: usize = 8;
                let mut allowed_list = allowed
                    .iter()
                    .take(LIST_LIMIT)
                    .map(|constant| format!("'{}'", constant.value))
                    .collect::<Vec<_>>()
                    .join(", ");
                if allowed.len() > LIST_LIMIT {
                    allowed_list.push_str(&format!(", ... ({} allowed values)", allowed.len()));
                }
                let position = idx + 1;
                self.out.push(LintDiagnostic::new(
                    "incorrect_stdlib_use",
                    format!(
                        "'{name}' argument {position} must be one of {allowed_list}, got '{value}'",
                    ),
                    span,
                ));
            }
        }
    }
}

/// If `expr` is a bare string literal, return its inner contents and
/// the span (excluding the surrounding quote characters). Returns
/// `None` for long-bracket strings (`[[...]]`) since their unescaping
/// is non-trivial and the constant-set check is a hint, not a proof.
/// Shared with the deprecated rule's constant-value check.
pub(crate) fn string_literal_value<'src>(
    expr: &Expression,
    source: &'src str,
) -> Option<(&'src str, Span)> {
    let Expression::StringLiteral(token) = expr else {
        return None;
    };
    let raw = &source[token.span.start as usize..token.span.end as usize];
    let bytes = raw.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    match bytes[0] {
        b'"' | b'\'' => {
            // Quoted literal. Strip the surrounding quotes; we don't
            // attempt to unescape - `\n` etc. won't match a constant
            // set value of `\n` either way, which is fine.
            if bytes.len() < 2 {
                return None;
            }
            let inner_start = token.span.start + 1;
            let inner_end = token.span.end - 1;
            Some((
                &source[inner_start as usize..inner_end as usize],
                Span::new(inner_start, inner_end),
            ))
        }
        _ => None,
    }
}

impl NodeRule for IncorrectStdlibUse {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt {
            StdlibChecker {
                source: ctx.source,
                semantic: ctx.semantic,
                out,
            }
            .check_call(&call_stmt.call);
        }
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::FunctionCall(call) = expr {
            StdlibChecker {
                source: ctx.source,
                semantic: ctx.semantic,
                out,
            }
            .check_call(call);
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::IncorrectStdlibUse;

    fn run(source: &str, version: LuaVersion) -> Vec<String> {
        crate::test_support::run_rule(&IncorrectStdlibUse, source, version)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn flags_too_few_args() {
        let messages = run("table.insert()", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("requires at least 2")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_too_many_args() {
        let messages = run("type(1, 2, 3)", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("at most 1")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_correct_call() {
        let messages = run("table.insert(t, 1)", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_invalid_constant_string() {
        // `collectgarbage` first arg must be one of the option names.
        let messages = run("collectgarbage('frob')", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("argument 1 must be")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_valid_constant_string() {
        let messages = run("collectgarbage('collect')", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_method_call() {
        // `obj:foo()` has no statically known receiver shape, so it
        // never resolves against stdlib paths.
        let messages = run("obj:foo()", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_every_valid_cframe_new_overload() {
        // Overload arity sweep: 0 / 1 Vector3 / 2 Vector3s / 3 / 7 / 12
        // number forms are all valid; none may false-positive.
        let source = "local v = Vector3.new(1, 2, 3)\n\
                      local a = CFrame.new()\n\
                      local b = CFrame.new(v)\n\
                      local c = CFrame.new(v, v)\n\
                      local d = CFrame.new(1, 2, 3)\n\
                      local e = CFrame.new(1, 2, 3, 4, 5, 6, 7)\n\
                      local f = CFrame.new(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12)";
        let diags = crate::test_support::run_rule_roblox(&IncorrectStdlibUse, source);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_cframe_new_between_overload_arities() {
        let diags = crate::test_support::run_rule_roblox(
            &IncorrectStdlibUse,
            "local c = CFrame.new(1, 2, 3, 4, 5)",
        );
        assert!(!diags.is_empty(), "5 args matches no CFrame.new overload");
    }

    #[test]
    fn ignores_valid_udim2_overloads() {
        let source = "local u = UDim.new(0, 10)\n\
                      local a = UDim2.new(0, 0, 1, 0)\n\
                      local b = UDim2.new(u, u)";
        let diags = crate::test_support::run_rule_roblox(&IncorrectStdlibUse, source);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_collectgarbage_param_form_in_55() {
        // The 5.5 three-arg tuning form matches only the `param`
        // overload; the plain-option overloads must not reject it.
        let messages = run("collectgarbage('param', 'pause', 100)", LuaVersion::Lua55);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_collectgarbage_incremental_tuning_in_54() {
        let messages = run(
            "collectgarbage('incremental', 100, 200, 13)",
            LuaVersion::Lua54,
        );
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_shadowed_base() {
        let messages = run("local type = f\ntype(1, 2, 3)", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_file_method_missing_args() {
        let messages = run("local f = io.open('x')\nf:setvbuf()", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("requires at least 1")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_file_method_bad_constant() {
        let messages = run("local f = io.open('x')\nf:seek('nope')", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("argument 1 must be")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_string_literal_method_missing_args() {
        let messages = run("local s = ('x'):rep()", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|m| m.contains("requires at least 1")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_method_on_unshaped_local() {
        let messages = run("local f = something()\nf:setvbuf()", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    fn run_roblox(source: &str) -> Vec<String> {
        crate::test_support::run_rule_roblox(&IncorrectStdlibUse, source)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn flags_get_service_typo() {
        let messages = run_roblox("game:GetService('Playerz')");
        assert!(
            messages
                .iter()
                .any(|m| m.contains("argument 1 must be") && m.contains("allowed values")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_valid_service_name() {
        let messages = run_roblox("game:GetService('ProximityPromptService')");
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_instance_new_unknown_class() {
        let messages = run_roblox("Instance.new('Playerz')");
        assert!(
            messages.iter().any(|m| m.contains("argument 1 must be")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_instance_new_abstract_class() {
        // BasePart is real but NotCreatable; IsA accepts it, new does not.
        let messages = run_roblox("Instance.new('BasePart')");
        assert!(
            messages.iter().any(|m| m.contains("argument 1 must be")),
            "{messages:?}"
        );
        let messages = run_roblox("local x = script:IsA('BasePart')");
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_valid_instance_new() {
        let messages = run_roblox("Instance.new('Folder')");
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_get_service_arity() {
        let messages = run_roblox("game:GetService()");
        assert!(
            messages.iter().any(|m| m.contains("requires at least 1")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_brickcolor_unknown_name() {
        let messages = run_roblox("BrickColor.new('Bright rad')");
        assert!(
            messages.iter().any(|m| m.contains("argument 1 must be")),
            "{messages:?}"
        );
        let messages = run_roblox("BrickColor.new('Bright red')");
        assert!(messages.is_empty(), "{messages:?}");
        // The 3-number form skips the constant check entirely.
        let messages = run_roblox("BrickColor.new(1, 0, 0)");
        assert!(messages.is_empty(), "{messages:?}");
    }
}
