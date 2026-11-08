//! Constructors for every numbered diagnostic, the only place their codes and
//! wording are written.
//!
//! E001, E003, E005, and W002 are never emitted, and their numbers stay
//! reserved.

use luck_token::Span;

use super::Diagnostic;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "every numbered code and its wording stay in this file"
)]
impl Diagnostic {
    #[must_use]
    pub fn e002(file_path: &str, span: Span) -> Self {
        Self::error_at("E002", "`require` argument must be a string literal", file_path, span)
            .with_help("use a string literal: `require(\"module_name\")`")
    }

    #[must_use]
    pub fn e004(file_path: &str, span: Span, module_name: &str, tried_paths: &[String]) -> Self {
        let paths_list = tried_paths.join("\n  - ");
        Self::error_at("E004", format!("module not found: `{module_name}`"), file_path, span)
            .with_help(format!("searched paths:\n  - {paths_list}"))
    }

    /// A Luau require string that doesn't begin with `./`, `../`, or `@`.
    #[must_use]
    pub fn e004_luau_scheme(file_path: &str, span: Span, module_name: &str) -> Self {
        Self::error_at(
            "E004",
            format!(
                "module not found: `{module_name}` (Luau requires must start with `./`, `../`, \
                 or `@`)"
            ),
            file_path,
            span,
        )
        .with_help(
            "Luau requires must use relative paths (`./foo`, `../bar`) or aliases (`@alias/foo`)",
        )
    }

    /// A bare `@self` require with no subpath.
    #[must_use]
    pub fn e004_self_needs_subpath(file_path: &str, span: Span) -> Self {
        Self::error_at(
            "E004",
            "`@self` can't be used alone, so use `@self/subpath`",
            file_path,
            span,
        )
    }

    #[must_use]
    pub fn e006(file_path: &str, span: Span) -> Self {
        Self::error_at("E006", "`package.loaded` manipulation is not allowed", file_path, span)
            .with_help(
                "Luau has no `package.loaded`, and the bundle's module cache is private, so \
                 remove the write",
            )
    }

    #[must_use]
    pub fn e007(file_path: &str, span: Span, details: &str) -> Self {
        Self::error_at("E007", "ambiguous module resolution", file_path, span)
            .with_help(format!("ambiguity: {details}, so remove one of the conflicting files"))
    }

    #[must_use]
    pub fn e008(file_path: &str, span: Span, parse_error: &str) -> Self {
        Self::error_at("E008", format!("parse error: {parse_error}"), file_path, span)
    }

    #[must_use]
    pub fn e009(file_path: &str, span: Span, limit: usize) -> Self {
        Self::error_at(
            "E009",
            format!("too many modules: exceeded {limit} module limit"),
            file_path,
            span,
        )
        .with_help(format!(
            "a bundle may contain at most {limit} modules, so reduce the dependency graph or \
             split the build"
        ))
    }

    #[must_use]
    pub fn e010(file_path: &str, span: Span, io_error: &str) -> Self {
        Self::error_at(
            "E010",
            format!("cannot read file `{file_path}`: {io_error}"),
            file_path,
            span,
        )
        .with_help("check that the file exists and is readable")
    }

    #[must_use]
    pub fn e011(entry_path: &str, span: Span) -> Self {
        Self::error_at("E011", format!("entry file not found: `{entry_path}`"), entry_path, span)
            .with_help("check that the entry path points at an existing file")
    }

    #[must_use]
    pub fn e012(file_path: &str, span: Span, byte_len: usize, limit: usize) -> Self {
        Self::error_at(
            "E012",
            format!("file too large: {byte_len} bytes (limit: {limit})"),
            file_path,
            span,
        )
        .with_help(format!("files must be at most {limit} bytes"))
    }

    #[must_use]
    pub fn w001(file_path: &str, span: Span, module_name: &str) -> Self {
        Self::warning_at(
            "W001",
            format!("module `{module_name}` required multiple times"),
            file_path,
            span,
        )
    }

    #[must_use]
    pub fn w003(file_path: &str, span: Span, cycle_path: &[String]) -> Self {
        let quoted_paths: Vec<String> = cycle_path.iter().map(|path| format!("`{path}`")).collect();
        let cycle = quoted_paths.join(" -> ");
        Self::warning_at("W003", "circular dependency between modules", file_path, span).with_help(
            format!(
                "cycle: {cycle}\nmodules load lazily, so a cycle through a function body works, \
                 but one hit while a module is still loading fails at runtime, as it would \
                 without bundling"
            ),
        )
    }

    #[must_use]
    pub fn w004(file_path: &str, span: Span) -> Self {
        Self::warning_at(
            "W004",
            "alias `self` defined in `.luaurc` is shadowed by built-in `@self`",
            file_path,
            span,
        )
    }

    /// A call through a local binding named `require`; left untouched.
    #[must_use]
    pub fn w005_shadowed(file_path: &str, span: Span) -> Self {
        Self::warning_at(
            "W005",
            "call goes through a local binding named `require` and is not bundled",
            file_path,
            span,
        )
        .with_help(
            "the call is left exactly as written and will invoke the local function at runtime",
        )
    }

    /// The global `require` referenced without being directly called.
    #[must_use]
    pub fn w005_aliased(file_path: &str, span: Span) -> Self {
        Self::warning_at(
            "W005",
            "global `require` is referenced without being called, so aliased call sites are not \
             bundled",
            file_path,
            span,
        )
        .with_help(
            "only direct `require(\"literal\")` calls are bundled, and calls through an alias \
             like `local r = require` fall through to the runtime's own `require`",
        )
    }

    #[must_use]
    pub fn w006(file_path: &str, span: Span, hot_comment: &str) -> Self {
        Self::warning_at(
            "W006",
            format!("hot comment `{hot_comment}` has no effect in a bundle"),
            file_path,
            span,
        )
        .with_help(
            "Luau hot comments only apply at the top of a chunk, so only the entry module's are \
             hoisted above the bundle's loader",
        )
    }

    /// A `require(expr)` routed through the bundle's loader.
    ///
    /// Lua targets only: the cache is keyed by module name, so a name computed at
    /// runtime can still find a bundled module.
    #[must_use]
    pub fn w007_loader_fallback(file_path: &str, span: Span) -> Self {
        Self::warning_at(
            "W007",
            "`require` argument is not a string literal, so the module is resolved at runtime",
            file_path,
            span,
        )
        .with_help(
            "the call goes through the bundle's loader, which serves bundled modules by name \
             and passes any other name to the runtime's own `require`, so use a string literal \
             to bundle the module itself",
        )
    }

    /// A `require(expr)` left exactly as written.
    ///
    /// Luau targets only: the bundle's cache is keyed by resolved file, and Roblox
    /// requires take an Instance, so a runtime argument can't name a bundled
    /// module.
    #[must_use]
    pub fn w007_runtime(file_path: &str, span: Span) -> Self {
        Self::warning_at(
            "W007",
            "`require` argument is not a string literal, so the module is resolved at runtime",
            file_path,
            span,
        )
        .with_help(
            "the call is left exactly as written and resolves against the runtime, not the \
             bundle, so use a string literal to bundle the module itself",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_code_severity_and_help() {
        let e2 = Diagnostic::e002("f.lua", Span::new(0, 10));
        assert_eq!(e2.code, "E002");
        assert_eq!(e2.file_path, "f.lua");
        assert!(e2.is_error());

        let e4 =
            Diagnostic::e004("f.lua", Span::new(0, 10), "mymod", &["src/mymod.lua".to_owned()]);
        assert!(e4.help.as_deref().expect("e004 always sets help").contains("src/mymod.lua"));

        let w1 = Diagnostic::w001("f.lua", Span::new(0, 5), "utils");
        assert_eq!(w1.code, "W001");
        assert!(!w1.is_error());
        assert!(w1.help.is_none());
    }
}
