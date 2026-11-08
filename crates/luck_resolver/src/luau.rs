use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use luck_core::{Diagnostic, native_path, normalize_path};

use crate::luaurc::AliasCache;
use crate::{ResolveRequest, ResolvedModule};

/// Resolves a relative or `@alias` require.
///
/// # Errors
///
/// Returns E004 when the module or alias is missing or the require has no
/// relative or `@alias` prefix, and E007 when the target is ambiguous.
pub(crate) fn resolve(
    aliases: &mut AliasCache,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let module = request.module;
    if module.starts_with("./") || module.starts_with("../") {
        let from_path = native_path(request.from_file);
        probe(&relative_base(&from_path).join(native_path(module)), request)
    } else if let Some(alias) = module.strip_prefix('@') {
        resolve_alias(aliases, alias, request)
    } else {
        Err(Diagnostic::e004_luau_scheme(request.from_file, request.span, module))
    }
}

/// Resolves `@alias` or `@alias/sub/path`, given without its `@`.
///
/// # Errors
///
/// Returns E004 when the alias or module is missing, and E007 when the
/// target is ambiguous.
fn resolve_alias(
    aliases: &mut AliasCache,
    alias: &str,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let (alias_name, sub_path) =
        alias.split_once('/').map_or((alias, None), |(name, rest)| (name, Some(rest)));
    let alias_lower = alias_name.to_lowercase();

    let from_path = native_path(request.from_file);
    let from_directory = from_path.parent().unwrap_or_else(|| Path::new("."));

    if alias_lower == "self" {
        return resolve_self(aliases, sub_path, from_directory, request);
    }

    let alias_path = aliases.in_effect(from_directory).get(&alias_lower).ok_or_else(|| {
        Diagnostic::e004_unknown_alias(request.from_file, request.span, request.module, alias_name)
    })?;

    sub_path.map_or_else(
        || probe(alias_path, request),
        |relative_path| probe(&alias_path.join(native_path(relative_path)), request),
    )
}

/// `@self` resolves against the requiring file's own directory. A `.luaurc`
/// alias named `self` can't override it and gets W004.
///
/// # Errors
///
/// Returns E004 on a bare `@self` or a missing module, and E007 when the
/// target is ambiguous.
fn resolve_self(
    aliases: &mut AliasCache,
    sub_path: Option<&str>,
    self_directory: &Path,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let relative_path = sub_path
        .ok_or_else(|| Diagnostic::e004_self_needs_subpath(request.from_file, request.span))?;

    let is_shadowed = aliases.in_effect(self_directory).contains_key("self");
    let mut resolved = probe(&self_directory.join(native_path(relative_path)), request)?;
    if is_shadowed {
        resolved.warnings.push(Diagnostic::w004(request.from_file, request.span));
    }
    Ok(resolved)
}

/// The directory relative imports resolve from. An `init.lua` or `init.luau`
/// file uses its parent's parent, matching Roblox's resolver.
fn relative_base(from_path: &Path) -> &Path {
    let from_directory = from_path.parent().unwrap_or_else(|| Path::new("."));
    if matches!(from_path.file_name().and_then(OsStr::to_str), Some("init.luau" | "init.lua")) {
        from_directory.parent().unwrap_or_else(|| Path::new("."))
    } else {
        from_directory
    }
}

/// Finds the module at `base_path`, which may exist with a `.luau` or `.lua`
/// extension or as a directory holding `init.luau` or `init.lua`.
///
/// # Errors
///
/// Returns E004 when no candidate exists, and E007 when more than one does.
fn probe(base_path: &Path, request: &ResolveRequest<'_>) -> Result<ResolvedModule, Diagnostic> {
    // Append the extension to the full name: `with_extension` replaces
    // everything after the last dot, so `./config.v2` would probe
    // `config.luau` instead of `config.v2.luau`.
    let file_luau = append_extension(base_path, ".luau");
    let file_lua = append_extension(base_path, ".lua");
    let file_match = existing_one(&file_luau, &file_lua, request)?;

    let init_luau = base_path.join("init.luau");
    let init_lua = base_path.join("init.lua");
    let directory_match = existing_one(&init_luau, &init_lua, request)?;

    match (file_match, directory_match) {
        (Some(file), Some(directory)) => Err(Diagnostic::e007_file_and_directory(
            request.from_file,
            request.span,
            &normalize_path(file),
            &normalize_path(directory),
        )),
        (Some(found), None) | (None, Some(found)) => {
            Ok(ResolvedModule { path: normalize_path(found), warnings: Vec::new() })
        }
        (None, None) => Err(Diagnostic::e004(
            request.from_file,
            request.span,
            request.module,
            &[&file_luau, &file_lua, &init_luau, &init_lua].map(|path| normalize_path(path)),
        )),
    }
}

/// Whichever of `luau` and `lua` is a file.
///
/// # Errors
///
/// Returns E007 when both are.
fn existing_one<'path>(
    luau: &'path Path,
    lua: &'path Path,
    request: &ResolveRequest<'_>,
) -> Result<Option<&'path Path>, Diagnostic> {
    match (luau.is_file(), lua.is_file()) {
        (true, true) => Err(Diagnostic::e007_both_extensions(
            request.from_file,
            request.span,
            &normalize_path(luau),
            &normalize_path(lua),
        )),
        (true, false) => Ok(Some(luau)),
        (false, true) => Ok(Some(lua)),
        (false, false) => Ok(None),
    }
}

fn append_extension(base_path: &Path, extension: &str) -> PathBuf {
    let mut name = base_path.as_os_str().to_os_string();
    name.push(extension);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use luck_core::Diagnostic;
    use luck_token::LuaVersion;
    use tempfile::TempDir;

    use crate::fixtures::{luau_project, normalized, request, write_files};
    use crate::{ResolvedModule, Resolver};

    /// Resolves `module` as required from `requiring_file`, a path relative to
    /// `project`.
    ///
    /// # Errors
    ///
    /// Returns the resolver's diagnostic when `module` doesn't resolve.
    fn resolve_luau(
        project: &TempDir,
        requiring_file: &str,
        module: &str,
    ) -> Result<ResolvedModule, Diagnostic> {
        let from_file = normalized(project, requiring_file);
        Resolver::new(LuaVersion::Luau, &[], project.path()).resolve(&request(module, &from_file))
    }

    #[test]
    fn resolves_relative_luau_file() {
        let project = luau_project();
        let resolved = resolve_luau(&project, "src/main.luau", "./utils").expect("resolve ./utils");
        assert_eq!(resolved.path, normalized(&project, "src/utils.luau"));
    }

    #[test]
    fn resolves_lua_file_when_only_lua_exists() {
        let project = luau_project();
        let resolved =
            resolve_luau(&project, "src/main.luau", "./helper").expect("resolve ./helper");
        assert_eq!(resolved.path, normalized(&project, "src/helper.lua"));
    }

    #[test]
    fn reports_e007_when_luau_and_lua_files_both_exist() {
        let project = luau_project();
        write_files(
            project.path(),
            &[("src/both.luau", "return {}"), ("src/both.lua", "return {}")],
        );
        let error = resolve_luau(&project, "src/main.luau", "./both").expect_err("ambiguous");
        assert_eq!(error.code, "E007");
        let expected_help = format!(
            "ambiguity: both `{}` and `{}` exist, so remove one of the conflicting files",
            normalized(&project, "src/both.luau"),
            normalized(&project, "src/both.lua")
        );
        assert_eq!(error.help, Some(expected_help));
    }

    #[test]
    fn resolves_directory_module_via_init_luau() {
        let project = luau_project();
        write_files(project.path(), &[("src/components/init.luau", "return {}")]);
        let resolved =
            resolve_luau(&project, "src/main.luau", "./components").expect("resolve ./components");
        assert_eq!(resolved.path, normalized(&project, "src/components/init.luau"));
    }

    #[test]
    fn resolves_directory_module_via_init_lua() {
        let project = luau_project();
        write_files(project.path(), &[("src/legacy/init.lua", "return {}")]);
        let resolved =
            resolve_luau(&project, "src/main.luau", "./legacy").expect("resolve ./legacy");
        assert_eq!(resolved.path, normalized(&project, "src/legacy/init.lua"));
    }

    #[test]
    fn reports_e007_when_file_and_directory_module_both_exist() {
        let project = luau_project();
        write_files(
            project.path(),
            &[("src/widget.luau", "return {}"), ("src/widget/init.luau", "return {}")],
        );
        let error = resolve_luau(&project, "src/main.luau", "./widget").expect_err("ambiguous");
        assert_eq!(error.code, "E007");
        let expected_help = format!(
            "ambiguity: both file `{}` and directory init file `{}` exist, so remove one of the \
             conflicting files",
            normalized(&project, "src/widget.luau"),
            normalized(&project, "src/widget/init.luau")
        );
        assert_eq!(error.help, Some(expected_help));
    }

    #[test]
    fn reports_e007_when_init_luau_and_init_lua_both_exist() {
        let project = luau_project();
        write_files(
            project.path(),
            &[("src/ambig/init.luau", "return {}"), ("src/ambig/init.lua", "return {}")],
        );
        let error = resolve_luau(&project, "src/main.luau", "./ambig").expect_err("ambiguous");
        assert_eq!(error.code, "E007");
        let expected_help = format!(
            "ambiguity: both `{}` and `{}` exist, so remove one of the conflicting files",
            normalized(&project, "src/ambig/init.luau"),
            normalized(&project, "src/ambig/init.lua")
        );
        assert_eq!(error.help, Some(expected_help));
    }

    /// Missing candidates aren't canonicalized, so they keep the require's
    /// `./`.
    #[test]
    fn reports_e004_with_searched_paths_when_relative_module_missing() {
        let project = luau_project();
        let error = resolve_luau(&project, "src/main.luau", "./nonexistent").expect_err("missing");
        assert_eq!(error.code, "E004");
        assert_eq!(error.message, "module not found: `./nonexistent`");
        let base = format!("{}/./nonexistent", normalized(&project, "src"));
        let expected_help = format!(
            "searched paths:\n  - `{base}.luau`\n  - `{base}.lua`\n  - `{base}/init.luau`\n  - \
             `{base}/init.lua`"
        );
        assert_eq!(error.help, Some(expected_help));
    }

    #[test]
    fn reports_e004_when_no_luaurc_declares_the_alias() {
        let project = luau_project();
        let error =
            resolve_luau(&project, "src/main.luau", "@Missing/utils").expect_err("unknown alias");
        assert_eq!(error.code, "E004");
        assert_eq!(error.message, "module not found: `@Missing/utils`");
        assert_eq!(
            error.help.as_deref(),
            Some("searched paths:\n  - alias `@Missing` not found in any `.luaurc`")
        );
    }

    #[test]
    fn reports_e004_for_unprefixed_require() {
        let project = luau_project();
        let error = resolve_luau(&project, "src/main.luau", "utils").expect_err("unprefixed");
        assert_eq!(error.code, "E004");
    }

    #[test]
    fn resolves_parent_relative_luau_file() {
        let project = luau_project();
        write_files(project.path(), &[("src/sub/child.luau", "-- child")]);
        let resolved =
            resolve_luau(&project, "src/sub/child.luau", "../utils").expect("resolve ../utils");
        assert_eq!(resolved.path, normalized(&project, "src/utils.luau"));
    }

    #[test]
    fn init_file_resolves_relative_from_parent_parent() {
        let project = luau_project();
        write_files(project.path(), &[("src/components/init.luau", "-- init")]);
        let resolved =
            resolve_luau(&project, "src/components/init.luau", "./utils").expect("resolve ./utils");
        assert_eq!(resolved.path, normalized(&project, "src/utils.luau"));
    }

    #[test]
    fn resolves_alias_from_luaurc() {
        let project = luau_project();
        write_files(
            project.path(),
            &[
                ("shared/common.luau", "return {}"),
                ("src/.luaurc", r#"{"aliases":{"shared":"../shared"}}"#),
            ],
        );
        let resolved =
            resolve_luau(&project, "src/main.luau", "@shared/common").expect("resolve @shared");
        assert_eq!(resolved.path, normalized(&project, "shared/common.luau"));
    }

    #[test]
    fn matches_alias_case_insensitively() {
        let project = luau_project();
        write_files(
            project.path(),
            &[
                ("shared/thing.luau", "return {}"),
                ("src/.luaurc", r#"{"aliases":{"Shared":"../shared"}}"#),
            ],
        );
        let resolved =
            resolve_luau(&project, "src/main.luau", "@shared/thing").expect("resolve @shared");
        assert_eq!(resolved.path, normalized(&project, "shared/thing.luau"));
    }

    #[test]
    fn nested_file_sees_aliases_from_every_ancestor_luaurc() {
        let project = luau_project();
        write_files(
            project.path(),
            &[
                ("src/deep/nested/mod.luau", "-- entry"),
                (".luaurc", r#"{"aliases":{"root_alias":"./root_lib"}}"#),
                ("src/.luaurc", r#"{"aliases":{"src_alias":"./src_lib"}}"#),
                ("root_lib/foo.luau", "return {}"),
                ("src/src_lib/bar.luau", "return {}"),
            ],
        );
        let from_file = "src/deep/nested/mod.luau";

        let root_alias_resolved =
            resolve_luau(&project, from_file, "@root_alias/foo").expect("resolve @root_alias");
        assert_eq!(root_alias_resolved.path, normalized(&project, "root_lib/foo.luau"));

        let src_alias_resolved =
            resolve_luau(&project, from_file, "@src_alias/bar").expect("resolve @src_alias");
        assert_eq!(src_alias_resolved.path, normalized(&project, "src/src_lib/bar.luau"));
    }

    #[test]
    fn self_alias_resolves_from_an_init_files_own_directory() {
        let project = luau_project();
        write_files(
            project.path(),
            &[("src/components/init.luau", "-- init"), ("src/components/button.luau", "return {}")],
        );
        let resolved = resolve_luau(&project, "src/components/init.luau", "@self/button")
            .expect("resolve @self/button");
        assert_eq!(resolved.path, normalized(&project, "src/components/button.luau"));
    }

    #[test]
    fn reports_e004_for_bare_self_alias() {
        let project = luau_project();
        let error = resolve_luau(&project, "src/main.luau", "@self").expect_err("bare @self");
        assert_eq!(error.code, "E004");
    }
}
