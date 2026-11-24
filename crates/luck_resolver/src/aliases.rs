use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::file_system::FileSystem;
use crate::luaurc::parse_aliases;
use crate::path::lexically_normalized;
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

/// The aliases in effect for a search starting in one directory.
///
/// Closer `.luaurc` files win, relative targets resolve against the
/// `.luaurc` that declared them, and a malformed `.luaurc` hides everything
/// declared above it, because Luau's upward search stops with an error
/// there.
#[derive(Debug, Default)]
pub(crate) struct AliasesInEffect {
    /// By lowercased name.
    pub(crate) targets: FxHashMap<String, AliasTarget>,
    /// The nearest malformed `.luaurc` the search hits.
    pub(crate) malformed_luaurc: Option<PathBuf>,
}

/// The `.luaurc` alias tables read so far, shared by every thread.
#[derive(Debug, Default)]
pub(crate) struct AliasCache {
    declared_tables: SharedCache<OsString, Arc<Declared>>,
    /// The aliases in effect in each directory a require came from.
    merged_tables: SharedCache<OsString, Arc<AliasesInEffect>>,
}

/// What one directory's `.luaurc` declares.
#[derive(Debug)]
enum Declared {
    /// By lowercased name, and none when the directory has no `.luaurc`.
    Aliases(FxHashMap<String, String>),
    /// The `.luaurc` can't be read or doesn't parse.
    Malformed,
}

impl AliasCache {
    pub(crate) fn clear(&self) {
        self.declared_tables.clear();
        self.merged_tables.clear();
    }

    /// The aliases in effect for a search starting at `directory`.
    pub(crate) fn in_effect(
        &self,
        file_system: &impl FileSystem,
        directory: &Path,
    ) -> Arc<AliasesInEffect> {
        self.merged_tables.get_or_insert_with(directory.as_os_str(), || {
            let mut aliases = AliasesInEffect::default();
            // Only the requested directory gets a merged map. Each ancestor
            // caches just its raw table.
            let ancestors: Vec<&Path> = directory.ancestors().collect();
            for ancestor in ancestors.into_iter().rev() {
                let declared =
                    self.declared_tables.get_or_insert_with(ancestor.as_os_str(), || {
                        Arc::new(read_declared(file_system, ancestor))
                    });
                match &*declared {
                    Declared::Aliases(declared_aliases) => {
                        aliases.targets.extend(
                            declared_aliases.iter().map(|(name, target)| {
                                (name.clone(), alias_target(ancestor, target))
                            }),
                        );
                    }
                    Declared::Malformed => {
                        aliases.targets.clear();
                        aliases.malformed_luaurc = Some(ancestor.join(".luaurc"));
                    }
                }
            }
            Arc::new(aliases)
        })
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
        // Walking `..` from an alias's target must leave the directory it
        // names, not a literal `..` segment.
        AliasTarget::Directory(lexically_normalized(&declaring_directory.join(&target)))
    }
}

fn read_declared(file_system: &impl FileSystem, directory: &Path) -> Declared {
    let luaurc = directory.join(".luaurc");
    if !file_system.metadata(&luaurc).is_ok_and(|metadata| metadata.is_file) {
        return Declared::Aliases(FxHashMap::default());
    }
    file_system
        .read(&luaurc)
        .ok()
        .and_then(|contents| parse_aliases(&contents))
        .map_or(Declared::Malformed, Declared::Aliases)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::ResolveError;
    use crate::cache::Cache;
    use crate::file_system::FileSystemOs;
    use crate::fixtures::{canonical, luau_project, write_files};
    use crate::luau::Luau;

    #[test]
    fn caches_only_requested_merged_maps_and_reuses_them() {
        let project = luau_project();
        // Cache keys derive from canonical `from_file` paths, so the direct
        // lookups must use the canonical directory too.
        let source_directory = canonical(&project, "src");
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
        assert_eq!(luau.aliases().in_effect(&FileSystemOs, &source_directory).targets.len(), 64);
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
        let source_directory = canonical(&project, "src");
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
        let source_directory = canonical(&project, "src");
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
}
