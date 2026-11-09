use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use luck_core::{Diagnostic, native_path, normalize_path};

use crate::luaurc::{AliasCache, AliasTarget};
use crate::{ResolveRequest, ResolvedModule};

/// What a module path names on disk.
enum Probed {
    Module(PathBuf),
    /// A directory without an init file, which a path can walk through but
    /// which is no module itself.
    Directory,
    Missing,
}

/// Resolves a relative or `@alias` require one path component at a time, as
/// Luau's navigator does.
///
/// # Errors
///
/// Returns E004 when the module or alias is missing or the require has no
/// relative or `@alias` prefix, and E007 when the target, a component on the
/// way to it, or the requiring module is ambiguous.
pub(crate) fn resolve(
    aliases: &mut AliasCache,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let module = request.module.replace('\\', "/");
    let from_path = native_path(request.from_file);
    let requiring_module = requiring_module_path(&from_path);
    // Luau resets to the requiring module first, so an ambiguous one fails
    // every require it makes.
    probe(&requiring_module, request)?;
    let parent = requiring_module.parent().unwrap_or_else(|| Path::new("."));
    if module.starts_with("./") || module.starts_with("../") {
        navigate(parent, &module, request)
    } else if let Some(alias) = module.strip_prefix('@') {
        resolve_alias(aliases, alias, parent, &requiring_module, request)
    } else {
        Err(Diagnostic::e004_luau_scheme(request.from_file, request.span, request.module))
    }
}

/// The module `from_path` is: `dir` for `dir/init.luau`, and the path without
/// its extension for any other Luau file.
fn requiring_module_path(from_path: &Path) -> PathBuf {
    if matches!(from_path.file_name().and_then(OsStr::to_str), Some("init.luau" | "init.lua")) {
        from_path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    } else if matches!(from_path.extension().and_then(OsStr::to_str), Some("luau" | "lua")) {
        from_path.with_extension("")
    } else {
        from_path.to_path_buf()
    }
}

/// Resolves `@alias` or `@alias/sub/path`, given without its `@`.
/// `lookup_directory` is the requiring module's parent, where the search for
/// `.luaurc` files starts.
///
/// # Errors
///
/// Returns E004 when the alias or module is missing or the aliases form a
/// cycle, and E007 when the target is ambiguous.
fn resolve_alias(
    aliases: &mut AliasCache,
    alias: &str,
    lookup_directory: &Path,
    requiring_module: &Path,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let (alias_name, sub_path) =
        alias.split_once('/').map_or((alias, None), |(name, rest)| (name, Some(rest)));

    if alias_name.eq_ignore_ascii_case("self") {
        return resolve_self(aliases, sub_path, requiring_module, lookup_directory, request);
    }

    let (directory, full_sub_path) =
        follow_alias(aliases, lookup_directory, alias_name, sub_path.unwrap_or(""), request)?;
    if matches!(probe(&directory, request)?, Probed::Missing) {
        return Err(module_not_found(&directory, &full_sub_path, request));
    }
    navigate(&directory, &full_sub_path, request)
}

/// Follows `alias_name` through any aliases of aliases to the directory the
/// chain ends at, with `sub_path` preceded by what the chain added.
///
/// # Errors
///
/// Returns E004 when an alias in the chain is missing or the chain is a
/// cycle.
fn follow_alias(
    aliases: &mut AliasCache,
    lookup_directory: &Path,
    alias_name: &str,
    sub_path: &str,
    request: &ResolveRequest<'_>,
) -> Result<(PathBuf, String), Diagnostic> {
    let table = aliases.in_effect(lookup_directory);
    let mut name = alias_name.to_owned();
    let mut full_sub_path = sub_path.to_owned();
    let mut followed: Vec<String> = Vec::new();
    loop {
        let lowercase_name = name.to_ascii_lowercase();
        if followed.contains(&lowercase_name) {
            followed.push(lowercase_name);
            return Err(Diagnostic::e004_alias_cycle(
                request.from_file,
                request.span,
                request.module,
                &followed,
            ));
        }
        match table.get(&lowercase_name) {
            Some(AliasTarget::Directory(directory)) => {
                return Ok((directory.clone(), full_sub_path));
            }
            Some(AliasTarget::Chained(target)) => {
                let (next_name, added_path) = target.split_once('/').unwrap_or((target, ""));
                full_sub_path = format!("{added_path}/{full_sub_path}");
                followed.push(lowercase_name);
                next_name.clone_into(&mut name);
            }
            None => {
                return Err(aliases.malformed_luaurc(lookup_directory).map_or_else(
                    || {
                        Diagnostic::e004_unknown_alias(
                            request.from_file,
                            request.span,
                            request.module,
                            &name,
                        )
                    },
                    |luaurc_path| {
                        Diagnostic::e004_malformed_luaurc(
                            request.from_file,
                            request.span,
                            request.module,
                            &normalize_path(&luaurc_path),
                        )
                    },
                ));
            }
        }
    }
}

/// `@self` walks down from the requiring module itself, so `@self/x` is `dir/x`
/// from `dir/init.luau` and `main/x` from `main.luau`, never a sibling. A
/// `.luaurc` alias named `self` can't override it and gets W004.
///
/// # Errors
///
/// Returns E004 on a bare `@self` or a missing module, and E007 when the
/// target is ambiguous.
fn resolve_self(
    aliases: &mut AliasCache,
    sub_path: Option<&str>,
    requiring_module: &Path,
    lookup_directory: &Path,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let relative_path = sub_path
        .ok_or_else(|| Diagnostic::e004_self_needs_subpath(request.from_file, request.span))?;

    let is_shadowed = aliases.in_effect(lookup_directory).contains_key("self");
    let mut resolved = navigate(requiring_module, relative_path, request)?;
    if is_shadowed {
        resolved.warnings.push(Diagnostic::w004(request.from_file, request.span));
    }
    Ok(resolved)
}

/// Walks `/`-separated `sub_path` from `start` and resolves the module it ends
/// at. Empty and `.` components are skipped, and `..` leaves the current
/// directory unchecked, as in Luau.
///
/// # Errors
///
/// Returns E004 when a component is missing or the path ends at no module,
/// and E007 when a component is ambiguous.
fn navigate(
    start: &Path,
    sub_path: &str,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let mut position = start.to_path_buf();
    let mut probed_position = None;
    for component in sub_path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if !position.pop() {
                    return Err(module_not_found(start, sub_path, request));
                }
                probed_position = None;
            }
            ".config" => return Err(module_not_found(start, sub_path, request)),
            child => {
                position.push(child);
                let probed = probe(&position, request)?;
                if matches!(probed, Probed::Missing) {
                    return Err(module_not_found(start, sub_path, request));
                }
                probed_position = Some(probed);
            }
        }
    }
    match probed_position.map_or_else(|| probe(&position, request), Ok)? {
        Probed::Module(found) => {
            Ok(ResolvedModule { path: normalize_path(&found), warnings: Vec::new() })
        }
        Probed::Directory | Probed::Missing => Err(module_not_found(start, sub_path, request)),
    }
}

/// E004 listing the four files the whole of `sub_path` could have been.
fn module_not_found(start: &Path, sub_path: &str, request: &ResolveRequest<'_>) -> Diagnostic {
    let trimmed_path = sub_path.trim_start_matches('/');
    let target = if trimmed_path.is_empty() {
        start.to_path_buf()
    } else {
        start.join(native_path(trimmed_path))
    };
    Diagnostic::e004(
        request.from_file,
        request.span,
        request.module,
        &[
            append_extension(&target, ".luau"),
            append_extension(&target, ".lua"),
            target.join("init.luau"),
            target.join("init.lua"),
        ]
        .map(|path| normalize_path(&path)),
    )
}

/// What `module_path`, a path with no extension, names on disk.
///
/// That's a `.luau` or `.lua` file, a directory with or without an `init`
/// file, or nothing. `init` itself can't be named as a file, only reached
/// through its directory.
///
/// # Errors
///
/// Returns E007 when more than one of those exists.
fn probe(module_path: &Path, request: &ResolveRequest<'_>) -> Result<Probed, Diagnostic> {
    let module_file = if module_path.file_name() == Some(OsStr::new("init")) {
        None
    } else {
        // Append the extension to the full name: `with_extension` replaces
        // everything after the last dot, so `./config.v2` would probe
        // `config.luau` instead of `config.v2.luau`.
        existing_one(
            append_extension(module_path, ".luau"),
            append_extension(module_path, ".lua"),
            request,
        )?
    };
    if !module_path.is_dir() {
        return Ok(module_file.map_or(Probed::Missing, Probed::Module));
    }
    let init_file =
        existing_one(module_path.join("init.luau"), module_path.join("init.lua"), request)?;
    match (module_file, init_file) {
        (Some(file), Some(init)) => Err(Diagnostic::e007_file_and_directory(
            request.from_file,
            request.span,
            &normalize_path(&file),
            &normalize_path(&init),
        )),
        (Some(file), None) => Err(Diagnostic::e007_file_beside_directory(
            request.from_file,
            request.span,
            &normalize_path(&file),
            &normalize_path(module_path),
        )),
        (None, Some(init)) => Ok(Probed::Module(init)),
        (None, None) => Ok(Probed::Directory),
    }
}

/// Whichever of `luau` and `lua` is a file.
///
/// # Errors
///
/// Returns E007 when both are.
fn existing_one(
    luau: PathBuf,
    lua: PathBuf,
    request: &ResolveRequest<'_>,
) -> Result<Option<PathBuf>, Diagnostic> {
    match (luau.is_file(), lua.is_file()) {
        (true, true) => Err(Diagnostic::e007_both_extensions(
            request.from_file,
            request.span,
            &normalize_path(&luau),
            &normalize_path(&lua),
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
