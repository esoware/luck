use std::ffi::OsString;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::file_system::FileSystem;
use crate::shared_cache::SharedCache;

#[derive(Debug)]
pub(crate) enum AliasTarget {
    Directory(PathBuf),
    /// Another alias, as written after its `@`, and any sub path, with the
    /// directory of the `.luaurc` declaring it, where the search for that
    /// alias starts.
    Chained {
        target: String,
        declared_in: PathBuf,
    },
}

/// The `.luaurc` alias tables read so far, shared by every thread.
#[derive(Debug, Default)]
pub(crate) struct AliasCache {
    declared_tables: SharedCache<OsString, Arc<DeclaredTable>>,
    /// The aliases in effect in each directory a require came from.
    merged_tables: SharedCache<OsString, Arc<FxHashMap<String, AliasTarget>>>,
}

impl AliasCache {
    pub(crate) fn clear(&self) {
        self.declared_tables.clear();
        self.merged_tables.clear();
    }

    /// The aliases in effect for a search starting at `directory`, keyed by
    /// lowercased name. Closer `.luaurc` files win, relative targets resolve
    /// against the `.luaurc` that declared them, and a malformed `.luaurc`
    /// hides everything declared above it, because Luau's upward search stops
    /// with an error there.
    pub(crate) fn in_effect(
        &self,
        fs: &impl FileSystem,
        directory: &Path,
    ) -> Arc<FxHashMap<String, AliasTarget>> {
        self.merged_tables.get_or_insert_with(directory.as_os_str(), || {
            let mut aliases = FxHashMap::default();
            // Only the requested directory gets a merged map. Each ancestor
            // caches just its raw table.
            let ancestors: Vec<&Path> = directory.ancestors().collect();
            for ancestor in ancestors.into_iter().rev() {
                let table = self.declared_tables.get_or_insert_with(ancestor.as_os_str(), || {
                    Arc::new(read_declared_aliases(fs, ancestor))
                });
                if table.is_malformed {
                    aliases.clear();
                }
                aliases.extend(
                    table
                        .iter()
                        .map(|(name, target)| (name.clone(), alias_target(ancestor, target))),
                );
            }
            Arc::new(aliases)
        })
    }

    /// The nearest malformed `.luaurc` a search from `directory` hits. Call
    /// after [`Self::in_effect`] for `directory`, which reads them.
    pub(crate) fn malformed_luaurc(&self, directory: &Path) -> Option<PathBuf> {
        directory
            .ancestors()
            .find(|ancestor| {
                self.declared_tables
                    .get(ancestor.as_os_str())
                    .is_some_and(|table| table.is_malformed)
            })
            .map(|ancestor| ancestor.join(".luaurc"))
    }
}

fn alias_target(declaring_directory: &Path, written_target: &str) -> AliasTarget {
    let target = written_target.replace('\\', "/");
    if let Some(chained) = target.strip_prefix('@') {
        AliasTarget::Chained {
            target: chained.to_owned(),
            declared_in: declaring_directory.to_path_buf(),
        }
    } else {
        AliasTarget::Directory(lexically_normalized(&declaring_directory.join(&target)))
    }
}

/// `path` with `.` and `..` collapsed, since walking `..` from an alias's
/// target must leave the directory it names, not a literal `..` segment.
fn lexically_normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            named @ (Component::Normal(_) | Component::Prefix(_) | Component::RootDir) => {
                normalized.push(named);
            }
        }
    }
    normalized
}

/// One `.luaurc`'s aliases, keyed by lowercased name.
#[derive(Debug, Default)]
struct DeclaredTable {
    aliases: FxHashMap<String, String>,
    is_malformed: bool,
}

impl Deref for DeclaredTable {
    type Target = FxHashMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.aliases
    }
}

/// Empty when the file is missing, malformed, or declares none.
fn read_declared_aliases(fs: &impl FileSystem, directory: &Path) -> DeclaredTable {
    let candidate = directory.join(".luaurc");
    if !fs.metadata(&candidate).is_ok_and(|metadata| metadata.is_file) {
        return DeclaredTable::default();
    }
    let parsed = fs.read(&candidate).ok().and_then(|contents| parse_aliases(&contents));
    let is_malformed = parsed.is_none();
    DeclaredTable { aliases: parsed.unwrap_or_default(), is_malformed }
}

/// This reads Luau's own grammar: bytes rather than UTF-8, nesting on the heap,
/// and a repeated key merging into the earlier ones.
fn parse_aliases(file_bytes: &[u8]) -> Option<FxHashMap<String, String>> {
    let mut reader =
        LuaurcReader { rest: file_bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(file_bytes) };
    let mut aliases = FxHashMap::default();
    let mut keys: Vec<&[u8]> = Vec::new();
    let mut is_in_array = false;
    if !reader.eat(b'{') {
        return None;
    }
    loop {
        if is_in_array {
            if reader.eat(b']') {
                is_in_array = false;
                keys.pop();
                reader.expect_separator(b'}')?;
            } else {
                reader.string()?;
                reader.expect_separator(b']')?;
            }
        } else if reader.eat(b'}') {
            if keys.pop().is_none() {
                return reader.peek().is_none().then_some(aliases);
            }
            reader.expect_separator(b'}')?;
        } else {
            keys.push(reader.string()?);
            if !reader.eat(b':') {
                return None;
            }
            if reader.eat(b'[') {
                is_in_array = true;
            } else if reader.eat(b'{') {
                // Its keys come next, under this one.
            } else {
                // Luau hands a boolean on as its text, so one in `aliases` is
                // just an odd target.
                let (value, is_boolean) = if reader.eat_word(b"true") {
                    (b"true".as_slice(), true)
                } else if reader.eat_word(b"false") {
                    (b"false".as_slice(), true)
                } else {
                    (reader.string()?, false)
                };
                match keys.as_slice() {
                    [b"aliases", name] if !is_valid_alias_name(name) => return None,
                    [b"aliases", name] => {
                        let lowercase_name = String::from_utf8_lossy(name).to_ascii_lowercase();
                        // `parseAlias` tests the key as written against its
                        // lowercased entries, so only a later name with an
                        // uppercase letter replaces an equal one.
                        if lowercase_name.as_bytes() != *name
                            || !aliases.contains_key(&lowercase_name)
                        {
                            aliases.insert(
                                lowercase_name,
                                String::from_utf8_lossy(value).into_owned(),
                            );
                        }
                    }
                    [b"languageMode"] if is_boolean => return None,
                    _ => {}
                }
                keys.pop();
                reader.expect_separator(b'}')?;
            }
        }
    }
}

/// Mirrors Luau's `isValidAlias`, which fails the whole file on a bad name.
fn is_valid_alias_name(name: &[u8]) -> bool {
    let unprefixed = name.strip_prefix(b"@").unwrap_or(name);
    !name.is_empty()
        && !matches!(name, b"." | b"..")
        && unprefixed.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

struct LuaurcReader<'contents> {
    rest: &'contents [u8],
}

impl<'contents> LuaurcReader<'contents> {
    /// A long comment that never closes yields its `-`, which no caller accepts.
    fn peek(&mut self) -> Option<u8> {
        loop {
            match self.rest {
                [b' ' | b'\t' | b'\r' | b'\n' | b'\x0b' | b'\x0c', tail @ ..] => {
                    self.rest = tail;
                }
                [b'/', b'/', ..] => self.skip_line(),
                [b'-', b'-', after_dashes @ ..] => {
                    let level = long_bracket_level(after_dashes);
                    match level.map(|found_level| after_long_bracket(after_dashes, found_level)) {
                        Some(Some(after_comment)) => self.rest = after_comment,
                        Some(None) => return Some(b'-'),
                        None => self.skip_line(),
                    }
                }
                [byte, ..] => return Some(*byte),
                [] => return None,
            }
        }
    }

    fn skip_line(&mut self) {
        let line_end =
            self.rest.iter().position(|b| matches!(b, b'\r' | b'\n')).unwrap_or(self.rest.len());
        self.rest = &self.rest[line_end..];
    }

    fn eat(&mut self, byte: u8) -> bool {
        let is_next = self.peek() == Some(byte);
        if is_next {
            self.rest = &self.rest[1..];
        }
        is_next
    }

    fn eat_word(&mut self, word: &[u8]) -> bool {
        self.peek();
        let after_word = self.rest.strip_prefix(word);
        if let Some(tail) = after_word {
            self.rest = tail;
        }
        after_word.is_some()
    }

    /// A value's trailing `,`, or the `closer` left for the next read.
    fn expect_separator(&mut self, closer: u8) -> Option<()> {
        (self.eat(b',') || self.peek() == Some(closer)).then_some(())
    }

    /// Luau keeps escapes as written, so this only needs them to find the closing
    /// quote.
    fn string(&mut self) -> Option<&'contents [u8]> {
        let quote = self.peek().filter(|b| matches!(b, b'"' | b'\''))?;
        let body = &self.rest[1..];
        let mut remaining = body;
        loop {
            match remaining {
                [] | [b'\n' | b'\r' | 0, ..] => return None,
                [b'\\', b'\r', b'\n', tail @ ..] | [b'\\', _, tail @ ..] => remaining = tail,
                [byte, tail @ ..] if *byte == quote => {
                    self.rest = tail;
                    return Some(&body[..body.len() - tail.len() - 1]);
                }
                [_, tail @ ..] => remaining = tail,
            }
        }
    }
}

fn long_bracket_level(text: &[u8]) -> Option<usize> {
    let after_opening = text.strip_prefix(b"[")?;
    let level = after_opening.iter().take_while(|b| **b == b'=').count();
    (after_opening.get(level) == Some(&b'[')).then_some(level)
}

fn after_long_bracket(text: &[u8], level: usize) -> Option<&[u8]> {
    let mut remaining = &text[level + 2..];
    loop {
        let closer = remaining.iter().position(|b| *b == b']')?;
        let after_closer = &remaining[closer + 1..];
        let equals_count = after_closer.iter().take_while(|b| **b == b'=').count();
        if equals_count == level && after_closer.get(level) == Some(&b']') {
            return Some(&after_closer[level + 1..]);
        }
        remaining = after_closer;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_token::LuaVersion;

    use super::*;
    use crate::cache::Cache;
    use crate::file_system::FileSystemOs;
    use crate::fixtures::{canonical, luau_project, project_with, write_files};
    use crate::luau::Luau;
    use crate::{ResolveError, ResolveOptions, Resolver};

    #[test]
    fn caches_only_requested_merged_maps_and_reuses_them() {
        let project = luau_project();
        // Cache keys derive from canonical `from_file` paths, so the direct
        // lookups must use the canonical directory too.
        let source_directory = canonical(&project.path().join("src"));
        let aliases: serde_json::Map<String, serde_json::Value> =
            (0..64).map(|i| (format!("alias{i}"), serde_json::json!("."))).collect();
        fs::write(
            source_directory.join(".luaurc"),
            serde_json::json!({"aliases": aliases}).to_string(),
        )
        .expect("write aliases");
        let current = source_directory.join("main.luau");
        let luau = Luau::default();
        let cache = Cache::new(FileSystemOs);
        let merged = Arc::as_ptr(&luau.aliases().in_effect(&FileSystemOs, &source_directory));
        assert_eq!(luau.aliases().merged_tables.len(), 1);
        assert_eq!(luau.aliases().declared_tables.len(), source_directory.ancestors().count());
        assert_eq!(luau.aliases().in_effect(&FileSystemOs, &source_directory).len(), 64);
        for module in ["@alias0/utils", "@ALIAS63/utils", "@alias0/utils"] {
            let resolved = luau.resolve(&cache, &current, module).expect("resolve cached alias");
            assert_eq!(resolved.path, source_directory.join("utils.luau"));
            assert!(!resolved.is_self_alias_shadowed);
            let reused = luau.aliases().in_effect(&FileSystemOs, &source_directory);
            assert_eq!(Arc::as_ptr(&reused), merged);
            assert_eq!(luau.aliases().merged_tables.len(), 1);
        }
    }

    #[test]
    fn cached_aliases_preserve_case_shadowing_and_declaring_directory() {
        let project = luau_project();
        let source_directory = canonical(&project.path().join("src"));
        let sibling = source_directory.with_file_name("sibling");
        fs::create_dir_all(&sibling).expect("create sibling");
        write_files(
            project.path(),
            &[
                (".luaurc", r#"{"aliases":{"Shared":"missing","Inherited":"src"}}"#),
                ("src/.luaurc", r#"{"aliases":{"sHaReD":"."}}"#),
            ],
        );
        let current = source_directory.join("main.luau");
        let sibling_file = sibling.join("main.luau");
        let luau = Luau::default();
        let cache = Cache::new(FileSystemOs);
        for _ in 0..2 {
            for module in ["@SHARED/utils", "@inherited/utils"] {
                let resolved =
                    luau.resolve(&cache, &current, module).expect("resolve inherited alias");
                assert_eq!(resolved.path, source_directory.join("utils.luau"));
            }
            let error = luau
                .resolve(&cache, &sibling_file, "@SHARED/utils")
                .expect_err("sibling must not inherit child aliases");
            assert!(matches!(error, ResolveError::NotFound(_)), "{error:?}");
        }
        assert_eq!(luau.aliases().merged_tables.len(), 2);
    }

    #[test]
    fn self_shadowing_is_reported_for_every_request_even_with_a_warm_cache() {
        let project = luau_project();
        let source_directory = canonical(&project.path().join("src"));
        write_files(project.path(), &[(".luaurc", r#"{"aliases":{"SeLf":"missing"}}"#)]);
        let current = source_directory.join("init.luau");
        let luau = Luau::default();
        let cache = Cache::new(FileSystemOs);
        for _ in 0..2 {
            let resolved =
                luau.resolve(&cache, &current, "@SELF/utils").expect("resolve built-in self");
            assert_eq!(resolved.path, source_directory.join("utils.luau"));
            assert!(resolved.is_self_alias_shadowed);
        }
        assert_eq!(luau.aliases().merged_tables.len(), 1);
    }

    #[test]
    fn a_resolver_sees_luaurc_edits_once_its_cache_clears() {
        for initial in [None, Some("{"), Some("{}"), Some(r#"{"aliases":{"cache_test":"."}}"#)] {
            let project = luau_project();
            // `nested/main.luau` never exists, so its path can't canonicalize
            // on its own and must start from the canonical `source_directory`.
            let source_directory = canonical(&project.path().join("src"));
            let nested = source_directory.join("nested");
            fs::create_dir_all(&nested).expect("create nested directory");
            let current = source_directory.join("main.luau");
            let nested_file = nested.join("main.luau");
            if let Some(contents) = initial {
                fs::write(source_directory.join(".luaurc"), contents)
                    .expect("write initial aliases");
            }
            let resolver = Resolver::new(ResolveOptions {
                version: LuaVersion::Luau,
                search_paths: Vec::new(),
                root: project.path().to_path_buf(),
            });
            let original = resolver.resolve(&current, "@cache_test/utils");
            fs::write(
                source_directory.join(".luaurc"),
                r#"{"aliases":{"cache_test":"../updated"}}"#,
            )
            .expect("update aliases");
            write_files(project.path(), &[("updated/utils.luau", "return {}")]);
            for from_file in [&current, &nested_file] {
                assert_eq!(resolver.resolve(from_file, "@cache_test/utils"), original);
            }
            resolver.clear_cache();
            for from_file in [&current, &nested_file] {
                let updated = resolver
                    .resolve(from_file, "@cache_test/utils")
                    .expect("a cleared resolver observes the edit");
                assert_eq!(updated.path, canonical(&project.path().join("updated/utils.luau")));
            }
        }
    }

    #[test]
    fn luaurc_accepts_json5_comments_and_trailing_commas() {
        let project = project_with(&[(
            ".luaurc",
            r#"{
            // This is a comment
            "languageMode": "strict",
            "aliases": {
                "utils": "../shared/utils",
                "components": "./components",
            },  // trailing comma
        }"#,
        )]);
        let aliases = read_declared_aliases(&FileSystemOs, project.path());
        assert_eq!(aliases.get("utils").map(String::as_str), Some("../shared/utils"));
        assert_eq!(aliases.get("components").map(String::as_str), Some("./components"));
    }

    #[test]
    fn luaurc_accepts_every_comment_form_the_luau_lexer_does() {
        let project = project_with(&[(
            ".luaurc",
            "-- line\r{ --[[ block\n}]] // line\n --[==[ ]] ]==] \"aliases\": {\"a\": \"./a\"} } --x",
        )]);
        let declared = read_declared_aliases(&FileSystemOs, project.path());
        assert!(!declared.is_malformed);
        assert_eq!(declared.get("a").map(String::as_str), Some("./a"));
    }

    #[test]
    fn luaurc_with_an_unterminated_long_comment_is_malformed() {
        let project = project_with(&[(
            ".luaurc",
            "\
--[[ never closed
{}",
        )]);
        assert!(read_declared_aliases(&FileSystemOs, project.path()).is_malformed);
    }

    #[test]
    fn luaurc_string_broken_by_a_carriage_return_is_malformed() {
        let project = project_with(&[(".luaurc", "{\"aliases\": {\"a\": \"./a\r\"}}")]);
        assert!(read_declared_aliases(&FileSystemOs, project.path()).is_malformed);
    }

    #[test]
    fn luaurc_boolean_alias_target_is_kept_as_its_text() {
        let project = project_with(&[(".luaurc", r#"{"aliases": {"a": true}}"#)]);
        let declared = read_declared_aliases(&FileSystemOs, project.path());
        assert!(!declared.is_malformed);
        assert_eq!(declared.get("a").map(String::as_str), Some("true"));
    }

    #[test]
    fn luaurc_without_aliases_or_unparsable_declares_no_aliases() {
        for contents in ["{}", "not valid json {{{"] {
            let project = project_with(&[(".luaurc", contents)]);
            assert!(read_declared_aliases(&FileSystemOs, project.path()).is_empty(), "{contents}");
        }
    }
}
