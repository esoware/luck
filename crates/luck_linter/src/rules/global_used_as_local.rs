use luck_semantic::{Reference, ScopeId, ScopeKind, Scoping};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GlobalUsedAsLocal;

declare_lint_rule!(
    GlobalUsedAsLocal,
    Style,
    Warning,
    "global variable is only used inside one function; use a local"
);

impl Rule for GlobalUsedAsLocal {
    fn run_once(&self, context: &LintContext<'_>) {
        let tree = context.scoping();
        let mut global_references: Vec<&Reference> = tree.global_references().collect();
        global_references.sort_by_key(|reference| reference.name);

        let mut firsts: Vec<&Reference> = Vec::new();
        for references in global_references.chunk_by(|left, right| left.name == right.name) {
            let Some(&first) = references.iter().min_by_key(|reference| reference.span.start)
            else {
                continue;
            };
            if tree.name(first.name) == "_" || context.semantic.is_known_global_name(first.name) {
                continue;
            }
            let is_assigned = references.iter().any(|reference| reference.kind.is_write());
            if !is_assigned {
                continue;
            }
            // Every reference sits inside a function, and all in the same one.
            let mut functions =
                references.iter().map(|reference| enclosing_function(tree, reference.scope));
            let Some(Some(function)) = functions.next() else {
                continue;
            };
            if !functions.all(|other| other == Some(function)) {
                continue;
            }
            firsts.push(first);
        }
        firsts.sort_by_key(|first| first.span.start);
        for first in firsts {
            context.diagnostic(
                Finding::new(
                    format!("global `{}` is only used inside one function", tree.name(first.name)),
                    first.span,
                )
                .with_help("declare it `local` inside that function"),
            );
        }
    }
}

fn enclosing_function(tree: &Scoping, scope: ScopeId) -> Option<ScopeId> {
    let function = tree.scope(scope).function;
    (tree.scope(function).kind == ScopeKind::Function).then_some(function)
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local function f()
    shared_state = 1
end
local function g()
    return shared_state
end
f()
g()",
        "\
local function f()
    counter = 1
end
f()
print(counter)",
        // Top-level globals may be the module's exports.
        "\
counter = 0
counter = counter + 1",
        "\
local function f()
    return config
end
f()",
        "\
local function f()
    print(1)
end
f()",
        // `g` is a separate function, so `state` is used in two.
        "\
local function f()
    state = 1
    local function g()
        return state
    end
    return g
end
f()",
    ];

    const FAIL: &[&str] = &[
        "\
local function f()
    counter = 0
    counter = counter + 1
    return counter
end
f()",
        "\
local function f()
    cache = {}
end
f()",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::GlobalUsedAsLocal, PASS, FAIL).test_and_snapshot();
    }
}
