use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_ast::shared::Block;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;
use luck_core::diagnostics::{Diagnostic, errors};
use luck_core::types::DynamicRequire;
use luck_semantic::SemanticAnalysis;
use luck_token::token::TokenKind;
use luck_token::{LuaVersion, Span};
use std::ops::Range;

/// Information about a single `require()` call extracted from a module.
#[derive(Debug, Clone)]
pub struct RequireInfo {
    /// The decoded runtime value of the require string (escape sequences
    /// resolved), exactly what real `require` would receive.
    pub require_string: String,
    /// Span of the `require(...)` call, handed to the resolver for diagnostics.
    pub span: Span,
    /// Byte range of the `require(...)` call expression; the emitter
    /// splices the loader call over exactly this range.
    pub call_span: Range<usize>,
}

/// Result of scanning a module for `require()` calls.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    pub requires: Vec<RequireInfo>,
    /// Byte ranges of the `require` identifier in calls whose argument is not
    /// a string literal. The emitter retargets exactly this token, leaving the
    /// argument expression (and any static require nested in it) untouched.
    pub dynamic_callees: Vec<Range<usize>>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Scans the ENTIRE module tree for `require()` calls - any statement,
/// any expression position, any function body. The lazy loader makes
/// require position-independent, exactly like real Lua. Scope analysis
/// filters out calls through local bindings named `require` (they keep
/// their user semantics, W005), and references to the global `require`
/// that are not direct calls warn too: those call sites escape bundling.
///
/// A `require(expr)` cannot name a module at build time; `dynamic_require`
/// decides whether that aborts the bundle or is kept for the runtime.
pub fn extract_requires(
    block: &Block,
    file_path: &str,
    version: LuaVersion,
    dynamic_require: DynamicRequire,
) -> ExtractResult {
    let analysis = luck_semantic::analyze(block, version);
    let mut finder = RequireFinder {
        file_path,
        version,
        dynamic_require,
        analysis: &analysis,
        requires: Vec::new(),
        dynamic_callees: Vec::new(),
        diagnostics: Vec::new(),
        seen_require_strings: rustc_hash::FxHashSet::default(),
        direct_callee_spans: rustc_hash::FxHashSet::default(),
    };
    finder.visit_block(block);

    let RequireFinder {
        mut requires,
        dynamic_callees,
        mut diagnostics,
        direct_callee_spans,
        ..
    } = finder;

    for reference in &analysis.scope_tree.references {
        if reference.name == "require"
            && reference.resolved.is_none()
            && !direct_callee_spans.contains(&reference.span)
        {
            diagnostics.push(errors::w005_aliased(file_path, reference.span.into()));
        }
    }

    requires.sort_by_key(|info| info.call_span.start);

    // Lua targets back the bundle cache with package.loaded itself, so
    // manipulating it behaves exactly as in real Lua. Luau has no
    // package table and the bundle cache is private there.
    if version.is_luau() {
        check_package_loaded(block, file_path, version, &mut diagnostics);
    }

    ExtractResult {
        requires,
        dynamic_callees,
        diagnostics,
    }
}

struct RequireFinder<'a> {
    file_path: &'a str,
    version: LuaVersion,
    dynamic_require: DynamicRequire,
    analysis: &'a SemanticAnalysis,
    requires: Vec<RequireInfo>,
    dynamic_callees: Vec<Range<usize>>,
    diagnostics: Vec<Diagnostic>,
    seen_require_strings: rustc_hash::FxHashSet<String>,
    direct_callee_spans: rustc_hash::FxHashSet<Span>,
}

impl RequireFinder<'_> {
    fn handle_call(&mut self, func_call: &FunctionCall) {
        let Some(callee_span) = require_callee_span(func_call) else {
            return;
        };
        self.direct_callee_spans.insert(callee_span);
        if self.analysis.resolves_to_local("require", callee_span) {
            self.diagnostics.push(errors::w005_shadowed(
                self.file_path,
                span_to_range(func_call.span),
            ));
            return;
        }

        match extract_require_string(func_call, self.version) {
            Some((require_string, call_span)) => {
                if !self.seen_require_strings.insert(require_string.clone()) {
                    self.diagnostics.push(errors::w001(
                        self.file_path,
                        call_span.clone(),
                        &require_string,
                    ));
                }
                self.requires.push(RequireInfo {
                    require_string,
                    span: func_call.span,
                    call_span,
                });
            }
            // `require(expr)` can't be resolved statically. Aborting the whole
            // bundle over one such call is rarely what the author wants, so by
            // default it stays and resolves at runtime.
            None => self.handle_dynamic_call(func_call, callee_span),
        }
    }

    fn handle_dynamic_call(&mut self, func_call: &FunctionCall, callee_span: Span) {
        let call_range = span_to_range(func_call.span);
        match self.dynamic_require {
            DynamicRequire::Error => {
                self.diagnostics
                    .push(errors::e002(self.file_path, call_range));
                return;
            }
            DynamicRequire::Warn if self.version.is_luau() => self
                .diagnostics
                .push(errors::w007_runtime(self.file_path, call_range)),
            DynamicRequire::Warn => self
                .diagnostics
                .push(errors::w007_loader_fallback(self.file_path, call_range)),
            DynamicRequire::Allow => {}
        }
        // Luau keys its cache by resolved file, and a Roblox require takes an
        // Instance, so there is nothing a runtime argument could look up
        // there: the call stays exactly as written.
        if !self.version.is_luau() {
            self.dynamic_callees.push(span_to_range(callee_span));
        }
    }
}

impl<'ast> Visitor<'ast> for RequireFinder<'_> {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionCall(func_call) = expr {
            self.handle_call(func_call);
        }
        self.walk_expression(expr);
    }

    fn visit_statement(&mut self, stmt: &'ast Statement) {
        // Statement-level calls never surface as Expression::FunctionCall
        // in the walk; a bare `require("side_effects")` statement is legal
        // and rewrites to a bare loader call.
        if let Statement::FunctionCall(call_stmt) = stmt {
            self.handle_call(&call_stmt.call);
        }
        self.walk_statement(stmt);
    }
}

/// The callee token span when `func_call` is a direct, non-method call
/// of a variable named `require` (whatever that name resolves to).
fn require_callee_span(func_call: &FunctionCall) -> Option<Span> {
    if func_call.method.is_some() {
        return None;
    }
    match &func_call.callee {
        Expression::Var(Var::Name(token)) if matches!(&token.kind, TokenKind::Identifier(name) if name == "require") => {
            Some(token.span)
        }
        _ => None,
    }
}

pub(crate) fn extract_require_string(
    func_call: &FunctionCall,
    version: LuaVersion,
) -> Option<(String, Range<usize>)> {
    let call_span = span_to_range(func_call.span);

    let literal_text = match &func_call.args {
        FunctionArgs::Parenthesized { args, .. } => {
            let arg_list: Vec<_> = args.iter().collect();
            if arg_list.len() != 1 {
                return None;
            }
            match &arg_list[0] {
                Expression::StringLiteral(literal) => &literal.text,
                _ => return None,
            }
        }
        FunctionArgs::StringLiteral(literal) => &literal.text,
        _ => return None,
    };

    let string_value = decode_literal(literal_text, version)?;
    Some((string_value, call_span))
}

/// Decode a string literal token to the runtime string real `require`
/// would see: escapes resolved, long-string leading newline stripped.
fn decode_literal(raw: &str, version: LuaVersion) -> Option<String> {
    let bytes = luck_token::literal::decode_string_literal(raw, version)?;
    String::from_utf8(bytes).ok()
}

fn check_package_loaded(
    block: &Block,
    file_path: &str,
    version: LuaVersion,
    diagnostics: &mut Vec<Diagnostic>,
) {
    struct PackageLoadedVisitor {
        file_path: String,
        version: LuaVersion,
        diagnostics: Vec<Diagnostic>,
    }

    impl<'ast> Visitor<'ast> for PackageLoadedVisitor {
        fn visit_statement(&mut self, stmt: &'ast Statement) {
            if let Statement::Assignment(assignment) = stmt {
                for var in assignment.targets.iter() {
                    if is_package_loaded_access(var, self.version) {
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
        version,
        diagnostics: Vec::new(),
    };
    visitor.visit_block(block);
    diagnostics.append(&mut visitor.diagnostics);
}

fn is_package_loaded_access(var: &Var, version: LuaVersion) -> bool {
    // Handles all AST shapes: `package.loaded.x`, `package.loaded["x"]`, `package["loaded"].x`
    expr_contains_package_loaded(&Expression::Var(var.clone()), version)
}

fn expr_contains_package_loaded(expr: &Expression, version: LuaVersion) -> bool {
    match expr {
        Expression::Var(var) => match var {
            Var::FieldAccess(field_access) => {
                if matches!(&field_access.name.kind, TokenKind::Identifier(name) if name == "loaded")
                    && is_package_name_expr(&field_access.prefix)
                {
                    return true;
                }
                expr_contains_package_loaded(&field_access.prefix, version)
            }
            Var::Index(index_expr) => {
                if is_string_literal_with_value(&index_expr.index, "loaded", version)
                    && is_package_name_expr(&index_expr.prefix)
                {
                    return true;
                }
                expr_contains_package_loaded(&index_expr.prefix, version)
            }
            _ => false,
        },
        _ => false,
    }
}

fn is_package_name_expr(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::Var(Var::Name(token))
            if matches!(&token.kind, TokenKind::Identifier(name) if name == "package")
    )
}

fn is_string_literal_with_value(expr: &Expression, expected: &str, version: LuaVersion) -> bool {
    if let Expression::StringLiteral(literal) = expr {
        decode_literal(&literal.text, version).is_some_and(|value| value == expected)
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

    fn parse(source: &str, version: LuaVersion) -> Block {
        let result = luck_parser::parse(source, version);
        assert!(
            result.errors.is_empty(),
            "parse failed: {:?}",
            result.errors
        );
        result.block
    }

    fn extract(source: &str) -> ExtractResult {
        extract_with(source, DynamicRequire::default())
    }

    fn extract_with(source: &str, dynamic_require: DynamicRequire) -> ExtractResult {
        extract_requires(
            &parse(source, LuaVersion::Lua54),
            "test.lua",
            LuaVersion::Lua54,
            dynamic_require,
        )
    }

    fn extract_luau(source: &str) -> ExtractResult {
        extract_requires(
            &parse(source, LuaVersion::Luau),
            "test.luau",
            LuaVersion::Luau,
            DynamicRequire::default(),
        )
    }

    fn codes<'a>(result: &'a ExtractResult, code: &str) -> Vec<&'a Diagnostic> {
        result
            .diagnostics
            .iter()
            .filter(|d| d.code == code)
            .collect()
    }

    #[test]
    fn extracts_single_require() {
        let result = extract("local utils = require(\"utils\")\nprint(utils.foo())\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "utils");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn extracts_multiple_requires_in_order() {
        let result = extract(
            "local a = require(\"mod_a\")\nlocal b = require(\"mod_b\")\nlocal c = require(\"mod_c\")\nprint(a, b, c)\n",
        );
        assert_eq!(result.requires.len(), 3);
        assert_eq!(result.requires[0].require_string, "mod_a");
        assert_eq!(result.requires[1].require_string, "mod_b");
        assert_eq!(result.requires[2].require_string, "mod_c");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn require_after_code_is_not_flagged() {
        // Position-independent with the lazy loader - no E001.
        let result = extract("print(\"hello\")\nlocal x = require(\"x\")\n");
        assert_eq!(result.requires.len(), 1);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn non_literal_require_flags_e002_in_error_mode() {
        let result = extract_with("local x = require(varname)\n", DynamicRequire::Error);
        assert_eq!(result.requires.len(), 0);
        assert!(result.dynamic_callees.is_empty());
        assert_eq!(codes(&result, "E002").len(), 1);
    }

    /// The default: one runtime-resolved require must not abort the bundle.
    #[test]
    fn non_literal_require_warns_and_is_kept_by_default() {
        let result = extract("local x = require(varname)\n");
        assert_eq!(result.requires.len(), 0);
        assert!(
            codes(&result, "E002").is_empty(),
            "{:?}",
            result.diagnostics
        );
        assert_eq!(codes(&result, "W007").len(), 1, "{:?}", result.diagnostics);
        // The callee token alone, so the argument expression is left intact.
        assert_eq!(result.dynamic_callees, vec![10..17]);
    }

    #[test]
    fn allow_mode_keeps_the_call_without_warning() {
        let result = extract_with("local x = require(varname)\n", DynamicRequire::Allow);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.dynamic_callees, vec![10..17]);
    }

    /// Luau keys its cache by resolved file and Roblox requires take an
    /// Instance, so there is nothing for a runtime argument to look up: the
    /// call is warned about but never retargeted.
    #[test]
    fn luau_dynamic_require_is_warned_but_not_retargeted() {
        let result = extract_luau("local x = require(varname)\n");
        assert_eq!(codes(&result, "W007").len(), 1, "{:?}", result.diagnostics);
        assert!(result.dynamic_callees.is_empty());
    }

    /// A static require nested inside a dynamic one still bundles: the
    /// retargeted span is the outer callee only.
    #[test]
    fn static_require_nested_in_a_dynamic_one_still_bundles() {
        let result = extract("local x = require(require(\"names\").first)\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "names");
        assert_eq!(result.dynamic_callees, vec![10..17]);
    }

    #[test]
    fn bare_require_statement_is_extracted() {
        let result = extract("require(\"something\")\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "something");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn escape_sequences_decode_like_real_lua() {
        // require("a\46b") is require("a.b") at runtime; the bundler must
        // resolve and deduplicate on the decoded value.
        let result = extract("local a = require(\"a\\46b\")\nlocal b = require(\"a.b\")\n");
        assert_eq!(result.requires.len(), 2);
        assert_eq!(result.requires[0].require_string, "a.b");
        assert_eq!(result.requires[1].require_string, "a.b");
        // Same decoded module twice: the duplicate-require warning fires.
        assert_eq!(codes(&result, "W001").len(), 1, "{:?}", result.diagnostics);
    }

    #[test]
    fn long_string_leading_newline_is_stripped() {
        let result = extract("local m = require [[\nmymod]]\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "mymod");
    }

    #[test]
    fn shadowed_require_is_skipped_with_w005() {
        let source = "local require = function(s) return s end\nlocal x = require(\"dep\")\n";
        let result = extract(source);
        assert!(
            result.requires.is_empty(),
            "shadowed require must not be bundled: {:?}",
            result.requires
        );
        assert_eq!(codes(&result, "W005").len(), 1, "{:?}", result.diagnostics);
        assert!(codes(&result, "E002").is_empty());
    }

    #[test]
    fn shadowed_require_in_inner_scope_only_skips_there() {
        let source = "local a = require(\"real\")\nlocal function f()\n    local require = print\n    require(\"fake\")\nend\n";
        let result = extract(source);
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "real");
        assert_eq!(codes(&result, "W005").len(), 1, "{:?}", result.diagnostics);
    }

    #[test]
    fn aliased_require_flags_w005() {
        let result = extract("local r = require\nlocal x = r(\"dep\")\n");
        assert!(result.requires.is_empty());
        assert_eq!(codes(&result, "W005").len(), 1, "{:?}", result.diagnostics);
    }

    #[test]
    fn direct_calls_do_not_flag_w005() {
        let result = extract("local a = require(\"x\")\nrequire(\"y\")\n");
        assert_eq!(result.requires.len(), 2);
        assert!(
            codes(&result, "W005").is_empty(),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn package_loaded_write_is_allowed_on_lua_targets() {
        // The bundle cache is package.loaded itself on Lua targets, so
        // manipulating it behaves exactly as in real Lua - no E006.
        let result = extract("package.loaded[\"mymod\"] = {}\npackage.loaded.other = {}\n");
        assert!(
            codes(&result, "E006").is_empty(),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn package_loaded_index_write_flags_e006_on_luau() {
        let result = extract_luau("package.loaded[\"mymod\"] = {}\n");
        assert_eq!(codes(&result, "E006").len(), 1);
    }

    #[test]
    fn package_loaded_field_write_flags_e006_on_luau() {
        let result = extract_luau("package.loaded.mymod = {}\n");
        assert_eq!(codes(&result, "E006").len(), 1);
    }

    #[test]
    fn duplicate_require_flags_w001() {
        let result = extract("local a = require(\"utils\")\nlocal b = require(\"utils\")\n");
        assert_eq!(result.requires.len(), 2);
        assert_eq!(codes(&result, "W001").len(), 1);
    }

    #[test]
    fn top_level_vararg_is_not_flagged() {
        // The loader calls each module with its real module name, so the
        // `local modname = ...` idiom keeps working - no W002.
        let result = extract("local modname = ...\nreturn modname\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn no_requires_yields_empty_result() {
        let result = extract("print(\"hello world\")\n");
        assert!(result.requires.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn require_string_call_syntax_is_extracted() {
        let result = extract("local m = require \"mymod\"\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "mymod");
    }

    #[test]
    fn single_quoted_require_is_extracted() {
        let result = extract("local m = require('mymod')\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "mymod");
    }

    #[test]
    fn long_bracket_require_is_extracted() {
        let result = extract("local m = require [[mymod]]\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "mymod");
    }

    #[test]
    fn require_in_multi_name_local_is_extracted() {
        let result = extract("local a, b = require(\"x\"), 1\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "x");
    }

    #[test]
    fn require_in_nested_function_is_extracted() {
        let result = extract("local function setup()\n    local m = require(\"inner\")\nend\n");
        assert_eq!(result.requires.len(), 1);
        assert_eq!(result.requires[0].require_string, "inner");
    }

    #[test]
    fn method_call_named_require_is_ignored() {
        let result = extract("local obj = {}\nfunction obj.require(s) end\nobj.require(\"x\")\n");
        assert!(result.requires.is_empty());
        assert!(
            codes(&result, "W005").is_empty(),
            "{:?}",
            result.diagnostics
        );
    }
}
