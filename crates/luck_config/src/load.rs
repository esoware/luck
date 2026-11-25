use std::path::{self, Path, PathBuf};
use std::{fs, io};

use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_resolver::{FileSystem as _, FileSystemOs, to_slash_lossy};
use luck_stdlib::StdlibDefinition;
use rustc_hash::FxHashMap;

use crate::config::LuckConfig;
use crate::parse::{entry_span, parse};

pub const CONFIG_FILE_NAME: &str = "luck.json";

/// How many files one `extends` chain may stack. Each level is a recursive
/// call, and no real project comes close.
const MAX_EXTENDS_DEPTH: usize = 64;

/// The nearest `luck.json` at or above `directory`, which mustn't hold a
/// `..`, since walking up from one would leave the directory it names.
pub fn find_config(directory: &Path) -> Option<PathBuf> {
    let absolute = path::absolute(directory).ok()?;
    absolute
        .ancestors()
        .map(|ancestor| ancestor.join(CONFIG_FILE_NAME))
        .find(|candidate| candidate.is_file())
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
pub(crate) fn load(config_path: &Path) -> Result<Loaded, FileDiagnostics> {
    let canonical =
        FileSystemOs.canonicalize(config_path).map_err(|e| unreadable(config_path, &e))?;
    let mut walk = ExtendsWalk::default();
    let config = walk.layer(config_path, canonical, None)?;
    Ok(Loaded { config, files: walk.files })
}

#[derive(Default)]
struct ExtendsWalk {
    /// The chain being loaded, so a diamond (A extends B and C, both extend
    /// D) is legal and only a path back onto the chain is a cycle.
    chain: Vec<PathBuf>,
    /// Each finished file per `root` boundary it was reached under, because
    /// a boundary constrains everything below it.
    finished: FxHashMap<(PathBuf, Option<PathBuf>), LuckConfig>,
    files: Vec<PathBuf>,
}

impl ExtendsWalk {
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
        let text = fs::read_to_string(path).map_err(|e| unreadable(path, &e))?;
        if !self.files.contains(&canonical) {
            self.files.push(canonical.clone());
        }
        let mut config = parse(&text).map_err(|diag| in_file(path, diag))?;
        let directory = config_directory(path);
        let error_on = |key: &str, name: &str, problem: &str| {
            in_file(path, Diagnostic::e014(Some(entry_span(&text, key, name)), problem))
        };
        for std_path in config.std.iter_mut().flatten() {
            let named = std_path.to_string_lossy().into_owned();
            *std_path = FileSystemOs.canonicalize(&directory.join(&*std_path)).map_err(|e| {
                error_on("std", &named, &format!("`std` names `{named}`, which can't be read: {e}"))
            })?;
        }
        let own_boundary = if config.root == Some(true) {
            Some(
                FileSystemOs
                    .canonicalize(&directory)
                    .map_err(|e| in_file(path, Diagnostic::e010(&display(&directory), &e)))?,
            )
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
            let extended_canonical = FileSystemOs.canonicalize(&extended).map_err(|e| {
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

/// The std definition at `path`, which names a file the `std` of a config
/// resolved.
pub(crate) fn read_std_definition(path: &Path) -> Result<StdlibDefinition, FileDiagnostics> {
    let text = fs::read_to_string(path).map_err(|e| unreadable(path, &e))?;
    StdlibDefinition::parse(&text).map_err(|diag| in_file(path, diag))
}

/// E010 in the file at `path`, which can't be read.
fn unreadable(path: &Path, e: &io::Error) -> FileDiagnostics {
    in_file(path, Diagnostic::e010(&display(path), e))
}

/// The form a message names a file in.
fn display(path: &Path) -> String {
    let canonical = FileSystemOs.canonicalize(path);
    to_slash_lossy(canonical.as_deref().unwrap_or(path)).into_owned()
}

/// `diag`, in the file at `path`, which names it canonically when it can.
pub(crate) fn in_file(path: &Path, diag: Diagnostic) -> FileDiagnostics {
    let canonical = FileSystemOs.canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    FileDiagnostics { path: canonical, diags: vec![diag] }
}

#[cfg(test)]
mod tests {
    use luck_stdlib::Variant;
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
            ("a.json", r#"{ "extends": ["b.json"], "variant": "standalone" }"#),
            ("b.json", r#"{ "extends": ["a.json"] }"#),
        ]);
        let error = load(&project.path().join("a.json")).unwrap_err();
        assert!(error.path.ends_with("b.json"), "{error:?}");
        assert_eq!(
            error.diags[0].message,
            "invalid config: `extends` names `a.json`, which extends this file again"
        );
    }

    #[test]
    fn root_config_extends_a_file_inside_its_directory() {
        let project = project_with(&[
            ("luck.json", r#"{ "root": true, "extends": ["base.json"], "variant": "standalone" }"#),
            ("base.json", r#"{ "lint": { "extra_globals": ["a"] } }"#),
        ]);
        let loaded = load(&project.path().join("luck.json")).expect("within-boundary extends");
        assert_eq!(loaded.config.root, Some(true));
        assert_eq!(loaded.files.len(), 2, "{:?}", loaded.files);
    }

    #[test]
    fn rejects_a_root_config_extending_outside_its_directory() {
        let project = project_with(&[
            ("outside.json", r#"{ "variant": "roblox" }"#),
            ("project/luck.json", r#"{ "root": true, "extends": ["../outside.json"] }"#),
        ]);
        let error = load(&project.path().join("project/luck.json")).unwrap_err();
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
        let error = load(&project.path().join("project/luck.json")).unwrap_err();
        assert!(error.diags[0].message.contains("which is outside"), "{error:?}");
    }

    #[test]
    fn extending_one_file_twice_does_not_grow_its_lists() {
        let project = project_with(&[
            ("base.json", r#"{ "include": ["a/**"], "lint": { "extra_globals": ["g"] } }"#),
            ("luck.json", r#"{ "extends": ["base.json", "base.json"], "include": ["b/**"] }"#),
        ]);
        let Loaded { config, files } = load(&project.path().join("luck.json")).expect("loads");
        let include: Vec<&str> = config.include.iter().flatten().map(Glob::pattern).collect();
        assert_eq!(include, ["a/**", "b/**"]);
        assert_eq!(config.lint.unwrap().extra_globals, vec!["g".to_owned()]);
        assert_eq!(files.len(), 2, "{files:?}");
    }

    #[test]
    fn non_root_config_extends_a_file_in_a_parent_directory() {
        let project = project_with(&[
            ("shared.json", r#"{ "variant": "roblox" }"#),
            ("project/luck.json", r#"{ "extends": ["../shared.json"] }"#),
        ]);
        let loaded = load(&project.path().join("project/luck.json")).expect("loads");
        assert_eq!(loaded.config.variant, Some(Variant::Roblox));
    }

    #[test]
    fn a_missing_extended_file_is_reported_on_the_file_that_names_it() {
        let project = project_with(&[("luck.json", r#"{ "extends": ["gone.json"] }"#)]);
        let error = load(&project.path().join("luck.json")).unwrap_err();
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
        let error = load(&project.path().join("luck.json")).unwrap_err();
        assert!(error.diags[0].message.contains("`extends` names `base.json`"), "{error:?}");
        assert_eq!(error.diags[0].span, Some(last_span(text, "\"base.json\"")));
    }

    #[test]
    fn a_std_error_is_placed_on_its_entry_and_not_on_another_key_s() {
        let text = r#"{ "exclude": ["globals.toml"], "std": ["globals.toml"] }"#;
        let project = project_with(&[("luck.json", text)]);
        let error = load(&project.path().join("luck.json")).unwrap_err();
        assert!(error.diags[0].message.contains("`std` names `globals.toml`"), "{error:?}");
        assert_eq!(error.diags[0].span, Some(last_span(text, "\"globals.toml\"")));
    }
}
