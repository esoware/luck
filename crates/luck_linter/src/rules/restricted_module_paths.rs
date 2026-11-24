use crate::config::concat;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::rule::{Rule, RuleOptions, declare_lint_rule};

/// Checks each `require` site semantic analysis finds by its module path.
///
/// The path's value counts, however it's spelled. A computed path like
/// `require(x .. y)` can't be checked, and a call of a `require` the chunk
/// shadows or rebinds loads nothing the chunk can know of.
#[derive(Debug, Clone, Default)]
pub(crate) struct RestrictedModulePaths {
    paths: Box<[String]>,
}

declare_lint_rule!(
    RestrictedModulePaths,
    Correctness,
    Error,
    "`require` of a module listed in `restricted_module_paths`",
    options = RestrictedModulePathsOptions
);

/// Options of the `restricted_module_paths` rule, which reports nothing
/// until they list `paths`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub(crate) struct RestrictedModulePathsOptions {
    /// Modules that can't be required. A trailing `.*` also covers
    /// submodules: `forbidden.*` matches `forbidden` and `forbidden.sub`.
    paths: Vec<String>,
}

impl RuleOptions for RestrictedModulePathsOptions {
    fn merge_onto(self, base: Self) -> Self {
        Self { paths: concat(self.paths, base.paths) }
    }
}

impl From<RestrictedModulePathsOptions> for RestrictedModulePaths {
    fn from(options: RestrictedModulePathsOptions) -> Self {
        Self { paths: options.paths.into() }
    }
}

impl Rule for RestrictedModulePaths {
    fn should_run(&self, _host: &ContextHost<'_>) -> bool {
        !self.paths.is_empty()
    }

    fn run_once(&self, context: &LintContext<'_>) {
        for literal in context.semantic.semantic().requires().iter().filter_map(|site| site.path) {
            let Some(path) = literal.as_str() else {
                continue;
            };
            if let Some(pattern) = self.paths.iter().find(|pattern| path_matches(path, pattern)) {
                context.diagnostic(
                    Finding::new(format!("`require` of restricted module `{path}`"), literal.span)
                        .with_help(format!("`{pattern}` is listed in `restricted_module_paths`")),
                );
            }
        }
    }
}

/// Exact match, except that a pattern ending in `.*` matches the module and
/// every submodule: `forbidden.*` covers `forbidden` and `forbidden.sub`.
fn path_matches(path: &str, pattern: &str) -> bool {
    let Some(prefix) = pattern.strip_suffix(".*") else {
        return path == pattern;
    };
    path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::config::LintConfig;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule_with_config;
    use crate::rules::RuleId;

    fn run(source: &str, paths: &[&str]) -> Vec<Message> {
        let lint = serde_json::json!({
            "rules": { "restricted_module_paths": ["error", { "paths": paths }] }
        });
        let config: LintConfig = serde_json::from_value(lint).expect("a valid `lint` section");
        run_rule_with_config(RuleId::RestrictedModulePaths, source, LuaVersion::Lua54, &config)
    }

    #[test]
    fn flags_matching_path() {
        let diags = run("require(\"forbidden.lib\")", &["forbidden.lib"]);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("forbidden.lib"), "{diags:?}");
    }

    #[test]
    fn ignores_unrelated_path() {
        let diags = run("require(\"ok.lib\")", &["forbidden.lib"]);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_empty_config() {
        let diags = run("require(\"anything\")", &[]);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_bare_quoted_form() {
        // `require"x"` parses as a single-string-arg call (no parens).
        let diags = run("require\"forbidden.lib\"", &["forbidden.lib"]);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_a_require_through_the_global_table() {
        for source in ["_G.require(\"forbidden.lib\")", "_ENV[\"require\"](\"forbidden.lib\")"] {
            let diags = run(source, &["forbidden.lib"]);
            assert_eq!(diags.len(), 1, "{source}: {diags:?}");
        }
    }

    #[test]
    fn ignores_method_form() {
        let diags = run(
            "\
local x = {}
x:require(\"forbidden.lib\")",
            &["forbidden.lib"],
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_nonliteral_argument() {
        // Can't statically resolve a variable's value; skip.
        let diags = run(
            "\
local p = \"forbidden.lib\"
require(p)",
            &["forbidden.lib"],
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_wildcard_module_and_submodules() {
        for source in ["require(\"forbidden\")", "require(\"forbidden.sub\")"] {
            let diags = run(source, &["forbidden.*"]);
            assert_eq!(diags.len(), 1, "{diags:?}");
        }
    }

    #[test]
    fn ignores_name_extending_wildcard_prefix() {
        let diags = run("require(\"forbiddenly\")", &["forbidden.*"]);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_a_path_spelled_with_escapes_or_long_brackets() {
        for source in
            [r#"require("forbidden\46sub")"#, "require[[forbidden]]", "require'\\x66orbidden'"]
        {
            let diags = run(source, &["forbidden.*"]);
            assert_eq!(diags.len(), 1, "{source}: {diags:?}");
        }
    }
}
