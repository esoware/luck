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
