use std::collections::hash_map::Entry;

use luck_ast::{AstKind, AstType, FunctionName, Statement};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;
use rustc_hash::FxHashMap;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DuplicateFunction;

declare_lint_rule!(
    DuplicateFunction,
    Suspicious,
    Warning,
    "function with the same name is defined twice in the same block"
);

impl Rule for DuplicateFunction {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        let mut definitions: FxHashMap<String, Span> = FxHashMap::default();
        for stmt in &block.stmts {
            // `redefining_local` covers local functions.
            let Statement::FunctionDecl(decl) = stmt else {
                continue;
            };
            let Some(name) = full_name(&decl.name) else {
                continue;
            };
            let name_span = decl.name.span;
            match definitions.entry(name) {
                Entry::Occupied(first) => context.diagnostic(
                    Finding::new(
                        format!("duplicate function definition `{}`", first.key()),
                        name_span,
                    )
                    .with_label(*first.get(), "first defined here"),
                ),
                Entry::Vacant(slot) => {
                    slot.insert(name_span);
                }
            }
        }
    }
}

/// The declared name as written, such as `a.b:c`.
fn full_name(function_name: &FunctionName<'_>) -> Option<String> {
    let mut names = function_name.names.iter();
    let mut joined = names.next()?.text.as_str().to_owned();
    for name in names {
        joined.push('.');
        joined.push_str(&name.text);
    }
    if let Some(method) = &function_name.method {
        joined.push(':');
        joined.push_str(&method.text);
    }
    Some(joined)
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local t = {}
function t.m() end
function t:m() end",
        "\
function f() end
function g() end",
        "\
local function f() end
local function f() end",
        "\
do function f() end end
do function f() end end",
    ];

    const FAIL: &[&str] = &[
        "\
function f() end
function f() end",
        "\
local t = {}
function t.m() end
function t.m() end",
        "\
local t = {}
function t:m() end
function t:m() end",
        "\
do
function f() end
function f() end
end",
        "\
function f() end
function f() end
function f() end",
        "\
function m.f0() end
function m.f1() end
function m.f0() end
function m.f1() end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::DuplicateFunction, PASS, FAIL).test_and_snapshot();
    }
}
