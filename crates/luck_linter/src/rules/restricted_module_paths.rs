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
    /// Modules that can't be required, each a require path as written, such
    /// as `@pkg/internal` or `./legacy`. A trailing `/*` also covers every
    /// path under one: `@pkg/*` matches `@pkg` and `@pkg/sub/x`. An alias
    /// matches in any case.
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

/// Whether `path` is `pattern`, component by component, or a path under it
/// when `pattern` ends in `/*`: `@pkg/*` covers `@pkg` and `@pkg/sub/x`.
///
/// Luau reads a `\` in a require path as a `/`, and an alias's name in any
/// ASCII case.
fn path_matches(path: &str, pattern: &str) -> bool {
    let (pattern, covers_subpaths) =
        pattern.strip_suffix("/*").map_or((pattern, false), |prefix| (prefix, true));
    let mut components = path.split(['/', '\\']);
    let mut is_alias = path.starts_with('@');
    for expected in pattern.split(['/', '\\']) {
        let Some(component) = components.next() else {
            return false;
        };
        let is_same =
            if is_alias { component.eq_ignore_ascii_case(expected) } else { component == expected };
        if !is_same {
            return false;
        }
        is_alias = false;
    }
    covers_subpaths || components.next().is_none()
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, Option<&str>)] = &[
        (r#"require("./ok/lib")"#, Some(r#"{"paths": ["./forbidden/lib"]}"#)),
        (r#"require("@anything")"#, Some(r#"{"paths": []}"#)),
        (
            "\
local x = {}
x:require(\"./forbidden/lib\")",
            Some(r#"{"paths": ["./forbidden/lib"]}"#),
        ),
        // A variable's value can't be resolved statically.
        (
            "\
local p = \"./forbidden/lib\"
require(p)",
            Some(r#"{"paths": ["./forbidden/lib"]}"#),
        ),
        (r#"require("@forbiddenly")"#, Some(r#"{"paths": ["@forbidden/*"]}"#)),
        // Without `/*`, a pattern covers no path under it.
        (r#"require("./forbidden/lib")"#, Some(r#"{"paths": ["./forbidden"]}"#)),
        // Only an alias matches in any case.
        (r#"require("./Forbidden")"#, Some(r#"{"paths": ["./forbidden"]}"#)),
    ];

    const FAIL: &[(&str, Option<&str>)] = &[
        (r#"require("./forbidden/lib")"#, Some(r#"{"paths": ["./forbidden/lib"]}"#)),
        // `require"x"` is a call with one string argument and no parentheses.
        (r#"require"./forbidden/lib""#, Some(r#"{"paths": ["./forbidden/lib"]}"#)),
        (r#"_G.require("./forbidden/lib")"#, Some(r#"{"paths": ["./forbidden/lib"]}"#)),
        (r#"_G["require"]("./forbidden/lib")"#, Some(r#"{"paths": ["./forbidden/lib"]}"#)),
        (r#"require("@forbidden")"#, Some(r#"{"paths": ["@forbidden/*"]}"#)),
        (r#"require("@forbidden/sub/x")"#, Some(r#"{"paths": ["@forbidden/*"]}"#)),
        (r#"require("@Forbidden/sub")"#, Some(r#"{"paths": ["@forbidden/*"]}"#)),
        (r#"require("@forbidden\\sub")"#, Some(r#"{"paths": ["@forbidden/sub"]}"#)),
        (r#"require("@forbidden\47sub")"#, Some(r#"{"paths": ["@forbidden/*"]}"#)),
        ("require[[@forbidden]]", Some(r#"{"paths": ["@forbidden/*"]}"#)),
        (r"require'\x40forbidden'", Some(r#"{"paths": ["@forbidden/*"]}"#)),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RestrictedModulePaths, PASS, FAIL).test_and_snapshot();
    }
}
