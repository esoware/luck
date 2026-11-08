use crate::diagnostic::{Category, Finding, Severity};
use crate::rule::{LintContext, Rule, WholeRule};

/// Flags every read or write of a global, stdlib names like `print`
/// included, unless it's listed in `extra_globals`.
///
/// This enforces a policy where every dependency arrives through a local or a
/// parameter. Off by default, like every `Style` rule.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoImplicitGlobals;

impl Rule for NoImplicitGlobals {
    fn name(&self) -> &'static str {
        "no_implicit_globals"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "implicit global reference, including standard-library globals"
    }
}

impl WholeRule for NoImplicitGlobals {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let mut findings = Vec::new();

        for reference in semantic.scope_tree.unresolved_references() {
            // `_` is a conventional discard name, not a real global.
            if reference.name == "_" {
                continue;
            }
            if semantic.extra_globals.contains(reference.name.as_str()) {
                continue;
            }

            findings.push(
                Finding::new(format!("global variable `{}` used", reference.name), reference.span)
                    .with_help("pass it in explicitly, or add it to `extra_globals`"),
            );
        }
        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_core::LintConfig;
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule_with_config;

    fn run(source: &str, extra_globals: &[&str]) -> Vec<LintDiagnostic> {
        let config = LintConfig {
            extra_globals: extra_globals.iter().map(ToString::to_string).collect(),
            ..LintConfig::default()
        };
        run_rule_with_config(&NoImplicitGlobals, source, LuaVersion::Lua54, &config)
    }

    #[test]
    fn flags_stdlib_global() {
        let diags = run("print(\"x\")", &[]);
        assert!(diags.iter().any(|diag| diag.message.contains("`print`")), "got: {diags:?}");
    }

    #[test]
    fn ignores_local_shadowing_global() {
        let diags = run("local print = function() end\nprint(\"x\")", &[]);
        assert!(diags.iter().all(|diag| !diag.message.contains("`print`")), "got: {diags:?}");
    }

    #[test]
    fn flags_custom_global() {
        let diags = run("myCustomGlobal()", &[]);
        assert!(
            diags.iter().any(|diag| diag.message.contains("`myCustomGlobal`")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn ignores_configured_extra_global() {
        let diags = run("myCustomGlobal()", &["myCustomGlobal"]);
        assert!(
            diags.iter().all(|diag| !diag.message.contains("`myCustomGlobal`")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn flags_stdlib_despite_configured_extra_global() {
        let diags = run("print(\"x\")\nmyCustomGlobal()", &["myCustomGlobal"]);
        assert!(diags.iter().any(|diag| diag.message.contains("`print`")), "{diags:?}");
        assert!(diags.iter().all(|diag| !diag.message.contains("`myCustomGlobal`")));
    }

    #[test]
    fn flags_write_to_global() {
        let diags = run("g = 1", &[]);
        assert!(diags.iter().any(|diag| diag.message.contains("`g`")), "got: {diags:?}");
    }

    #[test]
    fn ignores_discard_name() {
        let diags = run("_ = 1", &[]);
        assert!(diags.iter().all(|diag| !diag.message.contains("`_`")), "{diags:?}");
    }
}
