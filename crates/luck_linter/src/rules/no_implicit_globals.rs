use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags every read or write of a global, stdlib names like `print`
/// included, unless it's listed in `extra_globals`.
///
/// This enforces a policy where every dependency arrives through a local or a
/// parameter. Off by default, like every `Style` rule.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoImplicitGlobals;

impl_rule!(
    NoImplicitGlobals,
    "no_implicit_globals",
    Style,
    Warning,
    "implicit global reference, including standard-library globals"
);

impl WholeRule for NoImplicitGlobals {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let tree = semantic.scoping();
        let mut findings = Vec::new();

        for reference in tree.global_references() {
            let name = tree.name(reference.name);
            // `_` is a conventional discard name, not a real global.
            if name == "_" {
                continue;
            }
            if semantic.is_extra_global(reference.name) {
                continue;
            }

            findings.push(
                Finding::new(format!("global variable `{name}` used"), reference.span)
                    .with_help("pass it in explicitly, or add it to `extra_globals`"),
            );
        }
        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::config::LintConfig;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule_with_config;

    fn run(source: &str, extra_globals: &[&str]) -> Vec<Message> {
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
        let diags = run(
            "\
local print = function() end
print(\"x\")",
            &[],
        );
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
        let diags = run(
            "\
print(\"x\")
myCustomGlobal()",
            &["myCustomGlobal"],
        );
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
