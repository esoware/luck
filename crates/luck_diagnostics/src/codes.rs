//! E001, E003, E005, E006, E011, and W002 are never emitted, and their
//! numbers stay reserved.

use alloc::borrow::Cow;

use luck_token::Span;

use crate::diagnostic::{Diagnostic, Severity, Tag};

impl Diagnostic {
    fn error_at<Message>(code: &'static str, span: Option<Span>, message: Message) -> Self
    where
        Message: Into<Cow<'static, str>>,
    {
        Self::new(code, Severity::Error, message.into(), span)
    }

    fn warning_at<Message>(code: &'static str, span: Span, message: Message) -> Self
    where
        Message: Into<Cow<'static, str>>,
    {
        Self::new(code, Severity::Warning, message.into(), Some(span))
    }

    pub fn e002(span: Span) -> Self {
        Self::error_at("E002", Some(span), "`require` argument must be a string literal")
            .with_help("use a string literal: `require(\"module_name\")`")
    }

    /// `tried_paths` holds the candidate files in probe order, unquoted.
    pub fn e004(span: Span, module_name: &str, tried_paths: &[String]) -> Self {
        let quoted_paths: Vec<String> =
            tried_paths.iter().map(|path| format!("`{path}`")).collect();
        Self::module_not_found(span, module_name, &quoted_paths.join("\n  - "))
    }

    pub fn e004_unknown_alias(span: Span, module_name: &str, alias_name: &str) -> Self {
        Self::module_not_found(
            span,
            module_name,
            &format!("alias `@{alias_name}` not found in any `.luaurc`"),
        )
    }

    pub fn e004_malformed_luaurc(span: Span, module_name: &str, luaurc_path: &str) -> Self {
        Self::module_not_found(
            span,
            module_name,
            &format!("`{luaurc_path}`, which failed to parse"),
        )
    }

    /// `cycle` holds the alias names in order, without their `@`.
    pub fn e004_alias_cycle(span: Span, module_name: &str, cycle: &[String]) -> Self {
        let quoted_aliases: Vec<String> = cycle.iter().map(|alias| format!("`@{alias}`")).collect();
        Self::module_not_found(
            span,
            module_name,
            &format!("alias cycle: {}", quoted_aliases.join(" -> ")),
        )
    }

    fn module_not_found(span: Span, module_name: &str, searched: &str) -> Self {
        Self::error_at("E004", Some(span), format!("module not found: `{module_name}`"))
            .with_help(format!("searched paths:\n  - {searched}"))
    }

    pub fn e004_missing_prefix(span: Span, module_name: &str) -> Self {
        Self::error_at(
            "E004",
            Some(span),
            format!(
                "module not found: `{module_name}` (Luau requires must start with `./`, `../`, \
                 or `@`)"
            ),
        )
        .with_help(
            "Luau requires must use relative paths (`./foo`, `../bar`) or aliases (`@alias/foo`)",
        )
    }

    pub fn e004_bare_self_alias(span: Span) -> Self {
        Self::error_at("E004", Some(span), "`@self` can't be used alone, so use `@self/subpath`")
    }

    pub fn e007_both_extensions(span: Span, luau_path: &str, lua_path: &str) -> Self {
        Self::ambiguous_resolution(span, &format!("both `{luau_path}` and `{lua_path}` exist"))
    }

    pub fn e007_file_and_directory(span: Span, module_path: &str, init_path: &str) -> Self {
        Self::ambiguous_resolution(
            span,
            &format!("both file `{module_path}` and directory init file `{init_path}` exist"),
        )
    }

    pub fn e007_file_beside_directory(span: Span, module_path: &str, directory_path: &str) -> Self {
        Self::ambiguous_resolution(
            span,
            &format!("both file `{module_path}` and directory `{directory_path}` exist"),
        )
    }

    fn ambiguous_resolution(span: Span, details: &str) -> Self {
        Self::error_at("E007", Some(span), "ambiguous module resolution")
            .with_help(format!("ambiguity: {details}, so remove one of the conflicting files"))
    }

    /// A lex or parse failure, which no other code reports.
    pub fn e008<Message>(span: Span, message: Message) -> Self
    where
        Message: Into<Cow<'static, str>>,
    {
        Self::error_at("E008", Some(span), message)
    }

    pub fn e009(limit: usize) -> Self {
        Self::error_at("E009", None, format!("too many modules: exceeded {limit} module limit"))
            .with_help(format!(
                "a bundle may contain at most {limit} modules, so reduce the dependency graph or \
                 split the build"
            ))
    }

    /// `path` is the file that can't be read, as the message names it.
    pub fn e010(path: &str, io_error: &str) -> Self {
        Self::error_at("E010", None, format!("cannot read file `{path}`: {io_error}"))
            .with_help("check that the file exists and is readable")
    }

    pub fn e012(byte_len: usize, limit: usize) -> Self {
        Self::error_at("E012", None, format!("file too large: {byte_len} bytes (limit: {limit})"))
            .with_help(format!("files must be at most {limit} bytes"))
    }

    /// A construct in a module that parses alone but that the module's bundle
    /// wrapper, `wrapper_levels` deep, would nest past `limit`.
    pub fn e013(span: Span, wrapper_levels: usize, limit: usize) -> Self {
        Self::error_at(
            "E013",
            Some(span),
            format!("maximum nesting depth exceeded once bundled (luck resource limit: {limit})"),
        )
        .with_help(format!(
            "the bundle nests a module's code {wrapper_levels} levels deeper inside its loader \
             function, so nest this less deeply, such as by moving part of it into a local"
        ))
    }

    /// A `luck.json`, a file it extends, or a std definition it names, that
    /// isn't valid, at `span` when the problem has a place in the file.
    pub fn e014(span: Option<Span>, problem: &str) -> Self {
        Self::error_at("E014", span, format!("invalid config: {problem}"))
    }

    pub fn w001(span: Span, module_name: &str, first_span: Span) -> Self {
        Self::warning_at("W001", span, format!("module `{module_name}` required multiple times"))
            .with_label(first_span, "first required here")
    }

    /// At the require that closes the cycle, when there's one.
    pub fn w003(span: Option<Span>, cycle_path: &[String]) -> Self {
        let quoted_paths: Vec<String> = cycle_path.iter().map(|path| format!("`{path}`")).collect();
        let cycle = quoted_paths.join(" -> ");
        Self::new("W003", Severity::Warning, "circular dependency between modules".into(), span)
            .with_help(format!(
                "cycle: {cycle}\nmodules load lazily, so a cycle through a function body works, \
                 but one hit while a module is still loading fails at runtime, as it would \
                 without bundling"
            ))
    }

    pub fn w004(span: Span) -> Self {
        Self::warning_at(
            "W004",
            span,
            "alias `self` defined in `.luaurc` is shadowed by built-in `@self`",
        )
    }

    /// A read of a name `require` that isn't the global one: a local, a field
    /// of a local `_ENV`, or a global the file replaces.
    pub fn w005_shadowed(span: Span) -> Self {
        Self::warning_at(
            "W005",
            span,
            "this `require` is not the global `require`, so calls through it are not bundled",
        )
        .with_help(
            "a call through it is left exactly as written and runs whatever `require` holds \
             at runtime",
        )
    }

    pub fn w005_aliased(span: Span) -> Self {
        Self::warning_at(
            "W005",
            span,
            "global `require` is referenced without being called, so aliased call sites are not \
             bundled",
        )
        .with_help(
            "only direct `require(\"literal\")` calls are bundled, and calls through an alias \
             like `local r = require` fall through to the runtime's own `require`",
        )
    }

    pub fn w006(span: Span, hot_comment: &str) -> Self {
        Self::warning_at(
            "W006",
            span,
            format!("hot comment `{hot_comment}` has no effect in a bundle"),
        )
        .with_help(
            "Luau hot comments only apply at the top of a chunk, so only the entry module's \
                 are hoisted above the bundle's loader",
        )
        .with_tag(Tag::Unnecessary)
    }

    /// Lua targets only: the cache is keyed by module name, so a name computed at
    /// runtime can still find a bundled module.
    pub fn w007_loader_fallback(span: Span) -> Self {
        Self::warning_at(
            "W007",
            span,
            "`require` argument is not a string literal, so the module is resolved at runtime",
        )
        .with_help(
            "the call goes through the bundle's loader, which serves bundled modules by name \
             and passes any other name to the runtime's own `require`, so use a string literal \
             to bundle the module itself",
        )
    }

    /// Luau targets only: the bundle's cache is keyed by resolved file, and Roblox
    /// requires take an Instance, so a runtime argument can't name a bundled
    /// module.
    pub fn w007_runtime(span: Span) -> Self {
        Self::warning_at(
            "W007",
            span,
            "`require` argument is not a string literal, so the module is resolved at runtime",
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
        let e2 = Diagnostic::e002(Span::new(0, 10));
        assert_eq!((e2.code, e2.span), ("E002", Some(Span::new(0, 10))));
        assert!(e2.is_error());

        let e4 = Diagnostic::e004(Span::new(0, 10), "mymod", &["src/mymod.lua".to_owned()]);
        assert_eq!(
            e4.help.as_deref(),
            Some(
                "\
searched paths:
  - `src/mymod.lua`"
            )
        );

        let w1 = Diagnostic::w001(Span::new(20, 25), "utils", Span::new(0, 5));
        assert_eq!(w1.code, "W001");
        assert!(!w1.is_error());
        assert!(w1.help.is_none());
        assert_eq!(w1.labels.first().map(|label| label.span), Some(Span::new(0, 5)));
    }

    #[test]
    fn a_problem_with_no_place_has_no_span() {
        assert_eq!(Diagnostic::e010("gone.lua", "no such file").span, None);
        assert_eq!(Diagnostic::e014(None, "bad").span, None);
    }
}
