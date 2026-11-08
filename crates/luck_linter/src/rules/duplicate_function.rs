use luck_ast::{Block, FunctionName, Statement, Visitor};
use luck_token::Span;

use crate::diagnostic::{Category, Finding, Severity, line_column};
use crate::rule::{LintContext, Rule, WholeRule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateFunction;

impl Rule for DuplicateFunction {
    fn name(&self) -> &'static str {
        "duplicate_function"
    }

    fn category(&self) -> Category {
        Category::Suspicious
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "function with the same name is defined twice in the same block"
    }
}

impl WholeRule for DuplicateFunction {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut checker = DuplicateChecker { source: context.source, findings: Vec::new() };
        checker.visit_block(context.block);
        checker.findings
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

struct DuplicateChecker<'document> {
    source: &'document str,
    findings: Vec<Finding>,
}

impl DuplicateChecker<'_> {
    fn check_block(&mut self, block: &Block) {
        let mut definitions: Vec<(String, Span)> = Vec::new();

        for stmt in &block.stmts {
            // `redefining_local` covers local functions.
            let Statement::FunctionDecl(decl) = stmt else {
                continue;
            };
            let Some(name) = full_name(&decl.name) else {
                continue;
            };
            if let Some((_, first_span)) =
                definitions.iter().find(|(defined_name, _)| *defined_name == name)
            {
                let (line, _) = line_column(self.source, first_span.start);
                self.findings.push(Finding::new(
                    format!("duplicate function definition `{name}`, also defined on line {line}"),
                    decl.name.span,
                ));
            } else {
                definitions.push((name, decl.name.span));
            }
        }
    }
}

impl<'ast> Visitor<'ast> for DuplicateChecker<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.check_block(block);
        self.walk_block(block);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&DuplicateFunction, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_duplicate_plain_function() {
        let diags = run("function f() end\nfunction f() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`f`"));
        assert!(diags[0].message.contains("line 1"));
    }

    #[test]
    fn flags_duplicate_dotted_function() {
        let diags = run("local t = {}\nfunction t.m() end\nfunction t.m() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`t.m`"));
        assert!(diags[0].message.contains("line 2"));
    }

    #[test]
    fn flags_duplicate_method() {
        let diags = run("local t = {}\nfunction t:m() end\nfunction t:m() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`t:m`"));
    }

    #[test]
    fn flags_duplicates_in_nested_block() {
        let diags = run("do\nfunction f() end\nfunction f() end\nend");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("line 2"));
    }

    #[test]
    fn flags_each_later_duplicate() {
        let diags = run("function f() end\nfunction f() end\nfunction f() end");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn ignores_same_name_with_dot_and_colon() {
        let diags = run("local t = {}\nfunction t.m() end\nfunction t:m() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_different_names() {
        let diags = run("function f() end\nfunction g() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_local_functions() {
        let diags = run("local function f() end\nlocal function f() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_sibling_blocks() {
        let diags = run("do function f() end end\ndo function f() end end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
