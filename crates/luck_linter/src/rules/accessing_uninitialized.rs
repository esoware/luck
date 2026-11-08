use crate::diagnostic::{Category, Finding, Severity};
use crate::rule::{LintContext, Rule, WholeRule};
use crate::uninitialized::{UninitializedLocals, first_reference};

/// Luacheck 321: a local declared without a value is read before anything
/// is assigned to it.
///
/// Only a bare `local x` counts. `local x = nil` is initialized, and
/// `redundant_nil_init` covers it instead.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AccessingUninitialized;

impl Rule for AccessingUninitialized {
    fn name(&self) -> &'static str {
        "accessing_uninitialized"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "local is read before any value is assigned to it"
    }
}

impl WholeRule for AccessingUninitialized {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let tree = &context.semantic.scope_tree;
        let uninitialized_locals = UninitializedLocals::collect(context.block);
        tree.symbols
            .iter()
            .filter(|symbol| uninitialized_locals.contains(symbol) && !symbol.name.starts_with('_'))
            .filter_map(|symbol| {
                let first = first_reference(symbol, tree)?;
                first.kind.is_read().then(|| {
                    Finding::new(
                        format!(
                            "`{}` is read before any value is assigned, so it is `nil`",
                            symbol.name
                        ),
                        first.span,
                    )
                    .with_help("initialize at declaration or assign before reading")
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&AccessingUninitialized, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_read_before_assign() {
        let diags = run("local x\nprint(x)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_explicit_nil_init() {
        // `redundant_nil_init` may flag this, but it counts as initialized.
        let diags = run("local x = nil\nprint(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_write_then_read() {
        let diags = run("local x\nx = 1\nprint(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_write_in_branch_before_read() {
        let diags = run("local x\nif c then x = 1 end\nprint(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x\nprint(_x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
