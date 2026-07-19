use luck_semantic::scope::SymbolKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Luacheck 212: a function parameter is declared but never referenced.
///
/// Why this rule exists in addition to `unused_variable`: the existing
/// `unused_variable` rule already fires on `Parameter` symbols, but it
/// is a catch-all. Splitting parameter-unused out gives users a separate
/// toggle - methods often take parameters they ignore for protocol
/// reasons, and silencing the broader rule should not silence this one
/// too. Both rules firing on the same symbol is acceptable; users
/// disable whichever they don't want.
pub struct UnusedArgument;

impl Rule for UnusedArgument {
    fn name(&self) -> &'static str {
        "unused_argument"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "function parameter is declared but never referenced"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let _block = ctx.block;
        let semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut diagnostics = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.kind != SymbolKind::Parameter {
                continue;
            }
            // The implicit `self` parameter on `obj:method()` is
            // synthesized by the scope builder. Lua users have no way to
            // mark it unused, so we never flag it.
            if symbol.name == "self" {
                continue;
            }
            if symbol.name.starts_with('_') {
                continue;
            }
            if !symbol.reference_ids.is_empty() {
                continue;
            }

            let fix = Some(Fix {
                description: format!("prefix '{}' with '_'", symbol.name),
                edits: vec![TextEdit {
                    span: symbol.definition_span,
                    replacement: format!("_{}", symbol.name),
                }],
            });

            diagnostics.push(
                LintDiagnostic::new(
                    "unused_argument",
                    format!("unused argument '{}'", symbol.name),
                    symbol.definition_span,
                )
                .with_help("prefix with '_' to suppress this warning".to_string())
                .with_fix_opt(fix),
            );
        }

        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&UnusedArgument, source, LuaVersion::Lua54)
    }

    fn apply(source: &str, diag: &LintDiagnostic) -> String {
        let fix = diag.fix.as_ref().expect("fix");
        let edit = &fix.edits[0];
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..edit.span.start as usize]);
        out.push_str(&edit.replacement);
        out.push_str(&source[edit.span.end as usize..]);
        out
    }

    #[test]
    fn fix_prefixes_underscore_and_reparses() {
        let source = "local function f(x) end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "local function f(_x) end");
        let parse = luck_parser::parse(&fixed, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
    }

    #[test]
    fn flags_unused_parameter() {
        let diags = run("local function f(x) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("'x'"));
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local function f(_x) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_used_parameter() {
        let diags = run("local function f(x) return x end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_method_arg_not_self() {
        let source = "local obj = {}\nfunction obj:method(x) end";
        let diags = run(source);
        // Only `x` is flagged. `self` is the implicit method receiver.
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'x'"));
        assert!(!diags[0].message.contains("self"));
    }

    #[test]
    fn flags_multiple_parameters_some_used() {
        let diags = run("local function f(a, b, c) return a + c end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("'b'"));
    }
}
