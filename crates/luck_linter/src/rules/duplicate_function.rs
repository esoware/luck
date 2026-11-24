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
            let (declared_name, name_span) = if let Statement::FunctionDecl(decl) = stmt {
                (full_name(&decl.name), decl.name.span)
            } else if let Statement::GlobalFunction(function) = stmt {
                // Lua 5.5
                (Some(function.name.text.as_str().to_owned()), function.name.span)
            } else {
                continue;
            };
            let Some(name) = declared_name else {
                continue;
            };
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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        (
            "\
local t = {}
function t.m() end
function t:m() end",
            LuaTarget::Lua54,
        ),
        (
            "\
function f() end
function g() end",
            LuaTarget::Lua54,
        ),
        (
            "\
local function f() end
local function f() end",
            LuaTarget::Lua54,
        ),
        (
            "\
do function f() end end
do function f() end end",
            LuaTarget::Lua54,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (
            "\
function f() end
function f() end",
            LuaTarget::Lua54,
        ),
        (
            "\
local t = {}
function t.m() end
function t.m() end",
            LuaTarget::Lua54,
        ),
        (
            "\
local t = {}
function t:m() end
function t:m() end",
            LuaTarget::Lua54,
        ),
        (
            "\
do
function f() end
function f() end
end",
            LuaTarget::Lua54,
        ),
        (
            "\
function f() end
function f() end
function f() end",
            LuaTarget::Lua54,
        ),
        // Both statements define `f` in one block.
        (
            "\
global function f() end
global function f() end",
            LuaTarget::Lua55,
        ),
        (
            "\
function m.f0() end
function m.f1() end
function m.f0() end
function m.f1() end",
            LuaTarget::Lua54,
        ),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::DuplicateFunction, PASS, FAIL).test_and_snapshot();
    }
}
