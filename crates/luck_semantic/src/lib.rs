//! # luck_semantic
//!
//! Scope analysis, symbol resolution, and standard library definitions for Lua/Luau.
//!
//! Provides a `ScopeTree` that tracks variable declarations, references,
//! shadowing, and upvalue captures, plus the rich stdlib data model in
//! `stdlib_model`. Rules go through `SemanticAnalysis::lookup_stdlib_str`
//! for typed metadata and `SemanticAnalysis::is_known_global` for the
//! "is this name a stdlib or user-configured global?" check.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//!
//! let parsed = luck_parser::parse("local greeting = 1", LuaVersion::Lua54);
//! let analysis = luck_semantic::analyze(&parsed.block, LuaVersion::Lua54);
//! assert!(analysis.is_known_global("print"));
//! ```

pub mod builder;
pub mod nodes;
pub mod scope;
pub mod stdlib_model;

use std::collections::HashSet;

use compact_str::CompactString;
use luck_ast::shared::Block;
use luck_token::{LuaVersion, StdlibEnvironment};

use builder::ScopeTreeBuilder;
use scope::ScopeTree;
use stdlib_model::{EntryKind, StdlibDeprecation, StdlibEntry, StdlibLibrary, library_for};

/// Complete semantic analysis result for a Lua chunk.
#[derive(Debug)]
pub struct SemanticAnalysis {
    pub scope_tree: ScopeTree,
    /// Names treated as globals beyond what the stdlib defines. The
    /// linter driver fills this from the user's `extra_globals` config
    /// (e.g. `vim`, project-specific runtime names). Stdlib names are
    /// not duplicated here - query them via `is_known_global` or
    /// `stdlib()` directly.
    pub extra_globals: HashSet<String>,
    pub version: LuaVersion,
    /// Selects the stdlib environment for visibility: `Roblox` exposes
    /// Roblox-tier entries and hides standalone-only ones; `Standalone`
    /// does the reverse.
    pub environment: StdlibEnvironment,
}

/// Analyze a parsed Lua block, building the scope tree. Defaults to the
/// [`StdlibEnvironment::Standalone`] environment - the correct default for
/// vanilla Lua and standalone Luau. Luau callers targeting Roblox must use
/// [`analyze_with_environment`] with [`StdlibEnvironment::Roblox`].
pub fn analyze(block: &Block, version: LuaVersion) -> SemanticAnalysis {
    analyze_with_environment(block, version, StdlibEnvironment::Standalone)
}

/// Like [`analyze`] but selects the stdlib environment explicitly. Only
/// affects stdlib visibility, not scope analysis.
pub fn analyze_with_environment(
    block: &Block,
    version: LuaVersion,
    environment: StdlibEnvironment,
) -> SemanticAnalysis {
    let scope_tree = ScopeTreeBuilder::new().build(block);

    SemanticAnalysis {
        scope_tree,
        extra_globals: HashSet::new(),
        version,
        environment,
    }
}

impl SemanticAnalysis {
    /// The rich, typed library for this analysis's Lua version.
    #[must_use]
    pub fn stdlib(&self) -> &'static StdlibLibrary {
        library_for(self.version)
    }

    /// Look up a dotted path (e.g. `["table", "concat"]`) in the stdlib.
    #[must_use]
    pub fn lookup_stdlib(&self, path: &[CompactString]) -> Option<&'static StdlibEntry> {
        self.stdlib()
            .lookup(path)
            .filter(|entry| entry.available_in_luau(self.environment))
    }

    /// Same as [`Self::lookup_stdlib`] but accepts borrowed `&str`
    /// segments. Preferred when callers already have raw source slices.
    #[must_use]
    pub fn lookup_stdlib_str(&self, path: &[&str]) -> Option<&'static StdlibEntry> {
        self.stdlib()
            .lookup_str(path)
            .filter(|entry| entry.available_in_luau(self.environment))
    }

    /// True when the identifier occupying `span` resolves to a local
    /// binding rather than a global. Rules that pattern-match stdlib
    /// paths (`table.insert`, `require(...)`, `error(...)`) must bail
    /// when the base name is shadowed (`local table = {}`), or they
    /// report stdlib diagnostics against user values.
    #[must_use]
    pub fn resolves_to_local(&self, name: &str, span: luck_token::Span) -> bool {
        self.scope_tree.references.iter().any(|reference| {
            reference.span == span && reference.name == name && reference.resolved.is_some()
        })
    }

    /// Whether `name` is recognized as a global - either a stdlib entry
    /// for this version, or an entry in `extra_globals`.
    #[must_use]
    pub fn is_known_global(&self, name: &str) -> bool {
        self.stdlib()
            .globals
            .get(name)
            .is_some_and(|entry| entry.available_in_luau(self.environment))
            || self.extra_globals.contains(name)
    }

    /// Whether the named call path is marked `must_use`. Path with a
    /// single segment is a bare global; two segments hit a namespace
    /// member.
    #[must_use]
    pub fn is_must_use(&self, call_path: &[CompactString]) -> bool {
        match self.lookup_stdlib(call_path) {
            Some(entry) => match &entry.kind {
                EntryKind::Function(func) => func.must_use,
                _ => false,
            },
            None => false,
        }
    }

    /// Deprecation info attached to the entry at this path, if any.
    #[must_use]
    pub fn deprecation_info(&self, path: &[CompactString]) -> Option<&'static StdlibDeprecation> {
        let entry = self.lookup_stdlib(path)?;
        match &entry.kind {
            EntryKind::Function(func) => func.deprecated.as_ref(),
            EntryKind::Constant(value) | EntryKind::Property(value) => value.deprecated.as_ref(),
            EntryKind::Namespace(_) => None,
        }
    }

    /// Whether a bare-word global is read-only (i.e. should not be
    /// reassigned). Namespaces and constants both qualify.
    #[must_use]
    pub fn is_read_only_global(&self, name: &str) -> bool {
        let lib = self.stdlib();
        match lib
            .globals
            .get(name)
            .filter(|entry| entry.available_in_luau(self.environment))
        {
            Some(entry) => match &entry.kind {
                EntryKind::Function(func) => func.read_only,
                EntryKind::Constant(value) | EntryKind::Property(value) => value.read_only,
                EntryKind::Namespace(_) => true,
            },
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_analyze(source: &str) -> SemanticAnalysis {
        let result = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(
            result.errors.is_empty(),
            "parse errors: {:?}",
            result.errors
        );
        analyze(&result.block, LuaVersion::Lua54)
    }

    #[test]
    fn local_declaration_creates_symbol() {
        let analysis = parse_and_analyze("local x = 1");
        assert_eq!(analysis.scope_tree.symbols.len(), 1);
        assert_eq!(analysis.scope_tree.symbols[0].name, "x");
    }

    #[test]
    fn reference_resolves_to_local() {
        let analysis = parse_and_analyze("local x = 1\nprint(x)");
        let x_refs: Vec<_> = analysis
            .scope_tree
            .references
            .iter()
            .filter(|r| r.name == "x")
            .collect();
        assert_eq!(x_refs.len(), 1);
        assert!(x_refs[0].resolved.is_some());
    }

    #[test]
    fn global_reference_is_unresolved() {
        let analysis = parse_and_analyze("print(hello)");
        let unresolved: Vec<_> = analysis.scope_tree.unresolved_references().collect();
        let names: Vec<_> = unresolved.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"print"));
        assert!(names.contains(&"hello"));
    }

    #[test]
    fn function_body_creates_scope() {
        let analysis = parse_and_analyze("local function f(a, b) return a + b end");
        assert!(analysis.scope_tree.scopes.len() >= 2);
        assert_eq!(analysis.scope_tree.symbols.len(), 3);
    }

    #[test]
    fn upvalue_detection() {
        let analysis = parse_and_analyze("local x = 1\nlocal function f() return x end");
        let x_sym = &analysis.scope_tree.symbols[0];
        assert_eq!(x_sym.name, "x");
        assert!(x_sym.is_upvalue);
    }

    #[test]
    fn shadowing_detection() {
        let analysis = parse_and_analyze("local x = 1\ndo\n  local x = 2\nend");
        assert_eq!(analysis.scope_tree.symbols.len(), 2);
        let inner_x = &analysis.scope_tree.symbols[1];
        assert_eq!(inner_x.name, "x");
        assert!(inner_x.shadows.is_some());
    }

    #[test]
    fn for_loop_variables() {
        let analysis = parse_and_analyze("for i = 1, 10 do print(i) end");
        let i_sym = analysis
            .scope_tree
            .symbols
            .iter()
            .find(|s| s.name == "i")
            .unwrap();
        assert_eq!(i_sym.kind, scope::SymbolKind::NumericForVariable);
    }

    #[test]
    fn generic_for_variables() {
        let analysis = parse_and_analyze("for k, v in pairs(t) do print(k, v) end");
        let iter_vars: Vec<_> = analysis
            .scope_tree
            .symbols
            .iter()
            .filter(|s| s.kind == scope::SymbolKind::IteratorVariable)
            .collect();
        assert_eq!(iter_vars.len(), 2);
    }

    #[test]
    fn named_vararg_declares_symbol() {
        // Lua 5.5: `...name` binds the vararg table to a name.
        let result = luck_parser::parse(
            "local function f(...args) return args end",
            LuaVersion::Lua55,
        );
        assert!(
            result.errors.is_empty(),
            "parse errors: {:?}",
            result.errors
        );
        let analysis = analyze(&result.block, LuaVersion::Lua55);
        let args_sym = analysis
            .scope_tree
            .symbols
            .iter()
            .find(|s| s.name == "args")
            .expect("named vararg must be declared");
        assert_eq!(args_sym.kind, scope::SymbolKind::Parameter);
        let args_refs: Vec<_> = analysis
            .scope_tree
            .references
            .iter()
            .filter(|r| r.name == "args")
            .collect();
        assert_eq!(args_refs.len(), 1);
        assert!(
            args_refs[0].resolved == Some(args_sym.id),
            "reference must resolve to the vararg binding"
        );
    }

    #[test]
    fn unused_variable_detection() {
        let analysis = parse_and_analyze("local unused = 1\nlocal used = 2\nprint(used)");
        let unused: Vec<_> = analysis.scope_tree.unused_symbols().collect();
        let names: Vec<_> = unused.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"unused"));
        assert!(!names.contains(&"used"));
    }

    #[test]
    fn is_known_global_hits_stdlib() {
        let analysis = parse_and_analyze("local x = 1");
        assert!(analysis.is_known_global("print"));
        assert!(analysis.is_known_global("table"));
        assert!(!analysis.is_known_global("not_a_real_name"));
    }

    #[test]
    fn is_known_global_honours_extra_globals() {
        let mut analysis = parse_and_analyze("local x = 1");
        assert!(!analysis.is_known_global("vim"));
        analysis.extra_globals.insert("vim".to_string());
        assert!(analysis.is_known_global("vim"));
    }

    #[test]
    fn method_declares_self() {
        let source = "local t = {}\nfunction t:method() return self end";
        let result = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(result.errors.is_empty());
        let analysis = analyze(&result.block, LuaVersion::Lua54);
        let self_sym = analysis
            .scope_tree
            .symbols
            .iter()
            .find(|s| s.name == "self");
        assert!(
            self_sym.is_some(),
            "self should be declared as a symbol in method scope"
        );
    }

    #[test]
    fn non_method_no_self() {
        let source = "local t = {}\nfunction t.method() end";
        let result = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(result.errors.is_empty());
        let analysis = analyze(&result.block, LuaVersion::Lua54);
        let self_sym = analysis
            .scope_tree
            .symbols
            .iter()
            .find(|s| s.name == "self");
        assert!(
            self_sym.is_none(),
            "self should not be declared in non-method function"
        );
    }

    #[test]
    fn repeat_until_condition_sees_block_locals() {
        let analysis = parse_and_analyze("repeat\n  local done = true\nuntil done");
        let done_refs: Vec<_> = analysis
            .scope_tree
            .references
            .iter()
            .filter(|r| r.name == "done")
            .collect();
        assert_eq!(done_refs.len(), 1);
        assert!(
            done_refs[0].resolved.is_some(),
            "repeat-until condition should resolve locals from the loop body"
        );
    }

    #[test]
    fn for_loop_variable_not_visible_outside() {
        let analysis = parse_and_analyze("for i = 1, 10 do end\nprint(i)");
        let outer_i_ref = analysis
            .scope_tree
            .references
            .iter()
            .find(|r| r.name == "i" && r.resolved.is_none());
        assert!(
            outer_i_ref.is_some(),
            "for-loop variable i should not be visible outside the loop"
        );
    }

    #[test]
    fn upvalue_across_nested_functions() {
        let analysis = parse_and_analyze(
            "local x = 1\nlocal function outer()\n  local function inner() return x end\nend",
        );
        let x_sym = analysis
            .scope_tree
            .symbols
            .iter()
            .find(|s| s.name == "x")
            .unwrap();
        assert!(
            x_sym.is_upvalue,
            "x should be detected as upvalue when captured by nested inner function"
        );
    }

    #[test]
    fn generic_for_variables_not_visible_outside() {
        let analysis = parse_and_analyze("for k, v in pairs(t) do end\nprint(k)");
        let outer_k = analysis
            .scope_tree
            .references
            .iter()
            .find(|r| r.name == "k" && r.resolved.is_none());
        assert!(
            outer_k.is_some(),
            "generic for variables should not be visible outside the loop"
        );
    }
}

#[cfg(test)]
mod flavor_tests {
    use super::*;
    #[test]
    fn roblox_globals_hidden_in_standalone() {
        let parsed = luck_parser::parse("print(game)", LuaVersion::Luau);
        let standalone = analyze_with_environment(
            &parsed.block,
            LuaVersion::Luau,
            StdlibEnvironment::Standalone,
        );
        let roblox =
            analyze_with_environment(&parsed.block, LuaVersion::Luau, StdlibEnvironment::Roblox);
        assert!(!standalone.is_known_global("game"));
        assert!(roblox.is_known_global("game"));
        assert!(standalone.is_known_global("bit32")); // core: both
        assert!(roblox.is_known_global("bit32"));
    }
}
