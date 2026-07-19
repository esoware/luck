use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Flags every reference (read or write) to an unresolved name except
/// for user-configured `extra_globals`. The rule deliberately FIRES on
/// stdlib names like `print` and `tostring`: the goal is a
/// "no-implicit-globals" style policy where the file declares each
/// external name as a `local` at module top. The only escape hatch is
/// `extra_globals` in `LintConfig` (e.g. `vim`, `roblox`). Off by default.
pub struct GlobalUsage;

impl Rule for GlobalUsage {
    fn name(&self) -> &'static str {
        "global_usage"
    }
    fn category(&self) -> Category {
        // The diagnostic crate has no `Complexity` variant. This is a
        // codebase-wide stylistic rule, so `Style` is the right slot.
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "reference to a global variable; prefer an explicit local"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let _block = ctx.block;
        let semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut diagnostics = Vec::new();

        for reference in semantic.scope_tree.unresolved_references() {
            // Discard slot: never a real read.
            if reference.name == "_" {
                continue;
            }
            // User-configured extras are the explicit escape hatch.
            // Stdlib globals still fire - that's the point of the rule.
            if semantic.extra_globals.contains(reference.name.as_str()) {
                continue;
            }

            diagnostics.push(
                LintDiagnostic::new(
                    "global_usage",
                    format!("global variable '{}' used", reference.name),
                    reference.span,
                )
                .with_help("introduce a `local` alias at module top".to_string()),
            );
        }
        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    /// Run with the same `extra_globals` plumbing the driver uses: a
    /// user-defined name is inserted into `SemanticAnalysis::extra_globals`.
    /// The rule uses that set as its escape hatch and otherwise fires
    /// on every unresolved reference (stdlib names included).
    fn run(source: &str, extras: &[&str]) -> Vec<LintDiagnostic> {
        let config = crate::LintConfig {
            extra_globals: extras.iter().map(|name| name.to_string()).collect(),
            ..crate::LintConfig::default()
        };
        crate::test_support::run_rule_with_config(&GlobalUsage, source, LuaVersion::Lua54, &config)
    }

    #[test]
    fn flags_stdlib_global() {
        let diags = run("print(\"x\")", &[]);
        assert!(
            diags.iter().any(|d| d.message.contains("'print'")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn ignores_local_shadowing_global() {
        let diags = run("local print = function() end\nprint(\"x\")", &[]);
        assert!(
            diags.iter().all(|d| !d.message.contains("'print'")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn flags_custom_global() {
        let diags = run("myCustomGlobal()", &[]);
        assert!(
            diags.iter().any(|d| d.message.contains("'myCustomGlobal'")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn extras_silence_specific_name() {
        let diags = run("myCustomGlobal()", &["myCustomGlobal"]);
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("'myCustomGlobal'")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn extras_do_not_silence_other_names() {
        let diags = run("print(\"x\")\nmyCustomGlobal()", &["myCustomGlobal"]);
        assert!(diags.iter().any(|d| d.message.contains("'print'")));
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("'myCustomGlobal'"))
        );
    }

    #[test]
    fn flags_write_to_global() {
        let diags = run("g = 1", &[]);
        assert!(
            diags.iter().any(|d| d.message.contains("'g'")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn ignores_discard_name() {
        let diags = run("_ = 1", &[]);
        assert!(diags.iter().all(|d| !d.message.contains("'_'")));
    }
}
