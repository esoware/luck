use std::fs;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;
use serde::Deserialize;

/// The parts of a `.luaurc` (JSON5) the resolver reads.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Luaurc {
    #[expect(
        dead_code,
        reason = "never read, but a non-string value still rejects the whole file"
    )]
    language_mode: Option<String>,
    aliases: Option<FxHashMap<String, String>>,
}

/// The `.luaurc` alias tables read so far, which are never reread.
#[derive(Debug, Default)]
pub(crate) struct AliasCache {
    /// Each directory's own `.luaurc` aliases, as written.
    declared_tables: FxHashMap<PathBuf, FxHashMap<String, String>>,
    /// The aliases in effect in each directory a require came from.
    merged_tables: FxHashMap<PathBuf, FxHashMap<String, PathBuf>>,
}

impl AliasCache {
    /// Resolved `@alias` -> directory map for `directory`, keyed by lowercased
    /// alias name. Closer `.luaurc` files win; relative alias targets resolve
    /// against the `.luaurc` that declared them.
    pub(crate) fn in_effect(&mut self, directory: &Path) -> &FxHashMap<String, PathBuf> {
        self.merged_tables.entry(directory.to_path_buf()).or_insert_with(|| {
            let mut aliases = FxHashMap::default();
            // Only the requested directory gets a merged map. Each ancestor
            // caches just its raw table.
            let ancestors: Vec<&Path> = directory.ancestors().collect();
            for ancestor in ancestors.into_iter().rev() {
                let ancestor_aliases = self
                    .declared_tables
                    .entry(ancestor.to_path_buf())
                    .or_insert_with(|| read_declared_aliases(ancestor));
                aliases.extend(ancestor_aliases.iter().map(|(name, target)| {
                    (name.to_lowercase(), ancestor.join(target.replace('\\', "/")))
                }));
            }
            aliases
        })
    }
}

/// The aliases `directory/.luaurc` declares, empty when the file is missing,
/// malformed, or declares none.
fn read_declared_aliases(directory: &Path) -> FxHashMap<String, String> {
    let candidate = directory.join(".luaurc");
    if candidate.is_file()
        && let Ok(contents) = fs::read_to_string(&candidate)
        && let Ok(luaurc) = json5::from_str::<Luaurc>(&contents)
    {
        luaurc.aliases.unwrap_or_default()
    } else {
        FxHashMap::default()
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
