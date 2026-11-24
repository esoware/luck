use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags every read or write of a global, stdlib names like `print`
/// included, unless it's listed in `extra_globals`.
///
/// This enforces a policy where every dependency arrives through a local or a
/// parameter. Off by default, like every `Style` rule.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NoImplicitGlobals;

declare_lint_rule!(
    NoImplicitGlobals,
    Style,
    Warning,
    "implicit global reference, including standard-library globals"
);

impl Rule for NoImplicitGlobals {
    fn run_once(&self, context: &LintContext<'_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();

        for reference in tree.global_references() {
            let name = tree.name(reference.name);
            // `_` is a conventional discard name, not a real global.
            if name == "_" {
                continue;
            }
            if semantic.is_extra_global(reference.name) {
                continue;
            }

            context.diagnostic(
                Finding::new(format!("global variable `{name}` used"), reference.span)
                    .with_help("pass it in explicitly, or add it to `extra_globals`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::config::LintConfig;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule_with_config;
    use crate::rules::RuleId;

    fn run(source: &str, extra_globals: &[&str]) -> Vec<Message> {
        let config = LintConfig {
            extra_globals: extra_globals.iter().map(ToString::to_string).collect(),
            ..LintConfig::default()
        };
        run_rule_with_config(RuleId::NoImplicitGlobals, source, LuaVersion::Lua54, &config)
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
