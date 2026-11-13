use luck_ast::{
    Block, Expression, FunctionArgs, FunctionBody, FunctionCall, Statement, Var, Visitor,
};
use luck_core::Diagnostic;
use luck_semantic::{ScopeId, ScopeKind, ScopeTree, StdlibEntry, SymbolId};
use luck_token::{LuaVersion, Span, StdlibEnvironment, decode_string};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dynamic_require::DynamicRequire;

/// A `require("literal")` call that reaches the global `require`.
#[derive(Debug)]
pub(crate) struct RequireSite {
    /// The require string with escapes decoded, as real `require` would
    /// receive it.
    pub require_string: String,
    pub call_span: Span,
}

pub(crate) struct ExtractedRequires {
    /// In source order.
    pub sites: Vec<RequireSite>,
    /// The `require` identifier of each call whose argument isn't a string
    /// literal. The emitter retargets just this token, so a static require
    /// nested in the argument still gets its own rewrite.
    pub dynamic_callees: Vec<Span>,
    pub diags: Vec<Diagnostic>,
    /// The module's own chunk reads `...`.
    pub has_vararg_read: bool,
    /// Lua 5.1: the module reads the global `arg`.
    pub has_global_arg_read: bool,
    /// Lua 5.2+: the module assigns its `_ENV` or calls `module`.
    pub has_env_rebinding: bool,
    /// Lua 5.1: the most locals any one function captures. Zero elsewhere.
    pub max_function_upvalues: usize,
}

/// A call that doesn't reach the global `require`, through a local binding
/// named `require`, a local `_ENV`, or a global the file replaced, keeps its
/// meaning and gets W005.
///
/// So does a reference to the global `require` that isn't a direct call,
/// since calls through it escape bundling, like `_G.require`. A standard
/// library's name is served from `package.loaded`, so it stays the host's. On
/// Luau, writes to `package.loaded` get E006.
///
/// `dynamic_require` decides whether a `require(expr)` aborts the bundle or
/// stays for the runtime.
pub(crate) fn extract_requires(
    block: &Block,
    file_path: &str,
    version: LuaVersion,
    dynamic_require: DynamicRequire,
) -> ExtractedRequires {
    let scope_tree = luck_semantic::build_scope_tree(block, version);
    let mut finder = RequireFinder {
        file_path,
        version,
        dynamic_require,
        scope_tree: &scope_tree,
        sites: Vec::new(),
        dynamic_callees: Vec::new(),
        diags: Vec::new(),
        package_loaded_writes: Vec::new(),
        seen_require_strings: FxHashSet::default(),
        direct_callee_spans: FxHashSet::default(),
        function_depth: 0,
        vararg_function_depth: 0,
        has_vararg_read: false,
        has_global_arg_read: false,
    };
    finder.visit_block(block);

    let RequireFinder {
        mut sites,
        dynamic_callees,
        mut diags,
        package_loaded_writes,
        direct_callee_spans,
        has_vararg_read,
        has_global_arg_read,
        ..
    } = finder;

    diags.extend(
        scope_tree
            .unresolved_references()
            .filter(|reference| {
                reference.name == "require"
                    && reference.kind.is_read()
                    && !direct_callee_spans.contains(&reference.span)
            })
            .map(|reference| Diagnostic::w005_aliased(file_path, reference.span)),
    );

    sites.sort_by_key(|site| site.call_span.start);
    diags.extend(package_loaded_writes);

    let has_env_rebinding = version.has_env_upvalue()
        && (scope_tree.is_env_assigned
            || scope_tree.unresolved_references().any(|reference| reference.name == "module"));
    // Lua 5.1: the only target with an upvalue limit a loader can hit.
    let max_function_upvalues = if version.has_env_upvalue() || version.is_luau() {
        0
    } else {
        max_function_upvalues(&scope_tree)
    };

    ExtractedRequires {
        sites,
        dynamic_callees,
        diags,
        has_vararg_read,
        has_global_arg_read,
        has_env_rebinding,
        max_function_upvalues,
    }
}

/// The most distinct outer locals any one function captures. A capture counts
/// on every function between the use and the declaring scope, as Lua's
/// compiler counts it.
fn max_function_upvalues(tree: &ScopeTree) -> usize {
    let mut captures: FxHashMap<ScopeId, FxHashSet<SymbolId>> = FxHashMap::default();
    for reference in &tree.references {
        let Some(symbol) = reference.resolved else {
            continue;
        };
        let home = tree.symbol(symbol).scope;
        let mut current = Some(reference.scope);
        while let Some(id) = current
            && id != home
        {
            let scope = tree.scope(id);
            if scope.kind == ScopeKind::Function {
                captures.entry(id).or_default().insert(symbol);
            }
            current = scope.parent;
        }
    }
    captures.values().map(FxHashSet::len).max().unwrap_or(0)
}

struct RequireFinder<'input> {
    file_path: &'input str,
    version: LuaVersion,
    dynamic_require: DynamicRequire,
    scope_tree: &'input ScopeTree,
    sites: Vec<RequireSite>,
    dynamic_callees: Vec<Span>,
    diags: Vec<Diagnostic>,
    /// E006s, which follow every other diagnostic.
    package_loaded_writes: Vec<Diagnostic>,
    seen_require_strings: FxHashSet<String>,
    direct_callee_spans: FxHashSet<Span>,
    /// Function bodies entered so far; zero is the module's own chunk.
    function_depth: usize,
    /// How many of those declare `...`, each binding its own `arg` on 5.1.
    vararg_function_depth: usize,
    has_vararg_read: bool,
    has_global_arg_read: bool,
}

impl RequireFinder<'_> {
    fn handle_call(&mut self, call: &FunctionCall) {
        let Some(callee_span) = require_callee_span(call) else {
            return;
        };
        self.direct_callee_spans.insert(callee_span);
        if !self.scope_tree.reaches_host_global("require", callee_span) {
            self.diags.push(Diagnostic::w005_shadowed(self.file_path, call.span));
            return;
        }

        match literal_require_string(call, self.version) {
            // The real `require` finds a standard library in `package.loaded`
            // before any searcher, so no file can stand in for it.
            Some(require_string) if is_standard_library(&require_string, self.version) => {}
            Some(require_string) => {
                if !self.seen_require_strings.insert(require_string.clone()) {
                    self.diags.push(Diagnostic::w001(self.file_path, call.span, &require_string));
                }
                self.sites.push(RequireSite { require_string, call_span: call.span });
            }
            // `require(expr)` can't be resolved statically. Aborting the whole
            // bundle over one such call is rarely what the author wants, so by
            // default it stays and resolves at runtime.
            None => self.handle_dynamic_call(call, callee_span),
        }
    }

    fn handle_dynamic_call(&mut self, call: &FunctionCall, callee_span: Span) {
        match self.dynamic_require {
            DynamicRequire::Error => {
                self.diags.push(Diagnostic::e002(self.file_path, call.span));
                return;
            }
            DynamicRequire::Warn if self.version.is_luau() => {
                self.diags.push(Diagnostic::w007_runtime(self.file_path, call.span));
            }
            DynamicRequire::Warn => {
                self.diags.push(Diagnostic::w007_loader_fallback(self.file_path, call.span));
            }
            DynamicRequire::Allow => {}
        }
        // Luau keys its cache by resolved file, and a Roblox require takes an
        // Instance, so a runtime argument has nothing in the bundle to look
        // up and the call stays as written.
        if !self.version.is_luau() {
            self.dynamic_callees.push(callee_span);
        }
    }

    /// A local named `package` is no concern of the bundle's.
    fn flag_package_loaded_write(&mut self, target: &Var) {
        if let Some(package_span) = package_loaded_root(target, self.version)
            && self.scope_tree.reaches_host_global("package", package_span)
        {
            self.package_loaded_writes.push(Diagnostic::e006(self.file_path, target.span()));
        }
    }

    /// W005 for `_G.require` or `_ENV.require`, the global `require` reached
    /// through its table rather than a direct call.
    fn flag_global_table_require(&mut self, var: &Var) {
        if let Some((table_name, table_span)) = global_table_require(var, self.version)
            && self.scope_tree.reaches_host_global(table_name, table_span)
        {
            self.diags.push(Diagnostic::w005_aliased(self.file_path, var.span()));
        }
    }
}

impl<'ast> Visitor<'ast> for RequireFinder<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        // Lua targets use package.loaded itself as the bundle cache, so writes
        // to it behave as in real Lua. Luau has no package table, and its
        // bundle cache is private.
        if self.version.is_luau() {
            if let Statement::Assignment(assignment) = stmt {
                for target in assignment.targets.iter() {
                    self.flag_package_loaded_write(target);
                }
            }
            if let Statement::CompoundAssignment(compound) = stmt {
                self.flag_package_loaded_write(&compound.var);
            }
        }
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if self.function_depth == 0 && matches!(expr, Expression::VarArg(_)) {
            self.has_vararg_read = true;
        }
        self.walk_expr(expr);
    }

    fn visit_var(&mut self, var: &'ast Var) {
        self.flag_global_table_require(var);
        if let Var::Name(token) = var
            && self.vararg_function_depth == 0
            && self.version.has_implicit_vararg_table()
            && token.identifier() == Some("arg")
            && !self.scope_tree.resolves_to_local("arg", token.span)
        {
            self.has_global_arg_read = true;
        }
        self.walk_var(var);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        let is_vararg = body.vararg.is_some();
        self.function_depth = self.function_depth.saturating_add(1);
        self.vararg_function_depth =
            self.vararg_function_depth.saturating_add(usize::from(is_vararg));
        self.walk_function_body(body);
        self.vararg_function_depth =
            self.vararg_function_depth.saturating_sub(usize::from(is_vararg));
        self.function_depth = self.function_depth.saturating_sub(1);
    }

    fn visit_function_call(&mut self, call: &'ast FunctionCall) {
        self.handle_call(call);
        self.walk_function_call(call);
    }
}

/// Whether the real `require` serves `name` straight from `package.loaded`.
/// Luau has no `package` table, so its `require` always resolves a path.
fn is_standard_library(name: &str, version: LuaVersion) -> bool {
    !version.is_luau()
        && (name == "_G"
            || matches!(
                luck_semantic::library_for(version, StdlibEnvironment::Standalone)
                    .globals
                    .get(name),
                Some(StdlibEntry::Namespace(_))
            ))
}

/// When `var` reads the field `require` of the global table (`_G`, or `_ENV`
/// on Lua 5.2+), that table's name and span, which decide whether the access
/// reaches the host's global table.
fn global_table_require(var: &Var, version: LuaVersion) -> Option<(&str, Span)> {
    let (prefix, is_require_key) = match var {
        Var::FieldAccess(field_access) => {
            (&field_access.prefix, field_access.name.identifier() == Some("require"))
        }
        Var::Index(index_expr) => {
            let is_require_string = matches!(
                &index_expr.index,
                Expression::StringLiteral(literal)
                    if decode_string(&literal.text, version).is_some_and(|v| *v == *b"require")
            );
            (&index_expr.prefix, is_require_string)
        }
        Var::Name(_) => return None,
    };
    if !is_require_key {
        return None;
    }
    let Expression::Var(Var::Name(token)) = prefix else {
        return None;
    };
    let table_name = token.identifier()?;
    (table_name == "_G" || (table_name == "_ENV" && version.has_env_upvalue()))
        .then_some((table_name, token.span))
}

/// The callee token span when `call` is a direct, non-method call of a
/// variable named `require` (whatever that name resolves to).
fn require_callee_span(call: &FunctionCall) -> Option<Span> {
    if call.method.is_some() {
        return None;
    }
    let Expression::Var(Var::Name(token)) = &call.callee else {
        return None;
    };
    (token.identifier() == Some("require")).then_some(token.span)
}

/// Escapes are resolved and a long string's leading newline is stripped.
/// `None` if it doesn't decode or the result isn't UTF-8.
fn literal_require_string(call: &FunctionCall, version: LuaVersion) -> Option<String> {
    let literal = match &call.args {
        FunctionArgs::Parenthesized { args, .. } => {
            let [Expression::StringLiteral(literal)] = args.items.as_slice() else {
                return None;
            };
            literal
        }
        FunctionArgs::StringLiteral(literal) => literal,
        FunctionArgs::TableConstructor(_) => return None,
    };
    String::from_utf8(decode_string(&literal.text, version)?.into_owned()).ok()
}

/// The span of the `package` name when an assignment target is, or indexes
/// into, `package.loaded` or `package["loaded"]`.
fn package_loaded_root(target: &Var, version: LuaVersion) -> Option<Span> {
    let mut var = target;
    loop {
        let (prefix, is_loaded_key) = match var {
            Var::FieldAccess(field_access) => {
                (&field_access.prefix, field_access.name.identifier() == Some("loaded"))
            }
            Var::Index(index_expr) => {
                let is_loaded_string = matches!(
                    &index_expr.index,
                    Expression::StringLiteral(literal)
                        if decode_string(&literal.text, version).is_some_and(|v| *v == *b"loaded")
                );
                (&index_expr.prefix, is_loaded_string)
            }
            Var::Name(_) => return None,
        };
        if is_loaded_key
            && let Expression::Var(Var::Name(token)) = prefix
            && token.identifier() == Some("package")
        {
            return Some(token.span);
        }
        let Expression::Var(prefix_var) = prefix else {
            return None;
        };
        var = prefix_var;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, version: LuaVersion) -> Block {
        let result = luck_parser::parse(source, version);
        assert!(result.errors.is_empty(), "parse failed: {:?}", result.errors);
        result.block
    }

    fn extract(source: &str) -> ExtractedRequires {
        extract_with(source, DynamicRequire::default())
    }

    fn extract_with(source: &str, dynamic_require: DynamicRequire) -> ExtractedRequires {
        extract_requires(
            &parse(source, LuaVersion::Lua54),
            "test.lua",
            LuaVersion::Lua54,
            dynamic_require,
        )
    }

    fn extract_luau(source: &str) -> ExtractedRequires {
        extract_requires(
            &parse(source, LuaVersion::Luau),
            "test.luau",
            LuaVersion::Luau,
            DynamicRequire::default(),
        )
    }

    fn require_strings(result: &ExtractedRequires) -> Vec<&str> {
        result.sites.iter().map(|site| site.require_string.as_str()).collect()
    }

    fn with_code<'result>(
        result: &'result ExtractedRequires,
        code: &str,
    ) -> Vec<&'result Diagnostic> {
        result.diags.iter().filter(|diag| diag.code == code).collect()
    }

    #[test]
    fn extracts_single_require() {
        let result = extract("local utils = require(\"utils\")\nprint(utils.foo())\n");
        assert_eq!(require_strings(&result), ["utils"]);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn extracts_multiple_requires_in_order() {
        let result = extract(
            "local a = require(\"mod_a\")\nlocal b = require(\"mod_b\")\nlocal c = require(\"mod_c\")\nprint(a, b, c)\n",
        );
        assert_eq!(require_strings(&result), ["mod_a", "mod_b", "mod_c"]);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn require_after_code_is_not_flagged() {
        // Requires are position-independent with the lazy loader.
        let result = extract("print(\"hello\")\nlocal x = require(\"x\")\n");
        assert_eq!(result.sites.len(), 1);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn non_literal_require_flags_e002_in_error_mode() {
        let result = extract_with("local x = require(varname)\n", DynamicRequire::Error);
        assert!(result.sites.is_empty());
        assert_eq!(result.dynamic_callees, Vec::<Span>::new());
        assert_eq!(with_code(&result, "E002").len(), 1, "{:?}", result.diags);
    }

    /// The default: one runtime-resolved require must not abort the bundle.
    #[test]
    fn non_literal_require_warns_and_is_kept_by_default() {
        let result = extract("local x = require(varname)\n");
        assert!(result.sites.is_empty());
        assert!(with_code(&result, "E002").is_empty(), "{:?}", result.diags);
        assert_eq!(with_code(&result, "W007").len(), 1, "{:?}", result.diags);
        // The callee token alone, so the argument expression is left intact.
        assert_eq!(result.dynamic_callees, [Span::new(10, 17)]);
    }

    #[test]
    fn allow_mode_keeps_the_call_without_warning() {
        let result = extract_with("local x = require(varname)\n", DynamicRequire::Allow);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
        assert_eq!(result.dynamic_callees, [Span::new(10, 17)]);
    }

    /// Luau keys its cache by resolved file and Roblox requires take an
    /// Instance, so a runtime argument has nothing to look up. The call warns
    /// but is never retargeted.
    #[test]
    fn luau_dynamic_require_is_warned_but_not_retargeted() {
        let result = extract_luau("local x = require(varname)\n");
        assert_eq!(with_code(&result, "W007").len(), 1, "{:?}", result.diags);
        assert_eq!(result.dynamic_callees, Vec::<Span>::new());
    }

    /// A static require nested inside a dynamic one still bundles: the
    /// retargeted span is the outer callee only.
    #[test]
    fn static_require_nested_in_a_dynamic_one_is_still_extracted() {
        let result = extract("local x = require(require(\"names\").first)\n");
        assert_eq!(require_strings(&result), ["names"]);
        assert_eq!(result.dynamic_callees, [Span::new(10, 17)]);
    }

    #[test]
    fn bare_require_stmt_is_extracted() {
        let result = extract("require(\"something\")\n");
        assert_eq!(require_strings(&result), ["something"]);
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn escape_sequences_decode_like_real_lua() {
        // require("a\46b") is require("a.b") at runtime; the bundler must
        // resolve and deduplicate on the decoded value.
        let result = extract("local a = require(\"a\\46b\")\nlocal b = require(\"a.b\")\n");
        assert_eq!(require_strings(&result), ["a.b", "a.b"]);
        // Same decoded module twice: the duplicate-require warning fires.
        assert_eq!(with_code(&result, "W001").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn long_string_leading_newline_is_stripped() {
        let result = extract("local m = require [[\nmymod]]\n");
        assert_eq!(require_strings(&result), ["mymod"]);
    }

    #[test]
    fn shadowed_require_is_skipped_with_w005() {
        let source = "local require = function(s) return s end\nlocal x = require(\"dep\")\n";
        let result = extract(source);
        assert!(
            result.sites.is_empty(),
            "shadowed require must not be bundled: {:?}",
            result.sites
        );
        assert_eq!(with_code(&result, "W005").len(), 1, "{:?}", result.diags);
        assert!(with_code(&result, "E002").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn shadowed_require_in_inner_scope_only_skips_there() {
        let source = "local a = require(\"real\")\nlocal function f()\n    local require = print\n    require(\"fake\")\nend\n";
        let result = extract(source);
        assert_eq!(require_strings(&result), ["real"]);
        assert_eq!(with_code(&result, "W005").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn aliased_require_flags_w005() {
        let result = extract("local r = require\nlocal x = r(\"dep\")\n");
        assert!(result.sites.is_empty());
        assert_eq!(with_code(&result, "W005").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn direct_calls_do_not_flag_w005() {
        let result = extract("local a = require(\"x\")\nrequire(\"y\")\n");
        assert_eq!(result.sites.len(), 2);
        assert!(with_code(&result, "W005").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn package_loaded_write_is_allowed_on_lua_targets() {
        // On Lua targets the bundle cache is package.loaded itself, so
        // writes to it behave as in real Lua and need no E006.
        let result = extract("package.loaded[\"mymod\"] = {}\npackage.loaded.other = {}\n");
        assert!(with_code(&result, "E006").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn package_loaded_index_write_flags_e006_on_luau() {
        let result = extract_luau("package.loaded[\"mymod\"] = {}\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn package_loaded_field_write_flags_e006_on_luau() {
        let result = extract_luau("package.loaded.mymod = {}\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn package_loaded_compound_assignment_flags_e006_on_luau() {
        let result = extract_luau("package.loaded.x ..= \"a\"\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn nested_package_loaded_compound_assignment_flags_e006_on_luau() {
        let result = extract_luau("package.loaded[\"x\"].y += 1\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn compound_assignment_to_another_table_is_not_flagged_on_luau() {
        let result = extract_luau("loaded.x ..= \"a\"\n");
        assert!(with_code(&result, "E006").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn local_named_package_is_not_flagged_on_luau() {
        let result = extract_luau(
            "local package = { loaded = {} }\npackage.loaded.x = 1\npackage.loaded.y ..= \"a\"\n",
        );
        assert!(with_code(&result, "E006").is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn package_write_outside_a_local_named_package_is_flagged_on_luau() {
        let result = extract_luau("do local package = {} end\npackage.loaded.x = 1\n");
        assert_eq!(with_code(&result, "E006").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn duplicate_require_flags_w001() {
        let result = extract("local a = require(\"utils\")\nlocal b = require(\"utils\")\n");
        assert_eq!(result.sites.len(), 2);
        assert_eq!(with_code(&result, "W001").len(), 1, "{:?}", result.diags);
    }

    #[test]
    fn top_level_vararg_is_not_flagged() {
        // The loader passes each module its real name, so the
        // `local modname = ...` idiom works and needs no warning.
        let result = extract("local modname = ...\nreturn modname\n");
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn lua51_global_arg_read_ignores_a_vararg_functions_own_arg() {
        let extract_arg_read = |source: &str| {
            extract_requires(
                &luck_parser::parse(source, LuaVersion::Lua51).block,
                "test.lua",
                LuaVersion::Lua51,
                DynamicRequire::default(),
            )
            .has_global_arg_read
        };
        assert!(extract_arg_read("return arg\n"));
        assert!(extract_arg_read("local function f() return arg end\nreturn f\n"));
        assert!(!extract_arg_read("local function f(...) return arg end\nreturn f\n"));
        assert!(!extract_arg_read(
            "local function f(...) return function() return arg end end\nreturn f\n"
        ));
        assert!(!extract_arg_read("local arg = {}\nreturn arg\n"));
    }

    #[test]
    fn no_requires_yields_empty_result() {
        let result = extract("print(\"hello world\")\n");
        assert!(result.sites.is_empty());
        assert!(result.diags.is_empty(), "{:?}", result.diags);
    }

    #[test]
    fn require_string_call_syntax_is_extracted() {
        let result = extract("local m = require \"mymod\"\n");
        assert_eq!(require_strings(&result), ["mymod"]);
    }

    #[test]
    fn single_quoted_require_is_extracted() {
        let result = extract("local m = require('mymod')\n");
        assert_eq!(require_strings(&result), ["mymod"]);
    }

    #[test]
    fn long_bracket_require_is_extracted() {
        let result = extract("local m = require [[mymod]]\n");
        assert_eq!(require_strings(&result), ["mymod"]);
    }

    #[test]
    fn require_in_multi_name_local_is_extracted() {
        let result = extract("local a, b = require(\"x\"), 1\n");
        assert_eq!(require_strings(&result), ["x"]);
    }

    #[test]
    fn require_in_nested_function_is_extracted() {
        let result = extract("local function setup()\n    local m = require(\"inner\")\nend\n");
        assert_eq!(require_strings(&result), ["inner"]);
    }

    #[test]
    fn field_function_named_require_is_ignored() {
        let result = extract("local obj = {}\nfunction obj.require(s) end\nobj.require(\"x\")\n");
        assert!(result.sites.is_empty());
        assert!(with_code(&result, "W005").is_empty(), "{:?}", result.diags);
    }
}
