use std::io;
use std::path::{self, Path, PathBuf};

use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_resolver::{FileSystem, is_definition_file, to_slash_lossy};
use rustc_hash::FxHashMap;

use crate::config::{DefinitionsEntry, LuckConfig};
use crate::parse::{entry_span, parse};

pub const CONFIG_FILE_NAME: &str = "luck.json";

/// How many files one `extends` chain may stack. Each level is a recursive
/// call, and no real project comes close.
const MAX_EXTENDS_DEPTH: usize = 64;

/// Why a config doesn't load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// What's wrong with a config no file holds, an E014 placed in its text.
    Given(Diagnostic),
    /// What's wrong in a file it reads, placed in that file.
    File(FileDiagnostics),
}

impl From<FileDiagnostics> for ConfigError {
    fn from(file: FileDiagnostics) -> Self {
        Self::File(file)
    }
}

/// The nearest `luck.json` at or above `directory` in `file_system`, which
/// mustn't hold a `..`, since walking up from one would leave the directory
/// it names.
pub fn find_config(file_system: &dyn FileSystem, directory: &Path) -> Option<PathBuf> {
    let absolute = path::absolute(directory).ok()?;
    absolute
        .ancestors()
        .map(|ancestor| ancestor.join(CONFIG_FILE_NAME))
        .find(|candidate| file_system.metadata(candidate).is_ok_and(|metadata| metadata.is_file))
}

/// A bare file name's parent is the empty path, which can't be canonicalized
/// or watched, so it maps to `.`.
pub(crate) fn config_directory(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// A config merged through its `extends` chain, and the canonical path of
/// every file the chain read.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub config: LuckConfig,
    pub files: Vec<PathBuf>,
}

/// Later `extends` entries override earlier ones, and the file itself
/// overrides them all. A file reached more than once is read once.
pub(crate) fn load(
    file_system: &dyn FileSystem,
    config_path: &Path,
) -> Result<Loaded, FileDiagnostics> {
    let canonical = file_system
        .canonicalize(config_path)
        .map_err(|e| unreadable(file_system, config_path, &e))?;
    let mut walk = ExtendsWalk {
        file_system,
        chain: Vec::new(),
        finished: FxHashMap::default(),
        files: Vec::new(),
    };
    let config = walk.layer(config_path, canonical, None)?;
    Ok(Loaded { config, files: walk.files })
}

/// `text`, a config no file holds, so it extends none, with its
/// `definitions` relative to `root`.
pub(crate) fn given(
    file_system: &dyn FileSystem,
    text: &str,
    root: &Path,
) -> Result<LuckConfig, Diagnostic> {
    let mut config = parse(text)?;
    if let Some(name) = config.extends.iter().flatten().next() {
        let problem = format!(
            "`extends` names `{name}`, but a config no file holds can't extend one, so give the \
             whole config"
        );
        return Err(Diagnostic::e014(Some(entry_span(text, "extends", name)), &problem));
    }
    resolve_definitions(file_system, &mut config, root, |name, problem| {
        Diagnostic::e014(Some(entry_span(text, "definitions", name)), problem)
    })?;
    Ok(config)
}

/// Makes the path of each definition file `config` names canonical, from
/// `directory`, with `error_on` placing a problem on the entry at fault.
fn resolve_definitions<E>(
    file_system: &dyn FileSystem,
    config: &mut LuckConfig,
    directory: &Path,
    error_on: impl Fn(&str, &str) -> E,
) -> Result<(), E> {
    for entry in config.definitions.iter_mut().flatten() {
        match entry {
            DefinitionsEntry::BuiltIn(name) if name == "roblox" => {}
            DefinitionsEntry::BuiltIn(name) => {
                let named = format!("@{name}");
                let problem = format!(
                    "`definitions` names `{named}`, but `@roblox` is the one definition set built \
                     into luck"
                );
                return Err(error_on(&named, &problem));
            }
            DefinitionsEntry::File(file) => {
                let named = file.to_string_lossy().into_owned();
                if !is_definition_file(file) {
                    let problem = format!(
                        "`definitions` names `{named}`, but a definition file's name ends in \
                         `.d.luau`"
                    );
                    return Err(error_on(&named, &problem));
                }
                *file = file_system.canonicalize(&directory.join(&*file)).map_err(|e| {
                    let problem =
                        format!("`definitions` names `{named}`, which can't be read: {e}");
                    error_on(&named, &problem)
                })?;
            }
        }
    }
    Ok(())
}

struct ExtendsWalk<'fs> {
    file_system: &'fs dyn FileSystem,
    /// The chain being loaded, so a diamond (A extends B and C, both extend
    /// D) is legal and only a path back onto the chain is a cycle.
    chain: Vec<PathBuf>,
    /// Each finished file per `root` boundary it was reached under, because
    /// a boundary constrains everything below it.
    finished: FxHashMap<(PathBuf, Option<PathBuf>), LuckConfig>,
    files: Vec<PathBuf>,
}

impl ExtendsWalk<'_> {
    /// The config at `path`, whose canonical form is `canonical`, merged over
    /// what it extends.
    fn layer(
        &mut self,
        path: &Path,
        canonical: PathBuf,
        root_boundary: Option<&Path>,
    ) -> Result<LuckConfig, FileDiagnostics> {
        let finished_key = (canonical.clone(), root_boundary.map(Path::to_path_buf));
        if let Some(finished) = self.finished.get(&finished_key) {
            return Ok(finished.clone());
        }
        let file_system = self.file_system;
        let text = read_text(file_system, path)?;
        if !self.files.contains(&canonical) {
            self.files.push(canonical.clone());
        }
        let mut config = parse(&text).map_err(|diag| in_file(file_system, path, diag))?;
        let directory = config_directory(path);
        let error_on = |key: &str, name: &str, problem: &str| {
            let diag = Diagnostic::e014(Some(entry_span(&text, key, name)), problem);
            in_file(file_system, path, diag)
        };
        resolve_definitions(file_system, &mut config, &directory, |name, problem| {
            error_on("definitions", name, problem)
        })?;
        let own_boundary = if config.root == Some(true) {
            Some(file_system.canonicalize(&directory).map_err(|e| {
                in_file(file_system, path, Diagnostic::e010(&display(file_system, &directory), &e))
            })?)
        } else {
            None
        };
        let boundary = own_boundary.as_deref().or(root_boundary);
        self.chain.push(canonical);
        let mut merged = LuckConfig::default();
        for name in config.extends.iter().flatten() {
            if self.chain.len() > MAX_EXTENDS_DEPTH {
                return Err(error_on(
                    "extends",
                    name,
                    &format!("`extends` chains more than {MAX_EXTENDS_DEPTH} files deep"),
                ));
            }
            let extended = directory.join(name);
            let extended_canonical = file_system.canonicalize(&extended).map_err(|e| {
                error_on(
                    "extends",
                    name,
                    &format!("`extends` names `{name}`, which can't be read: {e}"),
                )
            })?;
            if self.chain.contains(&extended_canonical) {
                return Err(error_on(
                    "extends",
                    name,
                    &format!("`extends` names `{name}`, which extends this file again"),
                ));
            }
            if let Some(boundary_directory) = boundary
                && !extended_canonical.starts_with(boundary_directory)
            {
                return Err(error_on(
                    "extends",
                    name,
                    &format!(
                        "`extends` names `{name}`, which is outside `{}`, the directory of a \
                         `root` config",
                        boundary_directory.display()
                    ),
                ));
            }
            let base = self.layer(&extended, extended_canonical, boundary)?;
            merged = base.merge_onto(merged);
        }
        self.chain.pop();
        let resolved = config.merge_onto(merged);
        self.finished.insert(finished_key, resolved.clone());
        Ok(resolved)
    }
}

/// The text of the file at `path`, or its E010.
pub(crate) fn read_text(
    file_system: &dyn FileSystem,
    path: &Path,
) -> Result<String, FileDiagnostics> {
    file_system
        .read(path)
        .and_then(|bytes| String::from_utf8(bytes).map_err(io::Error::other))
        .map_err(|e| unreadable(file_system, path, &e))
}

/// E010 in the file at `path`, which can't be read.
fn unreadable(file_system: &dyn FileSystem, path: &Path, e: &io::Error) -> FileDiagnostics {
    in_file(file_system, path, Diagnostic::e010(&display(file_system, path), e))
}

/// The form a message names a file in.
fn display(file_system: &dyn FileSystem, path: &Path) -> String {
    let canonical = file_system.canonicalize(path);
    to_slash_lossy(canonical.as_deref().unwrap_or(path)).into_owned()
}

/// `diag`, in the file at `path`, which names it canonically when it can.
pub(crate) fn in_file(
    file_system: &dyn FileSystem,
    path: &Path,
    diag: Diagnostic,
) -> FileDiagnostics {
    let canonical = file_system.canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    FileDiagnostics { path: canonical, diags: vec![diag] }
}

#[cfg(test)]
mod tests {
    use luck_resolver::FileSystemOs;
    use luck_stdlib::SecurityLevel;
    use luck_token::Span;

    use super::*;
    use crate::fixtures::project_with;
    use crate::glob::Glob;

    #[test]
    fn config_directory_of_a_bare_file_name_is_the_current_directory() {
        assert_eq!(config_directory(Path::new("luck.json")), Path::new("."));
        assert_eq!(config_directory(Path::new("./luck.json")), Path::new("."));
        assert_eq!(config_directory(Path::new("sub/luck.json")), Path::new("sub"));
    }

    #[test]
    fn rejects_a_circular_extends_chain_where_it_closes() {
        let project = project_with(&[
            ("a.json", r#"{ "extends": ["b.json"], "minify": false }"#),
            ("b.json", r#"{ "extends": ["a.json"] }"#),
        ]);
        let error = load(&FileSystemOs, &project.path().join("a.json")).unwrap_err();
        assert!(error.path.ends_with("b.json"), "{error:?}");
        assert_eq!(
            error.diags[0].message,
            "invalid config: `extends` names `a.json`, which extends this file again"
        );
    }

    #[test]
    fn root_config_extends_a_file_inside_its_directory() {
        let project = project_with(&[
            ("luck.json", r#"{ "root": true, "extends": ["base.json"], "minify": false }"#),
            ("base.json", r#"{ "lint": { "extra_globals": ["a"] } }"#),
        ]);
        let loaded = load(&FileSystemOs, &project.path().join("luck.json"))
            .expect("within-boundary extends");
        assert_eq!(loaded.config.root, Some(true));
        assert_eq!(loaded.files.len(), 2, "{:?}", loaded.files);
    }

    #[test]
    fn rejects_a_root_config_extending_outside_its_directory() {
        let project = project_with(&[
            ("outside.json", r#"{ "minify": true }"#),
            ("project/luck.json", r#"{ "root": true, "extends": ["../outside.json"] }"#),
        ]);
        let error = load(&FileSystemOs, &project.path().join("project/luck.json")).unwrap_err();
        assert!(error.diags[0].message.contains("which is outside"), "{error:?}");
    }

    #[test]
    fn root_boundary_applies_to_a_file_already_reached_without_one() {
        let project = project_with(&[
            ("outside.json", "{}"),
            ("project/shared.json", r#"{ "extends": ["../outside.json"] }"#),
            ("project/root.json", r#"{ "root": true, "extends": ["shared.json"] }"#),
            ("project/luck.json", r#"{ "extends": ["shared.json", "root.json"] }"#),
        ]);
        let error = load(&FileSystemOs, &project.path().join("project/luck.json")).unwrap_err();
        assert!(error.diags[0].message.contains("which is outside"), "{error:?}");
    }

    #[test]
    fn extending_one_file_twice_does_not_grow_its_lists() {
        let project = project_with(&[
            ("base.json", r#"{ "include": ["a/**"], "lint": { "extra_globals": ["g"] } }"#),
            ("luck.json", r#"{ "extends": ["base.json", "base.json"], "include": ["b/**"] }"#),
        ]);
        let Loaded { config, files } =
            load(&FileSystemOs, &project.path().join("luck.json")).expect("loads");
        let include: Vec<&str> = config.include.iter().flatten().map(Glob::pattern).collect();
        assert_eq!(include, ["a/**", "b/**"]);
        assert_eq!(config.lint.unwrap().extra_globals, vec!["g".to_owned()]);
        assert_eq!(files.len(), 2, "{files:?}");
    }

    #[test]
    fn non_root_config_extends_a_file_in_a_parent_directory() {
        let project = project_with(&[
            ("shared.json", r#"{ "definitions": ["@roblox"], "security": "PluginSecurity" }"#),
            ("project/luck.json", r#"{ "extends": ["../shared.json"] }"#),
        ]);
        let loaded = load(&FileSystemOs, &project.path().join("project/luck.json")).expect("loads");
        assert_eq!(loaded.config.security, Some(SecurityLevel::PluginSecurity));
    }

    #[test]
    fn a_missing_extended_file_is_reported_on_the_file_that_names_it() {
        let project = project_with(&[("luck.json", r#"{ "extends": ["gone.json"] }"#)]);
        let error = load(&FileSystemOs, &project.path().join("luck.json")).unwrap_err();
        assert!(error.path.ends_with("luck.json"), "{error:?}");
        let diag = &error.diags[0];
        assert!(diag.message.contains("`extends` names `gone.json`, which can't be read"));
        assert_eq!(diag.span, Some(Span::new(14, 25)));
    }

    /// The span of the last time `quoted` is written in `text`.
    fn last_span(text: &str, quoted: &str) -> Span {
        let start = text.rfind(quoted).expect("written");
        Span::from_indices(start, start + quoted.len())
    }

    #[test]
    fn an_extends_error_is_placed_on_its_entry_and_not_on_a_comment_naming_it() {
        let text = r#"{
  // shared rules live in "base.json"
  "extends": ["base.json"]
}"#;
        let project = project_with(&[("luck.json", text)]);
        let error = load(&FileSystemOs, &project.path().join("luck.json")).unwrap_err();
        assert!(error.diags[0].message.contains("`extends` names `base.json`"), "{error:?}");
        assert_eq!(error.diags[0].span, Some(last_span(text, "\"base.json\"")));
    }

    #[test]
    fn a_definitions_error_is_placed_on_its_entry_and_not_on_another_key_s() {
        let text = r#"{ "exclude": ["globals.d.luau"], "definitions": ["globals.d.luau"] }"#;
        let project = project_with(&[("luck.json", text)]);
        let error = load(&FileSystemOs, &project.path().join("luck.json")).unwrap_err();
        let message = &error.diags[0].message;
        assert!(message.contains("`definitions` names `globals.d.luau`"), "{error:?}");
        assert_eq!(error.diags[0].span, Some(last_span(text, "\"globals.d.luau\"")));
    }
}
