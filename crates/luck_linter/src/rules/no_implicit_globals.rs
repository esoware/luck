use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Flags every reference (read or write) to an unresolved name except
/// for user-configured `extra_globals`. The rule deliberately FIRES on
/// stdlib names like `print` and `tostring`: the goal is a
/// "no-implicit-globals" style policy where dependencies are supplied
/// through local bindings or parameters. The explicit allowlist is
/// `extra_globals` in `LintConfig` (e.g. `vim`, `roblox`). Off by default.
pub struct NoImplicitGlobals;

impl Rule for NoImplicitGlobals {
    fn name(&self) -> &'static str {
        "no_implicit_globals"
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
        "implicit global reference, including standard-library globals"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let semantic = ctx.semantic;
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
                    self.name(),
                    format!("global variable `{}` used", reference.name),
                    reference.span,
                )
                .with_help("pass the dependency explicitly or configure extra_globals".to_string()),
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
        crate::test_support::run_rule_with_config(
            &NoImplicitGlobals,
            source,
            LuaVersion::Lua54,
            &config,
        )
    }

    #[test]
    fn flags_stdlib_global() {
        let diags = run("print(\"x\")", &[]);
        assert!(
            diags.iter().any(|d| d.message.contains("`print`")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn ignores_local_shadowing_global() {
        let diags = run("local print = function() end\nprint(\"x\")", &[]);
        assert!(
            diags.iter().all(|d| !d.message.contains("`print`")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn flags_custom_global() {
        let diags = run("myCustomGlobal()", &[]);
        assert!(
            diags.iter().any(|d| d.message.contains("`myCustomGlobal`")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn ignores_configured_extra_global() {
        let diags = run("myCustomGlobal()", &["myCustomGlobal"]);
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("`myCustomGlobal`")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn flags_stdlib_despite_configured_extra_global() {
        let diags = run("print(\"x\")\nmyCustomGlobal()", &["myCustomGlobal"]);
        assert!(
            diags.iter().any(|d| d.message.contains("`print`")),
            "{diags:?}"
        );
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("`myCustomGlobal`"))
        );
    }

    #[test]
    fn flags_write_to_global() {
        let diags = run("g = 1", &[]);
        assert!(
            diags.iter().any(|d| d.message.contains("`g`")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn ignores_discard_name() {
        let diags = run("_ = 1", &[]);
        assert!(
            diags.iter().all(|d| !d.message.contains("`_`")),
            "{diags:?}"
        );
    }
}
