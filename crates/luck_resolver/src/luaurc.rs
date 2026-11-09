use core::ops::Deref;
use std::fs;
use std::path::{Component, Path, PathBuf};

use luck_core::native_path;
use rustc_hash::FxHashMap;

/// What an alias names once its `.luaurc` is read.
#[derive(Debug)]
pub(crate) enum AliasTarget {
    Directory(PathBuf),
    /// Another alias, as written after its `@`, and any sub path.
    Chained(String),
}

/// The `.luaurc` alias tables read so far, which are never reread.
#[derive(Debug, Default)]
pub(crate) struct AliasCache {
    /// Each directory's own `.luaurc` aliases.
    declared_tables: FxHashMap<PathBuf, DeclaredTable>,
    /// The aliases in effect in each directory a require came from.
    merged_tables: FxHashMap<PathBuf, FxHashMap<String, AliasTarget>>,
}

impl AliasCache {
    /// The aliases in effect for a search starting at `directory`, keyed by
    /// lowercased name. Closer `.luaurc` files win, relative targets resolve
    /// against the `.luaurc` that declared them, and a malformed `.luaurc`
    /// hides everything declared above it, because Luau's upward search stops
    /// with an error there.
    pub(crate) fn in_effect(&mut self, directory: &Path) -> &FxHashMap<String, AliasTarget> {
        self.merged_tables.entry(directory.to_path_buf()).or_insert_with(|| {
            let mut aliases = FxHashMap::default();
            // Only the requested directory gets a merged map. Each ancestor
            // caches just its raw table.
            let ancestors: Vec<&Path> = directory.ancestors().collect();
            for ancestor in ancestors.into_iter().rev() {
                let table = self
                    .declared_tables
                    .entry(ancestor.to_path_buf())
                    .or_insert_with(|| read_declared_aliases(ancestor));
                if table.is_malformed {
                    aliases.clear();
                }
                aliases.extend(
                    table
                        .iter()
                        .map(|(name, target)| (name.clone(), alias_target(ancestor, target))),
                );
            }
            aliases
        })
    }

    /// The nearest malformed `.luaurc` a search from `directory` hits. Call
    /// after [`Self::in_effect`] for `directory`, which reads them.
    pub(crate) fn malformed_luaurc(&self, directory: &Path) -> Option<PathBuf> {
        directory
            .ancestors()
            .find(|ancestor| {
                self.declared_tables.get(*ancestor).is_some_and(|table| table.is_malformed)
            })
            .map(|ancestor| ancestor.join(".luaurc"))
    }
}

fn alias_target(declaring_directory: &Path, written_target: &str) -> AliasTarget {
    let target = written_target.replace('\\', "/");
    target.strip_prefix('@').map_or_else(
        || {
            AliasTarget::Directory(lexically_normalized(
                &declaring_directory.join(native_path(&target)),
            ))
        },
        |chained| AliasTarget::Chained(chained.to_owned()),
    )
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

/// The aliases `directory/.luaurc` declares, with the last of any names equal
/// after lowercasing kept, as in Luau. Empty when the file is missing,
/// malformed, or declares none.
fn read_declared_aliases(directory: &Path) -> DeclaredTable {
    let candidate = directory.join(".luaurc");
    if !candidate.is_file() {
        return DeclaredTable::default();
    }
    let parsed = fs::read(&candidate).ok().and_then(|contents| parse_aliases(&contents));
    let is_malformed = parsed.is_none();
    DeclaredTable { aliases: parsed.unwrap_or_default(), is_malformed }
}

/// The `aliases` of a `.luaurc`, or `None` when it doesn't parse.
///
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
                let value = if reader.eat_word(b"true") || reader.eat_word(b"false") {
                    None
                } else {
                    Some(reader.string()?)
                };
                match (keys.as_slice(), value) {
                    ([b"aliases", name], Some(target)) if is_valid_alias_name(name) => {
                        aliases.insert(
                            String::from_utf8_lossy(name).to_ascii_lowercase(),
                            String::from_utf8_lossy(target).into_owned(),
                        );
                    }
                    ([b"aliases", _], _) | ([b"languageMode"], None) => return None,
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
        && unprefixed
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

struct LuaurcReader<'contents> {
    rest: &'contents [u8],
}

impl<'contents> LuaurcReader<'contents> {
    /// The next byte past any whitespace and `//` or `--` comments.
    fn peek(&mut self) -> Option<u8> {
        loop {
            match self.rest {
                [byte, tail @ ..] if byte.is_ascii_whitespace() => self.rest = tail,
                [b'/', b'/', ..] | [b'-', b'-', ..] => {
                    let line_end =
                        self.rest.iter().position(|byte| *byte == b'\n').unwrap_or(self.rest.len());
                    self.rest = self.rest.get(line_end..).unwrap_or_default();
                }
                [byte, ..] => return Some(*byte),
                [] => return None,
            }
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        let is_next = self.peek() == Some(byte);
        if is_next {
            self.rest = self.rest.get(1..).unwrap_or_default();
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

    /// The next quoted string's raw bytes. Luau keeps escapes as written, so
    /// this only needs them to find the closing quote.
    fn string(&mut self) -> Option<&'contents [u8]> {
        let quote = self.peek().filter(|byte| matches!(byte, b'"' | b'\''))?;
        let body = self.rest.get(1..)?;
        let mut remaining = body;
        loop {
            match remaining {
                [] | [b'\n', ..] => return None,
                [b'\\', _, tail @ ..] => remaining = tail,
                [byte, tail @ ..] if *byte == quote => {
                    self.rest = tail;
                    let text_length = body.len().saturating_sub(tail.len()).saturating_sub(1);
                    return body.get(..text_length);
                }
                [_, tail @ ..] => remaining = tail,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use luck_core::normalize_path;
    use luck_token::{LuaVersion, Span};

    use super::*;
    use crate::fixtures::{luau_project, project_with, request, write_files};
    use crate::{ResolveRequest, Resolver, luau};

    #[test]
    fn caches_only_requested_merged_maps_and_reuses_them() {
        let project = luau_project();
        // Cache keys derive from canonical `from_file` paths, so the direct
        // lookups must use the canonical directory too.
        let source_directory = PathBuf::from(normalize_path(&project.path().join("src")));
        let aliases: serde_json::Map<String, serde_json::Value> =
            (0u8..64).map(|index| (format!("alias{index}"), serde_json::json!("."))).collect();
        fs::write(
            source_directory.join(".luaurc"),
            serde_json::json!({"aliases": aliases}).to_string(),
        )
        .expect("write aliases");
        let current = normalize_path(&source_directory.join("main.luau"));
        let mut cache = AliasCache::default();
        let cached = ptr::from_ref(cache.in_effect(&source_directory));
        assert_eq!(cache.merged_tables.len(), 1);
        assert_eq!(cache.declared_tables.len(), source_directory.ancestors().count());
        assert_eq!(cache.in_effect(&source_directory).len(), 64);
        for module in ["@alias0/utils", "@ALIAS63/utils", "@self/utils", "@alias0/utils"] {
            let resolved = luau::resolve(&mut cache, &request(module, &current))
                .expect("resolve cached alias");
            assert_eq!(resolved.path, normalize_path(&source_directory.join("utils.luau")));
            assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
            assert_eq!(ptr::from_ref(cache.in_effect(&source_directory)), cached);
            assert_eq!(cache.merged_tables.len(), 1);
        }
    }

    #[test]
    fn cached_aliases_preserve_case_shadowing_and_declaring_directory() {
        let project = luau_project();
        let source_directory = project.path().join("src");
        let sibling = project.path().join("sibling");
        fs::create_dir_all(&sibling).expect("create sibling");
        write_files(
            project.path(),
            &[
                (".luaurc", r#"{"aliases":{"Shared":"missing","Inherited":"src"}}"#),
                ("src/.luaurc", r#"{"aliases":{"sHaReD":"."}}"#),
            ],
        );
        let current = normalize_path(&source_directory.join("main.luau"));
        let sibling_file = normalize_path(&sibling.join("main.luau"));
        let mut cache = AliasCache::default();
        for _ in 0u8..2 {
            for module in ["@SHARED/utils", "@inherited/utils"] {
                let resolved = luau::resolve(&mut cache, &request(module, &current))
                    .expect("resolve inherited alias");
                assert_eq!(resolved.path, normalize_path(&source_directory.join("utils.luau")));
            }
            let error = luau::resolve(&mut cache, &request("@SHARED/utils", &sibling_file))
                .expect_err("sibling must not inherit child aliases");
            assert_eq!(error.code, "E004");
        }
        assert_eq!(cache.merged_tables.len(), 2);
    }

    #[test]
    fn self_shadow_warnings_are_per_request_even_with_a_warm_cache() {
        let project = luau_project();
        let source_directory = project.path().join("src");
        write_files(project.path(), &[(".luaurc", r#"{"aliases":{"SeLf":"missing"}}"#)]);
        let current = normalize_path(&source_directory.join("init.luau"));
        let mut cache = AliasCache::default();
        for span in [Span::new(0, 10), Span::new(20, 30)] {
            let resolved = luau::resolve(
                &mut cache,
                &ResolveRequest { module: "@SELF/utils", from_file: &current, span },
            )
            .expect("resolve built-in self");
            assert_eq!(resolved.path, normalize_path(&source_directory.join("utils.luau")));
            assert_eq!(resolved.warnings.len(), 1, "{:?}", resolved.warnings);
            assert_eq!(resolved.warnings[0].code, "W004");
            assert_eq!(resolved.warnings[0].span, span);
        }
        assert_eq!(cache.merged_tables.len(), 1);
    }

    #[test]
    fn alias_cache_changes_are_observed_only_by_a_fresh_resolver() {
        for initial in [None, Some("{"), Some("{}"), Some(r#"{"aliases":{"cache_test":"."}}"#)] {
            let project = luau_project();
            // `nested/main.luau` never exists, so its path can't canonicalize
            // on its own and must start from the canonical `source_directory`.
            let source_directory = PathBuf::from(normalize_path(&project.path().join("src")));
            let nested = source_directory.join("nested");
            fs::create_dir_all(&nested).expect("create nested directory");
            let current = normalize_path(&source_directory.join("main.luau"));
            let nested_file = normalize_path(&nested.join("main.luau"));
            if let Some(contents) = initial {
                fs::write(source_directory.join(".luaurc"), contents)
                    .expect("write initial aliases");
            }
            let mut resolver = Resolver::new(LuaVersion::Luau, &[], project.path());
            let original = resolver
                .resolve(&request("@cache_test/utils", &current))
                .map(|resolved| resolved.path);
            fs::write(
                source_directory.join(".luaurc"),
                r#"{"aliases":{"cache_test":"../updated"}}"#,
            )
            .expect("update aliases");
            write_files(project.path(), &[("updated/utils.luau", "return {}")]);
            for from_file in [&current, &nested_file] {
                let cached = resolver
                    .resolve(&request("@cache_test/utils", from_file))
                    .map(|resolved| resolved.path);
                match &original {
                    Ok(path) => assert_eq!(cached.expect("cached alias"), *path),
                    Err(_) => assert_eq!(cached.expect_err("cached missing alias").code, "E004"),
                }
                let fresh = Resolver::new(LuaVersion::Luau, &[], project.path())
                    .resolve(&request("@cache_test/utils", from_file))
                    .expect("fresh resolver observes edit");
                assert_eq!(fresh.path, normalize_path(&project.path().join("updated/utils.luau")));
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
        let aliases = read_declared_aliases(project.path());
        assert_eq!(aliases.get("utils").map(String::as_str), Some("../shared/utils"));
        assert_eq!(aliases.get("components").map(String::as_str), Some("./components"));
    }

    #[test]
    fn luaurc_without_aliases_or_unparsable_declares_no_aliases() {
        for contents in ["{}", "not valid json {{{"] {
            let project = project_with(&[(".luaurc", contents)]);
            assert!(read_declared_aliases(project.path()).is_empty(), "{contents}");
        }
    }
}
