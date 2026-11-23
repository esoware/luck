use luck_diagnostics::Tag;
use luck_semantic::SymbolKind;

use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{LintContext, WholeRule, impl_rule};
use crate::underscore_fix::prefix_underscore;

/// The `_` prefix is a suggestion, because it silences the warning rather
/// than answering it, and an unused variable is often a misspelled use.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnusedVariable;

impl_rule!(
    UnusedVariable,
    "unused_variable",
    Correctness,
    Warning,
    "variable is declared but never read",
    Suggestion
);

impl WholeRule for UnusedVariable {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let tree = semantic.scope_tree();
        let mut findings = Vec::new();

        for symbol in tree.symbols() {
            let name = tree.name(symbol.name);
            if name.starts_with('_') {
                continue;
            }

            // Luau: an exported binding is read through the module's export
            // table, and renaming it would change the module's API. A
            // `<close>` local is used by its scope exit.
            if symbol.is_exported || symbol.is_close {
                continue;
            }

            // `unused_argument` and `unused_loop_variable` cover parameters
            // and loop variables.
            let kind_name = match symbol.kind {
                SymbolKind::Local => "variable",
                SymbolKind::FunctionName => "function",
                SymbolKind::Parameter
                | SymbolKind::ImplicitSelf
                | SymbolKind::IteratorVariable
                | SymbolKind::NumericForVariable => continue,
            };

            let has_read = tree
                .resolved_references(symbol.id)
                .iter()
                .any(|&reference_id| tree.reference(reference_id).kind.is_read());

            if has_read {
                continue;
            }

            findings.push(
                Finding::new(format!("unused {kind_name} `{name}`"), symbol.definition_span)
                    .with_tag(Tag::Unnecessary)
                    .with_help("prefix with `_` to suppress this warning")
                    .with_fix(context, FixKind::Suggestion, |fixer| {
                        prefix_underscore(fixer, semantic, symbol)
                    }),
            );
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::{applied, fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<Message> {
        run_rule(&UnusedVariable, source, LuaVersion::Luau)
    }

    #[test]
    fn ignores_exported_bindings() {
        let diags = run("\
export local answer = 42
export const LIMIT = 1i
export function helper()
return 1
end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_plain_local_next_to_exports() {
        let diags = run("\
export local answer = 42
local dead = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`dead`"));
        assert_eq!(diags[0].tag, Some(Tag::Unnecessary), "an editor fades it");
    }

    #[test]
    fn ignores_variable_read_by_typeof() {
        // typeof(expr) inside a type position references the runtime
        // binding; it must count as a use.
        let diags = run("\
local n = 1
type T = typeof(n)
export type U = T");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_variable_read_by_typeof_in_cast() {
        let diags = run("\
local n = 1
local m = (nil :: typeof(n))
print(m)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_variable_unused_despite_type_alias() {
        let diags = run("\
local n = 1
type T = number
export type U = T");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_without_fix_that_captures_existing_underscore_name() {
        let source = "\
local _x = 1
local x = 2
print(_x)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            applied(source, &diags),
            source,
            "the rename shouldn't make `print(_x)` read the second local"
        );
    }

    #[test]
    fn ignores_close_variable() {
        let source = "\
local lock <close> = acquire()
print(1)";
        let diags = run_rule(&UnusedVariable, source, LuaVersion::Lua54);
        assert!(diags.is_empty(), "a `<close>` local is used by its scope exit: {diags:?}");
    }

    #[test]
    fn flags_without_fix_that_would_bind_env() {
        let diags = run_rule(&UnusedVariable, "local ENV = 1", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "a local `_ENV` redirects every global: {diags:?}");
    }

    #[test]
    fn flags_with_suggestion_when_prefixed_name_is_free() {
        let source = "\
local x = 2
print(_y)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].fix.as_ref().map(|fix| fix.kind), Some(FixKind::Suggestion));
        assert_eq!(
            fix_and_reparse(source, &diags, LuaVersion::Luau),
            "\
local _x = 2
print(_y)"
        );
    }
}
