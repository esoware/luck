use luck_ast::{AstKind, AstType, Name, Statement};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{MAX_ACTIVE_LOCALS, Span};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags a local declared again in the same block (Luacheck 411), which
/// hides the first one from all the code after it.
///
/// `shadowing` reports any redeclaration of a visible name, including ones
/// from enclosing blocks. Doing it in the same block is most often a
/// copy-paste bug, so it gets its own toggle.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RedefiningLocal;

declare_lint_rule!(
    RedefiningLocal,
    Style,
    Warning,
    "local redeclared with the same name in the same block"
);

impl Rule for RedefiningLocal {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        // Each name the block declares, in order. Every one stays active
        // until the block ends, so no Lua compiles a block with more than
        // `MAX_ACTIVE_LOCALS`, and the rest of one isn't checked.
        let mut declarations: Vec<(&str, Span)> = Vec::new();
        let mut declare = |declared: &'ast Name<'ast>| {
            let name = declared.text.as_str();
            // A leading `_` marks a name as deliberately unused.
            if declarations.len() > MAX_ACTIVE_LOCALS || name.starts_with('_') {
                return;
            }
            if let Some(&(_, first_span)) =
                declarations.iter().find(|(declared_name, _)| *declared_name == name)
            {
                context.diagnostic(
                    Finding::new(
                        format!("local `{name}` is redeclared in the same block"),
                        declared.span,
                    )
                    .with_label(first_span, "first declared here"),
                );
            }
            declarations.push((name, declared.span));
        };
        for stmt in &block.stmts {
            if let Statement::LocalFunction(local_function) = stmt {
                declare(&local_function.name);
            }
            if let Statement::LocalAssignment(local) = stmt {
                for attributed in &local.names {
                    declare(&attributed.name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    /// `count` locals, then the first declared again.
    fn locals_then_the_first_again(count: u32) -> String {
        let mut source = String::new();
        for i in 0..count {
            writeln!(source, "local x{i} = 1").unwrap();
        }
        source.push_str("local x0 = 1");
        source
    }

    const PASS: &[(&str, LuaTarget)] = &[
        (
            "\
do local x = 1 end
do local x = 2 end",
            LuaTarget::Lua54,
        ),
        // Inner block shadowing is `shadowing`'s job.
        (
            "\
local x = 1
do local x = 2 end",
            LuaTarget::Lua54,
        ),
        (
            "\
local _ = 1
local _ = 2",
            LuaTarget::Lua54,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (
            "\
do local x = 1
local x = 2
print(x) end",
            LuaTarget::Lua54,
        ),
        (
            "\
local x = 1
local x = 2
local x = 3",
            LuaTarget::Lua54,
        ),
        (
            "\
local function f() end
local function f() end",
            LuaTarget::Lua54,
        ),
    ];

    #[test]
    fn test() {
        let at_limit = locals_then_the_first_again(199);
        // Only Luau's compiler, not luck, rejects this many active locals.
        let past_limit = locals_then_the_first_again(201);
        let pass: Vec<_> =
            PASS.iter().copied().chain([(past_limit.as_str(), LuaTarget::Luau)]).collect();
        let fail: Vec<_> =
            FAIL.iter().copied().chain([(at_limit.as_str(), LuaTarget::Lua54)]).collect();
        Tester::new(RuleId::RedefiningLocal, &pass, &fail).test_and_snapshot();
    }
}
