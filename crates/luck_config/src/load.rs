use std::fs;
use std::path::{self, Path, PathBuf};

use json5::char::{is_json5_identifier, is_json5_line_terminator};
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_resolver::{FileSystem as _, FileSystemOs, to_slash_lossy};
use luck_stdlib::StdlibDefinition;
use luck_token::Span;
use rustc_hash::FxHashMap;

use crate::config::LuckConfig;

pub const CONFIG_FILE_NAME: &str = "luck.json";

/// How many files one `extends` chain may stack. Each level is a recursive
/// call, and no real project comes close.
const MAX_EXTENDS_DEPTH: usize = 64;

/// The nearest `luck.json` at or above `directory`, which mustn't hold a
/// `..`, since walking up from one would leave the directory it names.
#[must_use]
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
///
/// # Errors
///
/// Returns E010 when `config_path` can't be read, and E014 when a file in
/// the chain isn't a valid config or names a file it can't extend, in the
/// file at fault.
pub(crate) fn load(config_path: &Path) -> Result<Loaded, FileDiagnostics> {
    let canonical = FileSystemOs.canonicalize(config_path).map_err(|e| {
        in_file(config_path, Diagnostic::e010(&display(config_path), &e.to_string()))
    })?;
    let mut walk = ExtendsWalk::default();
    let config = walk.layer(config_path, canonical, None, 0)?;
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
    ///
    /// # Errors
    ///
    /// As for [`load`].
    fn layer(
        &mut self,
        path: &Path,
        canonical: PathBuf,
        root_boundary: Option<&Path>,
        depth: usize,
    ) -> Result<LuckConfig, FileDiagnostics> {
        let finished_key = (canonical.clone(), root_boundary.map(Path::to_path_buf));
        if let Some(finished) = self.finished.get(&finished_key) {
            return Ok(finished.clone());
        }
        let text = fs::read_to_string(path)
            .map_err(|e| in_file(path, Diagnostic::e010(&display(path), &e.to_string())))?;
        if !self.files.contains(&canonical) {
            self.files.push(canonical.clone());
        }
        let mut config = parse(&text).map_err(|diag| in_file(path, diag))?;
        let directory = config_directory(path);
        for std_path in config.std.iter_mut().flatten() {
            let named = std_path.to_string_lossy().into_owned();
            *std_path = FileSystemOs.canonicalize(&directory.join(&*std_path)).map_err(|e| {
                in_file(
                    path,
                    Diagnostic::e014(
                        Some(text_span(&text, &named)),
                        &format!("`std` names `{named}`, which can't be read: {e}"),
                    ),
                )
            })?;
        }
        let own_boundary = if config.root == Some(true) {
            Some(FileSystemOs.canonicalize(&directory).map_err(|e| {
                in_file(path, Diagnostic::e010(&display(&directory), &e.to_string()))
            })?)
        } else {
            None
        };
        let boundary = own_boundary.as_deref().or(root_boundary);
        let extends_error = |name: &str, problem: String| {
            in_file(path, Diagnostic::e014(Some(text_span(&text, name)), &problem))
        };
        self.chain.push(canonical);
        let mut merged = LuckConfig::default();
        for name in config.extends.iter().flatten() {
            if depth == MAX_EXTENDS_DEPTH {
                return Err(extends_error(
                    name,
                    format!("`extends` chains more than {MAX_EXTENDS_DEPTH} files deep"),
                ));
            }
            let extended = directory.join(name);
            let extended_canonical = FileSystemOs.canonicalize(&extended).map_err(|e| {
                extends_error(name, format!("`extends` names `{name}`, which can't be read: {e}"))
            })?;
            if self.chain.contains(&extended_canonical) {
                return Err(extends_error(
                    name,
                    format!("`extends` names `{name}`, which extends this file again"),
                ));
            }
            if let Some(boundary_directory) = boundary
                && !extended_canonical.starts_with(boundary_directory)
            {
                return Err(extends_error(
                    name,
                    format!(
                        "`extends` names `{name}`, which is outside `{}`, the directory of a \
                         `root` config",
                        boundary_directory.display()
                    ),
                ));
            }
            let base =
                self.layer(&extended, extended_canonical, boundary, depth.saturating_add(1))?;
            merged = base.merge_onto(merged);
        }
        self.chain.pop();
        let resolved = config.merge_onto(merged);
        self.finished.insert(finished_key, resolved.clone());
        Ok(resolved)
    }
}

/// # Errors
///
/// Returns E014, at the place the JSON5 reader stopped, when `text` isn't a
/// valid config.
pub(crate) fn parse(text: &str) -> Result<LuckConfig, Diagnostic> {
    json5::from_str(text).map_err(|e| {
        let message = e.to_string();
        let (problem, span) = e.position().map_or_else(
            || (message.as_str(), None),
            |position| {
                let problem = message.strip_suffix(&format!(" at {position}")).unwrap_or(&message);
                (problem, Some(token_at(text, offset_of(text, position))))
            },
        );
        Diagnostic::e014(span, problem)
    })
}

/// The byte offset of `position`, which counts lines and characters the way
/// JSON5 does.
fn offset_of(text: &str, position: json5::Position) -> usize {
    let mut chars = text.char_indices().peekable();
    let mut line = 0;
    while line < position.line {
        let Some((_, c)) = chars.next() else {
            return text.len();
        };
        if is_json5_line_terminator(c) {
            if c == '\r' {
                chars.next_if(|&(_, next)| next == '\n');
            }
            line = line.saturating_add(1);
        }
    }
    chars.nth(position.column).map_or(text.len(), |(offset, _)| offset)
}

/// The token that starts at `start`: a quoted string, or a run of the
/// characters a key, a number, or a literal is made of, so an error
/// underlines the word at fault.
fn token_at(text: &str, start: usize) -> Span {
    let rest = text.get(start..).unwrap_or_default();
    let mut chars = rest.char_indices();
    let length = match chars.next() {
        None => 0,
        Some((_, quote @ ('"' | '\''))) => {
            let mut is_escaped = false;
            chars
                .find(|&(_, c)| {
                    let is_closing = c == quote && !is_escaped;
                    is_escaped = c == '\\' && !is_escaped;
                    is_closing
                })
                .map_or(rest.len(), |(index, _)| index.saturating_add(1))
        }
        Some((_, first)) if is_word_char(first) => {
            chars.find(|&(_, c)| !is_word_char(c)).map_or(rest.len(), |(index, _)| index)
        }
        Some((_, first)) => first.len_utf8(),
    };
    Span::from_indices(start, start.saturating_add(length))
}

fn is_word_char(c: char) -> bool {
    is_json5_identifier(c) || matches!(c, '+' | '-' | '.')
}

/// Where `name` is written in `text` as a string, or the start when it's
/// written another way, such as through an escape.
fn text_span(text: &str, name: &str) -> Span {
    ['"', '\'']
        .iter()
        .find_map(|quote| text.find(&format!("{quote}{name}{quote}")))
        .map_or_else(Span::default, |start| {
            Span::from_indices(start, start.saturating_add(name.len()).saturating_add(2))
        })
}

/// The std definition at `path`, which names a file the `std` of a config
/// resolved.
///
/// # Errors
///
/// Returns E010 when the file can't be read, and E014 when it isn't a valid
/// std definition, in the file.
pub(crate) fn read_std_definition(path: &Path) -> Result<StdlibDefinition, FileDiagnostics> {
    let text = fs::read_to_string(path)
        .map_err(|e| in_file(path, Diagnostic::e010(&display(path), &e.to_string())))?;
    StdlibDefinition::parse(&text).map_err(|diag| in_file(path, diag))
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
    use super::*;
    use crate::fixtures::{parse_error, project_with};

    #[test]
    fn rejects_invalid_json5_at_the_place_it_stops() {
        let diag = parse("{\n  lua: \"lua54\" \"luau\"\n}").unwrap_err();
        assert_eq!(diag.code, "E014");
        assert_eq!(diag.message, "invalid config: expected comma");
        assert_eq!(diag.span, Some(Span::new(17, 23)));
    }

    #[test]
    fn places_an_unknown_key_on_the_key() {
        let text = "{\r\n  \"lua\": \"lua54\",\r\n  unknown_key: true\r\n}";
        let diag = parse(text).unwrap_err();
        assert!(
            diag.message.starts_with("invalid config: unknown field `unknown_key`"),
            "{diag:?}"
        );
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"unknown_key"[..]));
    }

    #[test]
    fn places_a_bad_value_on_the_value() {
        let text = "{ \"lua\": 'lua99' }";
        let diag = parse(text).unwrap_err();
        assert!(diag.message.contains("unknown variant `lua99`"), "{diag:?}");
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"'lua99'"[..]));
    }

    #[test]
    fn places_an_unknown_rule_on_its_name() {
        let text = "{ lint: { rules: { unsued_variable: 'off' } } }";
        let diag = parse(text).unwrap_err();
        assert_eq!(
            diag.message,
            "invalid config: unknown lint rule `unsued_variable`, did you mean `unused_variable`?"
        );
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"unsued_variable"[..]));
    }

    #[test]
    fn parses_json5_comments_unquoted_keys_and_trailing_commas() {
        let error = parse_error("{ target: }");
        assert!(error.starts_with("invalid config:"), "{error}");
        let config = parse(
            r#"{
            // this is a comment
            lua: "lua54",
            minify: true, // trailing comma
        }"#,
        )
        .expect("JSON5 features should parse");
        assert_eq!(config.minify, Some(true));
    }

    #[test]
    fn offset_of_counts_json5_line_terminators_and_characters() {
        let text = "a\r\nb\u{2028}\u{e9}cd";
        let at = |line, column| offset_of(text, json5::Position { line, column });
        assert_eq!(at(1, 0), 3);
        assert_eq!(at(2, 1), 9);
        assert_eq!(at(5, 0), text.len());
    }

    #[test]
    fn config_directory_of_a_bare_file_name_is_the_current_directory() {
        assert_eq!(config_directory(Path::new("luck.json")), Path::new("."));
        assert_eq!(config_directory(Path::new("./luck.json")), Path::new("."));
        assert_eq!(config_directory(Path::new("sub/luck.json")), Path::new("sub"));
    }

    #[test]
    fn rejects_a_circular_extends_chain_where_it_closes() {
        let project = project_with(&[
            ("a.json", r#"{ "extends": ["b.json"], "lua": "lua54" }"#),
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
            ("luck.json", r#"{ "root": true, "extends": ["base.json"], "lua": "lua54" }"#),
            ("base.json", r#"{ "lint": { "extra_globals": ["a"] } }"#),
        ]);
        let loaded = load(&project.path().join("luck.json")).expect("within-boundary extends");
        assert_eq!(loaded.config.root, Some(true));
        assert_eq!(loaded.files.len(), 2, "{:?}", loaded.files);
    }

    #[test]
    fn rejects_a_root_config_extending_outside_its_directory() {
        let project = project_with(&[
            ("outside.json", r#"{ "lua": "lua53" }"#),
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
            ("base.json", r#"{ "search_paths": ["a"], "lint": { "extra_globals": ["g"] } }"#),
            ("luck.json", r#"{ "extends": ["base.json", "base.json"], "search_paths": ["b"] }"#),
        ]);
        let Loaded { config, files } = load(&project.path().join("luck.json")).expect("loads");
        assert_eq!(config.search_paths, Some(vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(config.lint.unwrap().extra_globals, vec!["g".to_owned()]);
        assert_eq!(files.len(), 2, "{files:?}");
    }

    #[test]
    fn non_root_config_extends_a_file_in_a_parent_directory() {
        let project = project_with(&[
            ("shared.json", r#"{ "lua": "lua52" }"#),
            ("project/luck.json", r#"{ "extends": ["../shared.json"] }"#),
        ]);
        let loaded = load(&project.path().join("project/luck.json")).expect("loads");
        assert_eq!(loaded.config.lua, Some(luck_token::LuaTarget::Lua52));
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
}
