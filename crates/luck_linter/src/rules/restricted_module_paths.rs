use luck_ast::{Expression, FunctionArgs, FunctionCall};
use luck_token::short_string_body;

use crate::call::is_global_call;
use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RestrictedModulePaths;

impl_rule!(
    RestrictedModulePaths,
    "restricted_module_paths",
    Correctness,
    Error,
    "`require` of a module listed in `restricted_module_paths`"
);

impl NodeRule for RestrictedModulePaths {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr])
    }
    fn on_call(
        &self,
        call: &FunctionCall,
        _position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        findings.extend(check_call(call, context));
    }
}

fn check_call(call: &FunctionCall, context: &LintContext<'_>) -> Option<Finding> {
    let restricted_paths = &context.config.restricted_module_paths;
    if restricted_paths.is_empty() {
        return None;
    }
    // `obj:require(...)` and a shadowed `require` are user functions, not
    // the loader.
    if !is_global_call(call, "require", context.semantic) {
        return None;
    }
    let literal = match &call.args {
        // A computed path like `require(x .. y)` can't be checked.
        FunctionArgs::Parenthesized { args, .. } => {
            let Some(Expression::StringLiteral(literal)) = args.first() else {
                return None;
            };
            literal
        }
        FunctionArgs::StringLiteral(literal) => literal,
        FunctionArgs::TableConstructor(_) => return None,
    };
    // Escapes stay as written. Long-bracket strings give `None`; module paths
    // rarely use them.
    let path = short_string_body(&literal.text)?;
    let pattern = restricted_paths.iter().find(|pattern| path_matches(path, pattern))?;
    Some(
        Finding::new(format!("`require` of restricted module `{path}`"), literal.span)
            .with_help(format!("`{pattern}` is listed in `restricted_module_paths`")),
    )
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
    use luck_core::LintConfig;
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule_with_config;

    fn run(source: &str, paths: Vec<String>) -> Vec<LintDiagnostic> {
        let config = LintConfig { restricted_module_paths: paths, ..LintConfig::default() };
        run_rule_with_config(&RestrictedModulePaths, source, LuaVersion::Lua54, &config)
    }

    #[test]
    fn flags_matching_path() {
        let diags = run("require(\"forbidden.lib\")", vec!["forbidden.lib".to_owned()]);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("forbidden.lib"), "{diags:?}");
    }

    #[test]
    fn ignores_unrelated_path() {
        let diags = run("require(\"ok.lib\")", vec!["forbidden.lib".to_owned()]);
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
        let diags = run("require\"forbidden.lib\"", vec!["forbidden.lib".to_owned()]);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_method_form() {
        let diags =
            run("local x = {}\nx:require(\"forbidden.lib\")", vec!["forbidden.lib".to_owned()]);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_nonliteral_argument() {
        // Can't statically resolve a variable's value; skip.
        let diags =
            run("local p = \"forbidden.lib\"\nrequire(p)", vec!["forbidden.lib".to_owned()]);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_wildcard_module_and_submodules() {
        for source in ["require(\"forbidden\")", "require(\"forbidden.sub\")"] {
            let diags = run(source, vec!["forbidden.*".to_owned()]);
            assert_eq!(diags.len(), 1, "{diags:?}");
        }
    }

    #[test]
    fn ignores_name_extending_wildcard_prefix() {
        let diags = run("require(\"forbiddenly\")", vec!["forbidden.*".to_owned()]);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
