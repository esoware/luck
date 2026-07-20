use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_ast::shared::Block;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;
use luck_core::diagnostics::{Diagnostic, errors};
use luck_token::Span;
use luck_token::token::TokenKind;
use std::ops::Range;

/// Information about a single `require()` call extracted from a module.
#[derive(Debug, Clone)]
pub struct RequireInfo {
    pub local_name: String,
    pub require_string: String,
    pub span: Range<usize>,
    /// Byte span of just the `require(...)` call expression within the source
    pub call_span: Range<usize>,
}

/// Result of scanning a module for `require()` calls.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    pub requires: Vec<RequireInfo>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Scans the ENTIRE module tree for `require()` calls - any statement,
/// any expression position, any function body. The lazy loader makes
/// require position-independent, exactly like real Lua: `local m =
/// require("x") :: T`, `require("m").field`, conditional requires, and
/// requires inside functions all bundle.
pub fn extract_requires(block: &Block, file_path: &str) -> ExtractResult {
    let mut finder = RequireFinder {
        file_path,
        requires: Vec::new(),
        diagnostics: Vec::new(),
        seen_require_strings: std::collections::HashSet::new(),
    };
    finder.visit_block(block);

    let RequireFinder {
        mut requires,
        mut diagnostics,
        ..
    } = finder;
    requires.sort_by_key(|info| info.call_span.start);

    check_package_loaded(block, file_path, &mut diagnostics);

    ExtractResult {
        requires,
        diagnostics,
    }
}

struct RequireFinder<'a> {
    file_path: &'a str,
    requires: Vec<RequireInfo>,
    diagnostics: Vec<Diagnostic>,
    seen_require_strings: std::collections::HashSet<String>,
}

impl RequireFinder<'_> {
    fn record_require(&mut self, func_call: &FunctionCall) {
        match extract_require_string(func_call) {
            Some((require_string, call_span)) => {
                if !self.seen_require_strings.insert(require_string.clone()) {
                    self.diagnostics.push(errors::w001(
                        self.file_path,
                        call_span.clone(),
                        &require_string,
                    ));
                }
                self.requires.push(RequireInfo {
                    local_name: String::new(),
                    require_string,
                    span: call_span.clone(),
                    call_span,
                });
            }
            // `require(expr)` can't be resolved statically.
            None => {
                self.diagnostics
                    .push(errors::e002(self.file_path, span_to_range(func_call.span)));
            }
        }
    }
}

impl<'ast> Visitor<'ast> for RequireFinder<'_> {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionCall(func_call) = expr
            && is_require_call(func_call)
        {
            self.record_require(func_call);
        }
        self.walk_expression(expr);
    }

    fn visit_statement(&mut self, stmt: &'ast Statement) {
        // Statement-level calls never surface as Expression::FunctionCall
        // in the walk; a bare `require("side_effects")` statement is legal
        // and rewrites to a bare `__luck_require(n)` call.
        if let Statement::FunctionCall(call_stmt) = stmt
            && is_require_call(&call_stmt.call)
        {
            self.record_require(&call_stmt.call);
        }
        self.walk_statement(stmt);
    }
}

fn is_require_call(func_call: &FunctionCall) -> bool {
    func_call.method.is_none()
        && matches!(
            &func_call.callee,
            Expression::Var(var) if matches!(
                var.as_ref(),
                Var::Name(token) if matches!(&token.kind, TokenKind::Identifier(name) if name == "require")
            )
        )
}

pub(crate) fn extract_require_string(func_call: &FunctionCall) -> Option<(String, Range<usize>)> {
    if !is_require_call(func_call) {
        return None;
    }

    let call_span = span_to_range(func_call.span);

    match &func_call.args {
        FunctionArgs::Parenthesized { args, .. } => {
            let arg_list: Vec<_> = args.iter().collect();
            if arg_list.len() != 1 {
                return None;
            }
            match &arg_list[0] {
                Expression::StringLiteral(literal) => {
                    let string_value = extract_string_literal_value(&literal.text)?;
                    Some((string_value, call_span))
                }
                _ => None,
            }
        }
        FunctionArgs::StringLiteral(literal) => {
            let string_value = extract_string_literal_value(&literal.text)?;
            Some((string_value, call_span))
        }
        _ => None,
    }
}

pub(crate) fn extract_string_literal_value(raw: &str) -> Option<String> {
    if let Some(inner) = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(inner.to_string())
    } else if let Some(inner) = raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        Some(inner.to_string())
    } else if let Some(after_bracket) = raw.strip_prefix('[') {
        // Long string: [[...]] or [=[...]=] etc.
        let eq_count = after_bracket.chars().take_while(|&c| c == '=').count();
        let open_len = 2 + eq_count; // [==[
        let close_len = 2 + eq_count; // ]==]
        if raw.len() >= open_len + close_len {
            Some(raw[open_len..raw.len() - close_len].to_string())
        } else {
            Some(raw.to_string())
        }
    } else {
        Some(raw.to_string())
    }
}

fn check_package_loaded(block: &Block, file_path: &str, diagnostics: &mut Vec<Diagnostic>) {
    struct PackageLoadedVisitor {
        file_path: String,
        diagnostics: Vec<Diagnostic>,
    }

    impl<'ast> Visitor<'ast> for PackageLoadedVisitor {
        fn visit_statement(&mut self, stmt: &'ast Statement) {
            if let Statement::Assignment(assignment) = stmt {
                for var in assignment.targets.iter() {
                    if is_package_loaded_access(var) {
                        self.diagnostics
                            .push(errors::e006(&self.file_path, var_span(var)));
                    }
                }
            }
            self.walk_statement(stmt);
        }
    }

    let mut visitor = PackageLoadedVisitor {
        file_path: file_path.to_string(),
        diagnostics: Vec::new(),
    };
    visitor.visit_block(block);
    diagnostics.append(&mut visitor.diagnostics);
}

fn is_package_loaded_access(var: &Var) -> bool {
    // Handles all AST shapes: `package.loaded.x`, `package.loaded["x"]`, `package["loaded"].x`
    expr_contains_package_loaded(&Expression::Var(Box::new(var.clone())))
}

fn expr_contains_package_loaded(expr: &Expression) -> bool {
    match expr {
        Expression::Var(var) => match var.as_ref() {
            Var::FieldAccess(field_access) => {
                if matches!(&field_access.name.kind, TokenKind::Identifier(name) if name == "loaded")
                    && is_package_name_expr(&field_access.prefix)
                {
                    return true;
                }
                expr_contains_package_loaded(&field_access.prefix)
            }
            Var::Index(index_expr) => {
                if is_string_literal_with_value(&index_expr.index, "loaded")
                    && is_package_name_expr(&index_expr.prefix)
                {
                    return true;
                }
                expr_contains_package_loaded(&index_expr.prefix)
            }
            _ => false,
        },
        _ => false,
    }
}

fn is_package_name_expr(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::Var(var) if matches!(
            var.as_ref(),
            Var::Name(token) if matches!(&token.kind, TokenKind::Identifier(name) if name == "package")
        )
    )
}

fn is_string_literal_with_value(expr: &Expression, expected: &str) -> bool {
    if let Expression::StringLiteral(literal) = expr {
        extract_string_literal_value(&literal.text).is_some_and(|val| val == expected)
    } else {
        false
    }
}

fn var_span(var: &Var) -> Range<usize> {
    match var {
        Var::Name(token) => span_to_range(token.span),
        Var::Index(index_expr) => span_to_range(index_expr.span),
        Var::FieldAccess(field_access) => span_to_range(field_access.span),
    }
}

fn span_to_range(span: Span) -> Range<usize> {
    span.start as usize..span.end as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn parse_lua(source: &str) -> Block {
        let result = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(
            result.errors.is_empty(),
            "parse failed: {:?}",
            result.errors
        );
        result.block
    }

    #[test]
    fn test_basic_require() {
        let source = r#"local utils = require("utils")
print(utils.foo())
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "utils");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_multiple_requires() {
        let source = r#"local a = require("mod_a")
local b = require("mod_b")
local c = require("mod_c")
print(a, b, c)
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 3);
        assert_eq!(result.requires[0].require_string, "mod_a");
        assert_eq!(result.requires[1].require_string, "mod_b");
        assert_eq!(result.requires[2].require_string, "mod_c");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_require_after_code_is_legal() {
        // Position-independent with the lazy loader - no E001.
        let source = "print(\"hello\")
local x = require(\"x\")
";
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn test_non_literal_require_e002() {
        let source = r#"local x = require(varname)
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 0);
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == "E002")
            .collect();
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_bare_require_statement_extracted() {
        // `require("x")` as a statement is legal: it rewrites to a bare
        // `__luck_require(n)` call (side-effect import).
        let source = "require(\"something\")
";
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "something");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn test_package_loaded_e006() {
        let source = r#"package.loaded["mymod"] = {}
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == "E006")
            .collect();
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_package_loaded_dot_e006() {
        let source = r#"package.loaded.mymod = {}
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == "E006")
            .collect();
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_duplicate_require_w001() {
        let source = r#"local a = require("utils")
local b = require("utils")
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 2);
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == "W001")
            .collect();
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn test_top_level_vararg_no_warning() {
        // The loader calls each module with its slot id, so the
        // `local modname = ...` idiom keeps working - no W002.
        let source = "local modname = ...
return modname
";
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn test_vararg_in_function_no_warning() {
        let source = r#"local function foo(...)
    return ...
end
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == "W002")
            .collect();
        assert_eq!(warnings.len(), 0);
    }

    #[test]
    fn test_no_requires() {
        let source = r#"print("hello world")
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert!(result.requires.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_require_with_return() {
        let source = r#"local utils = require("utils")
return utils.process()
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert!(block.last_stmt.is_some());
    }

    #[test]
    fn test_require_string_syntax() {
        let source = r#"local m = require "mymod"
"#;
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "mymod");
    }

    #[test]
    fn test_require_single_quoted_parens() {
        let source = "local m = require('mymod')\n";
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "mymod");
    }

    #[test]
    fn test_require_long_string() {
        let source = "local m = require [[mymod]]\n";
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "mymod");
    }

    #[test]
    fn test_multi_name_local_extracted() {
        // Whole-tree scan: `local a, b = require("x"), 1` bundles too.
        let source = "local a, b = require(\"x\"), 1
";
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "x");
    }

    #[test]
    fn test_require_in_nested_function_extracted() {
        // Deferred requires inside functions are the lazy loader's whole
        // point (mutually recursive modules).
        let source = "local function setup()
    local m = require(\"inner\")
end
";
        let block = parse_lua(source);
        let result = extract_requires(&block, "test.lua");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "inner");
    }
}
