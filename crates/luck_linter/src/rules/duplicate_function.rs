use luck_ast::{FunctionName, Statement};
use luck_token::Span;
use rustc_hash::FxHashMap;

use crate::diagnostic::{Finding, line_column};
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateFunction;

impl_rule!(
    DuplicateFunction,
    "duplicate_function",
    Suspicious,
    Warning,
    "function with the same name is defined twice in the same block"
);

impl WholeRule for DuplicateFunction {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();
        for block in context.blocks {
            let mut definitions: FxHashMap<String, Span> = FxHashMap::default();
            for stmt in &block.stmts {
                // `redefining_local` covers local functions.
                let (declared_name, name_span) = if let Statement::FunctionDecl(decl) = stmt {
                    (full_name(&decl.name), decl.name.span)
                } else if let Statement::GlobalFunction(function) = stmt {
                    // Lua 5.5
                    (function.name.identifier().map(str::to_owned), function.name.span)
                } else {
                    continue;
                };
                let Some(name) = declared_name else {
                    continue;
                };
                if let Some(first_span) = definitions.get(&name) {
                    let (line, _) =
                        line_column(context.source, first_span.start, context.semantic.version);
                    findings.push(Finding::new(
                        format!(
                            "duplicate function definition `{name}`, also defined on line {line}"
                        ),
                        name_span,
                    ));
                } else {
                    definitions.insert(name, name_span);
                }
            }
        }
        findings
    }
}

/// The declared name as written, such as `a.b:c`, or `None` if any part
/// isn't an identifier.
fn full_name(function_name: &FunctionName) -> Option<String> {
    let mut names = function_name.names.iter();
    let mut joined = names.next()?.identifier()?.to_owned();
    for token in names {
        joined.push('.');
        joined.push_str(token.identifier()?);
    }
    if let Some(method_token) = &function_name.method {
        joined.push(':');
        joined.push_str(method_token.identifier()?);
    }
    Some(joined)
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&DuplicateFunction, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_duplicate_plain_function() {
        let diags = run("\
function f() end
function f() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`f`"));
        assert!(diags[0].message.contains("line 1"));
    }

    #[test]
    fn flags_duplicate_dotted_function() {
        let diags = run("\
local t = {}
function t.m() end
function t.m() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`t.m`"));
        assert!(diags[0].message.contains("line 2"));
    }

    #[test]
    fn flags_duplicate_method() {
        let diags = run("\
local t = {}
function t:m() end
function t:m() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`t:m`"));
    }

    #[test]
    fn flags_duplicates_in_nested_block() {
        let diags = run("\
do
function f() end
function f() end
end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("line 2"));
    }

    #[test]
    fn flags_each_later_duplicate() {
        let diags = run("\
function f() end
function f() end
function f() end");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn ignores_same_name_with_dot_and_colon() {
        let diags = run("\
local t = {}
function t.m() end
function t:m() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_different_names() {
        let diags = run("\
function f() end
function g() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_local_functions() {
        let diags = run("\
local function f() end
local function f() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_sibling_blocks() {
        let diags = run("\
do function f() end end
do function f() end end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_duplicate_global_function() {
        let source = "\
global function f() end
global function f() end";
        let diags = run_rule(&DuplicateFunction, source, LuaVersion::Lua55);
        assert_eq!(diags.len(), 1, "both statements define `f` in one block: {diags:?}");
    }

    #[test]
    fn flags_each_repeat_in_a_long_block() {
        let mut functions = String::new();
        for i in 0..2_000u32 {
            let _unused = writeln!(functions, "function m.f{i}() end");
        }
        let diags = run(&format!("{functions}{functions}"));
        assert_eq!(diags.len(), 2_000, "{diags:?}");
    }
}
