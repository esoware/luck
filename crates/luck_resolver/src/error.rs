use std::path::PathBuf;

use luck_diagnostics::Diagnostic;
use luck_token::Span;

use crate::path::to_slash_lossy;

/// Why a require names no module, or more than one, or a definition file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// Every file the module could have been, in the order they were tried,
    /// from the directory the search reached.
    NotFound(Vec<PathBuf>),
    /// The require starts with none of `./`, `../`, and `@`.
    MissingPrefix,
    /// `@self` with no path after it.
    BareSelfAlias,
    /// No `.luaurc` or `.config.luau` in effect declares the alias, given
    /// without its `@`.
    UnknownAlias(String),
    /// The `.luaurc` or `.config.luau` at this path doesn't load, which hides
    /// every alias declared above it.
    MalformedConfig(PathBuf),
    /// This directory has both a `.luaurc` and a `.config.luau`, which Luau
    /// refuses, and which hides every alias declared above it.
    AmbiguousConfig(PathBuf),
    /// The aliases followed until one repeated, lowercased and without their
    /// `@`.
    AliasCycle(Vec<String>),
    /// The require names this definition file, which is no module.
    DefinitionFile(PathBuf),
    /// The module is both a `.luau` and a `.lua` file.
    BothExtensions { luau: PathBuf, lua: PathBuf },
    /// The module is both a file and a directory with an `init` file.
    FileAndDirectory { file: PathBuf, init: PathBuf },
    /// The module, or a directory on the way to it, is both a file and a
    /// directory without an `init` file.
    FileBesideDirectory { file: PathBuf, directory: PathBuf },
}

impl ResolveError {
    /// The E004 or E007 that reports this error for `module`, the require
    /// string at `span`.
    pub fn to_diagnostic(&self, span: Span, module: &str) -> Diagnostic {
        match self {
            Self::NotFound(searched) => {
                let tried_paths: Vec<String> =
                    searched.iter().map(|path| to_slash_lossy(path).into_owned()).collect();
                Diagnostic::e004(span, module, &tried_paths)
            }
            Self::MissingPrefix => Diagnostic::e004_missing_prefix(span, module),
            Self::BareSelfAlias => Diagnostic::e004_bare_self_alias(span),
            Self::UnknownAlias(alias) => Diagnostic::e004_unknown_alias(span, module, alias),
            Self::MalformedConfig(config) => {
                Diagnostic::e004_malformed_config(span, module, &to_slash_lossy(config))
            }
            Self::AmbiguousConfig(directory) => {
                Diagnostic::e004_ambiguous_config(span, module, &to_slash_lossy(directory))
            }
            Self::AliasCycle(cycle) => Diagnostic::e004_alias_cycle(span, module, cycle),
            Self::DefinitionFile(path) => {
                Diagnostic::e004_definition_file(span, module, &to_slash_lossy(path))
            }
            Self::BothExtensions { luau, lua } => {
                Diagnostic::e007_both_extensions(span, &to_slash_lossy(luau), &to_slash_lossy(lua))
            }
            Self::FileAndDirectory { file, init } => Diagnostic::e007_file_and_directory(
                span,
                &to_slash_lossy(file),
                &to_slash_lossy(init),
            ),
            Self::FileBesideDirectory { file, directory } => {
                Diagnostic::e007_file_beside_directory(
                    span,
                    &to_slash_lossy(file),
                    &to_slash_lossy(directory),
                )
            }
        }
    }
}
