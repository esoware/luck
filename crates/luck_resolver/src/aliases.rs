use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::error::ResolveError;
use crate::file_system::FileSystem;
use crate::luaurc::parse_aliases;
use crate::path::lexically_normalized;
use crate::resolver::LuauConfigAliases;
use crate::shared_cache::{Generation, SharedCache};

#[derive(Debug)]
pub(crate) enum AliasTarget {
    Directory(PathBuf),
    /// Another alias, as written after its `@`, and any sub path, with the
    /// directory of the config declaring it, where the search for that alias
    /// starts.
    Chained {
        target: String,
        declared_in: PathBuf,
    },
}

/// The aliases in effect for a search starting in one directory.
///
/// Each directory's config is its `.luaurc` or its `.config.luau`. Closer
/// configs win, relative targets resolve against the config that declared
/// them, and a config that doesn't load, or a directory with both, hides
/// everything declared above it, because Luau's upward search stops with an
/// error there.
#[derive(Debug, Default)]
pub(crate) struct AliasesInEffect {
    /// By lowercased name.
    pub(crate) targets: FxHashMap<String, AliasTarget>,
    /// What's wrong with the nearest config the search can't read.
    pub(crate) invalid_config: Option<ResolveError>,
}

/// The alias tables read so far, shared by every thread.
#[derive(Debug)]
pub(crate) struct AliasCache {
    declared_tables: SharedCache<OsString, Arc<Declared>>,
    /// The aliases in effect in each directory a require came from.
    merged_tables: SharedCache<OsString, Arc<AliasesInEffect>>,
}

/// What one directory's config declares.
#[derive(Debug)]
enum Declared {
    /// By lowercased name, and none when the directory has no config.
    Aliases(FxHashMap<String, String>),
    /// [`ResolveError::MalformedConfig`] or [`ResolveError::AmbiguousConfig`].
    Invalid(ResolveError),
}

impl AliasCache {
    pub(crate) fn new(generation: &Arc<Generation>) -> Self {
        Self {
            declared_tables: SharedCache::new(generation),
            merged_tables: SharedCache::new(generation),
        }
    }

    pub(crate) fn clear(&self) {
        self.declared_tables.clear();
        self.merged_tables.clear();
    }

    /// The aliases in effect for a search starting at `directory`.
    pub(crate) fn in_effect(
        &self,
        file_system: &impl FileSystem,
        luau_config: Option<LuauConfigAliases>,
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
                        Arc::new(read_declared(file_system, luau_config, ancestor))
                    });
                match &*declared {
                    Declared::Aliases(declared_aliases) => {
                        aliases.targets.extend(
                            declared_aliases.iter().map(|(name, target)| {
                                (name.clone(), alias_target(ancestor, target))
                            }),
                        );
                    }
                    Declared::Invalid(error) => {
                        aliases.targets.clear();
                        aliases.invalid_config = Some(error.clone());
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

/// A `.config.luau` is Luau code, so only `luau_config` can say what it
/// declares, and without it, one doesn't load.
fn read_declared(
    file_system: &impl FileSystem,
    luau_config: Option<LuauConfigAliases>,
    directory: &Path,
) -> Declared {
    let luaurc = directory.join(".luaurc");
    let config_luau = directory.join(".config.luau");
    let is_file = |path: &Path| file_system.metadata(path).is_ok_and(|metadata| metadata.is_file);
    let (config, aliases) = match (is_file(&luaurc), is_file(&config_luau)) {
        (false, false) => return Declared::Aliases(FxHashMap::default()),
        (true, true) => {
            return Declared::Invalid(ResolveError::AmbiguousConfig(directory.to_path_buf()));
        }
        (true, false) => {
            let aliases =
                file_system.read(&luaurc).ok().and_then(|contents| parse_aliases(&contents));
            (luaurc, aliases)
        }
        (false, true) => {
            let contents = file_system.read(&config_luau).ok();
            let aliases = luau_config.zip(contents).and_then(|(read_aliases, contents)| {
                Some(read_aliases(str::from_utf8(&contents).ok()?).ok()?.into_iter().collect())
            });
            (config_luau, aliases)
        }
    };
    aliases.map_or(Declared::Invalid(ResolveError::MalformedConfig(config)), Declared::Aliases)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::Resolver;
    use crate::file_system::FileSystemOs;
    use crate::fixtures::{canonical, sample_project, write_files};

    #[test]
    fn caches_only_requested_merged_maps_and_reuses_them() {
        let project = sample_project();
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
        let resolver = Resolver::new();
        let merged =
            Arc::as_ptr(&resolver.aliases().in_effect(&FileSystemOs, None, &source_directory));
        assert_eq!(resolver.aliases().merged_tables.len(), 1);
        assert_eq!(resolver.aliases().declared_tables.len(), source_directory.ancestors().count());
        assert_eq!(
            resolver.aliases().in_effect(&FileSystemOs, None, &source_directory).targets.len(),
            64
        );
        for module in ["@alias0/utils", "@ALIAS63/utils", "@alias0/utils"] {
            let resolution = resolver.resolve(&current, module).expect("resolve cached alias");
            assert_eq!(resolution.path, source_directory.join("utils.luau"));
            assert!(!resolution.is_self_alias_shadowed);
            let reused = resolver.aliases().in_effect(&FileSystemOs, None, &source_directory);
            assert_eq!(Arc::as_ptr(&reused), merged);
            assert_eq!(resolver.aliases().merged_tables.len(), 1);
        }
    }

    #[test]
    fn cached_aliases_preserve_case_shadowing_and_declaring_directory() {
        let project = sample_project();
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
        let resolver = Resolver::new();
        for _ in 0..2 {
            for module in ["@SHARED/utils", "@inherited/utils"] {
                let resolution =
                    resolver.resolve(&current, module).expect("resolve inherited alias");
                assert_eq!(resolution.path, source_directory.join("utils.luau"));
            }
            let error = resolver
                .resolve(&sibling_file, "@SHARED/utils")
                .expect_err("sibling must not inherit child aliases");
            assert!(matches!(error, ResolveError::NotFound(_)), "{error:?}");
        }
        assert_eq!(resolver.aliases().merged_tables.len(), 2);
    }

    #[test]
    fn self_shadowing_is_reported_for_every_request_even_with_a_warm_cache() {
        let project = sample_project();
        let source_directory = canonical(&project, "src");
        write_files(project.path(), &[(".luaurc", r#"{"aliases":{"SeLf":"missing"}}"#)]);
        let current = source_directory.join("init.luau");
        let resolver = Resolver::new();
        for _ in 0..2 {
            let resolution =
                resolver.resolve(&current, "@SELF/utils").expect("resolve built-in self");
            assert_eq!(resolution.path, source_directory.join("utils.luau"));
            assert!(resolution.is_self_alias_shadowed);
        }
        assert_eq!(resolver.aliases().merged_tables.len(), 1);
    }
}
