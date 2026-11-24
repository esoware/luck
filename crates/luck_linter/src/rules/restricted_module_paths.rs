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

    fn run_once(&self, context: &LintContext<'_, '_>) {
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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget, Option<&str>)] = &[
        (r#"require("ok.lib")"#, LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.lib"]}"#)),
        (r#"require("anything")"#, LuaTarget::Lua54, Some(r#"{"paths": []}"#)),
        (
            "\
local x = {}
x:require(\"forbidden.lib\")",
            LuaTarget::Lua54,
            Some(r#"{"paths": ["forbidden.lib"]}"#),
        ),
        // A variable's value can't be resolved statically.
        (
            "\
local p = \"forbidden.lib\"
require(p)",
            LuaTarget::Lua54,
            Some(r#"{"paths": ["forbidden.lib"]}"#),
        ),
        (r#"require("forbiddenly")"#, LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.*"]}"#)),
    ];

    const FAIL: &[(&str, LuaTarget, Option<&str>)] = &[
        (r#"require("forbidden.lib")"#, LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.lib"]}"#)),
        // `require"x"` is a call with one string argument and no parentheses.
        (r#"require"forbidden.lib""#, LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.lib"]}"#)),
        (
            r#"_G.require("forbidden.lib")"#,
            LuaTarget::Lua54,
            Some(r#"{"paths": ["forbidden.lib"]}"#),
        ),
        (
            r#"_ENV["require"]("forbidden.lib")"#,
            LuaTarget::Lua54,
            Some(r#"{"paths": ["forbidden.lib"]}"#),
        ),
        (r#"require("forbidden")"#, LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.*"]}"#)),
        (r#"require("forbidden.sub")"#, LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.*"]}"#)),
        (r#"require("forbidden\46sub")"#, LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.*"]}"#)),
        ("require[[forbidden]]", LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.*"]}"#)),
        (r"require'\x66orbidden'", LuaTarget::Lua54, Some(r#"{"paths": ["forbidden.*"]}"#)),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RestrictedModulePaths, PASS, FAIL).test_and_snapshot();
    }
}
