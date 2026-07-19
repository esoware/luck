use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall, Var};
use luck_semantic::SemanticAnalysis;
use luck_token::{Span, TokenKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct RestrictedModulePaths;

impl Rule for RestrictedModulePaths {
    fn name(&self) -> &'static str {
        "restricted_module_paths"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "require() of a path on the project's restricted list"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

struct RestrictedChecker<'src, 'out> {
    source: &'src str,
    semantic: &'src SemanticAnalysis,
    paths: &'src [String],
    out: &'out mut Vec<LintDiagnostic>,
}

impl<'src> RestrictedChecker<'src, '_> {
    fn check_call(&mut self, call: &FunctionCall) {
        // Only flag direct `require(...)` calls. Method-form
        // `obj:require(...)` could be any user method.
        if call.method.is_some() {
            return;
        }
        let Expression::Var(var) = &call.callee else {
            return;
        };
        let Var::Name(token) = var.as_ref() else {
            return;
        };
        let TokenKind::Identifier(name) = &token.kind else {
            return;
        };
        if name.as_str() != "require" {
            return;
        }
        // Shadowed `require` is a user function, not the loader.
        if self.semantic.resolves_to_local(name.as_str(), token.span) {
            return;
        }

        // Extract the literal string argument. The two shapes we accept
        // are `require("path")` and `require"path"`.
        let (literal_span, literal_text) = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let mut iter = args.iter();
                let Some(first) = iter.next() else {
                    return;
                };
                // Only a single literal arg is meaningful for path
                // matching; bail on `require(x .. y)` and similar.
                let Expression::StringLiteral(tok) = first else {
                    return;
                };
                let Some(body) = self.string_body(tok.span) else {
                    return;
                };
                (tok.span, body)
            }
            FunctionArgs::StringLiteral(tok) => {
                let Some(body) = self.string_body(tok.span) else {
                    return;
                };
                (tok.span, body)
            }
            FunctionArgs::TableConstructor(_) => return,
        };

        for restricted in self.paths {
            if path_matches(&literal_text, restricted) {
                self.out.push(
                    LintDiagnostic::new(
                        "restricted_module_paths",
                        format!("require of restricted module '{literal_text}'"),
                        literal_span,
                    )
                    .with_help(format!(
                        "'{restricted}' is on the project's restricted list"
                    )),
                );
                return;
            }
        }
    }

    /// Pull the body out of a short-string literal token. Long-bracket
    /// strings return None - they're unusual for module paths and
    /// would need escape-aware decoding.
    fn string_body(&self, span: Span) -> Option<String> {
        let slice = &self.source[span.start as usize..span.end as usize];
        let bytes = slice.as_bytes();
        let first = *bytes.first()?;
        if first != b'"' && first != b'\'' {
            return None;
        }
        if bytes.len() < 2 || *bytes.last()? != first {
            return None;
        }
        Some(slice[1..slice.len() - 1].to_string())
    }
}

fn checker<'src, 'out>(
    ctx: &'src LintContext<'src>,
    out: &'out mut Vec<LintDiagnostic>,
) -> Option<RestrictedChecker<'src, 'out>> {
    let paths = &ctx.config.restricted_module_paths;
    if paths.is_empty() {
        return None;
    }
    Some(RestrictedChecker {
        source: ctx.source,
        semantic: ctx.semantic,
        paths,
        out,
    })
}

impl NodeRule for RestrictedModulePaths {
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
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt
            && let Some(mut checker) = checker(ctx, out)
        {
            checker.check_call(&call_stmt.call);
        }
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::FunctionCall(call) = expr
            && let Some(mut checker) = checker(ctx, out)
        {
            checker.check_call(call);
        }
    }
}

/// Whether `path` should be considered a match for the restricted
/// `pattern`. Currently exact string equality; a trailing `.*` is
/// honored as a prefix match so `forbidden.*` matches every submodule.
fn path_matches(path: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix(".*") {
        path == prefix || path.starts_with(&format!("{prefix}."))
    } else {
        path == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str, paths: Vec<String>) -> Vec<LintDiagnostic> {
        let config = crate::LintConfig {
            restricted_module_paths: paths,
            ..crate::LintConfig::default()
        };
        crate::test_support::run_rule_with_config(
            &RestrictedModulePaths,
            source,
            LuaVersion::Lua54,
            &config,
        )
    }

    #[test]
    fn flags_matching_path() {
        let diags = run(
            "require(\"forbidden.lib\")",
            vec!["forbidden.lib".to_string()],
        );
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0].message.contains("forbidden.lib"),
            "msg: {}",
            diags[0].message
        );
    }

    #[test]
    fn ignores_unrelated_path() {
        let diags = run("require(\"ok.lib\")", vec!["forbidden.lib".to_string()]);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_empty_config() {
        let diags = run("require(\"anything\")", Vec::new());
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_bare_quoted_form() {
        // `require"x"` parses as a single-string-arg call (no parens).
        let diags = run(
            "require\"forbidden.lib\"",
            vec!["forbidden.lib".to_string()],
        );
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_method_form() {
        let diags = run(
            "local x = {}\nx:require(\"forbidden.lib\")",
            vec!["forbidden.lib".to_string()],
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_nonliteral_argument() {
        // Can't statically resolve a variable's value; skip.
        let diags = run(
            "local p = \"forbidden.lib\"\nrequire(p)",
            vec!["forbidden.lib".to_string()],
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn wildcard_suffix_matches_submodules() {
        // `forbidden.*` matches both `forbidden` and `forbidden.sub`.
        let paths = vec!["forbidden.*".to_string()];
        assert_eq!(run("require(\"forbidden\")", paths.clone()).len(), 1);
        assert_eq!(run("require(\"forbidden.sub\")", paths.clone()).len(), 1);
        assert!(run("require(\"forbiddenly\")", paths.clone()).is_empty());
    }
}
