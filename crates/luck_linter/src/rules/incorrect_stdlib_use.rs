use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall};
use luck_semantic::SemanticAnalysis;
use luck_semantic::stdlib_model::{EntryKind, StdlibArgKind, StdlibEntry, StdlibFunction};
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

impl<'src> StdlibChecker<'src, '_> {
    fn check_call(&mut self, call: &FunctionCall) {
        // Method calls don't resolve to stdlib paths.
        if call.method.is_some() {
            return;
        }
        let Some((name, func)) = self.resolve_call(call) else {
            return;
        };

        // Vararg passthrough (`f(other_call(...))`) can hide any number
        // of arguments, so we conservatively skip arity checks.
        let positional_args = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => args.iter().collect(),
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => Vec::new(),
        };

        let arg_count = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => args.len(),
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => 1,
        };

        // The comment above is only true if we actually check: a call or
        // `...` in tail position expands to any number of values, so the
        // static count is a lower bound only - min-arity can't fire.
        let has_multi_value_tail = positional_args.last().is_some_and(|last| {
            matches!(last, Expression::FunctionCall(_) | Expression::VarArg(_))
        });

        if arg_count < func.min_args && !has_multi_value_tail {
            self.out.push(LintDiagnostic::new(
                "incorrect_stdlib_use",
                format!(
                    "'{name}' requires at least {} argument(s), got {arg_count}",
                    func.min_args
                ),
                call.span,
            ));
            return;
        }
        if let Some(max) = func.max_args
            && arg_count > max
        {
            self.out.push(LintDiagnostic::new(
                "incorrect_stdlib_use",
                format!("'{name}' accepts at most {max} argument(s), got {arg_count}"),
                call.span,
            ));
            return;
        }

        // Constant-typed parameter check. Only applies to a literal
        // string argument at a fixed parameter position - we can't
        // statically resolve dynamic values.
        for (idx, expr) in positional_args.iter().enumerate() {
            let Some(param) = func.params.get(idx) else {
                break;
            };
            let StdlibArgKind::Constant(allowed) = &param.kind else {
                continue;
            };
            let Some((value, span)) = string_literal_value(expr, self.source) else {
                continue;
            };
            if !allowed.iter().any(|allowed_value| allowed_value == value) {
                let allowed_list = allowed
                    .iter()
                    .map(|allowed_value| format!("'{allowed_value}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
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

    fn resolve_call(&self, call: &FunctionCall) -> Option<(String, &'static StdlibFunction)> {
        let (path, display_name) = match &call.callee {
            Expression::Var(var) => match var {
                luck_ast::expr::Var::Name(token) => {
                    let name = self.slice(token.span);
                    // Shadowed base (`local error = ...`) is not the stdlib.
                    if self.semantic.resolves_to_local(name, token.span) {
                        return None;
                    }
                    (vec![name], name.to_string())
                }
                luck_ast::expr::Var::FieldAccess(fa) => {
                    let Expression::Var(prefix_var) = &fa.prefix else {
                        return None;
                    };
                    let luck_ast::expr::Var::Name(prefix_token) = prefix_var else {
                        return None;
                    };
                    let prefix = self.slice(prefix_token.span);
                    // Shadowed base (`local table = {}`) is not the stdlib.
                    if self.semantic.resolves_to_local(prefix, prefix_token.span) {
                        return None;
                    }
                    let field = self.slice(fa.name.span);
                    (vec![prefix, field], format!("{prefix}.{field}"))
                }
                _ => return None,
            },
            _ => return None,
        };
        let entry: &StdlibEntry = self.semantic.lookup_stdlib_str(&path)?;
        if let EntryKind::Function(func) = &entry.kind {
            Some((display_name, func.as_ref()))
        } else {
            None
        }
    }

    fn slice(&self, span: Span) -> &'src str {
        &self.source[span.start as usize..span.end as usize]
    }
}

/// If `expr` is a bare string literal, return its inner contents and
/// the span (excluding the surrounding quote characters). Returns
/// `None` for long-bracket strings (`[[...]]`) since their unescaping
/// is non-trivial and the constant-set check is a hint, not a proof.
fn string_literal_value<'src>(expr: &Expression, source: &'src str) -> Option<(&'src str, Span)> {
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
        // `obj:foo()` should never resolve against stdlib paths.
        let messages = run("obj:foo()", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }
}
