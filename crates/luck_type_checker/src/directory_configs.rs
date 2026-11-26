use std::cell::RefCell;
use std::path::{Path, PathBuf};

use luck_diagnostics::Diagnostic;
use luck_token::{LineIndex, Span, index_to_offset, offset_to_index};
use rustc_hash::FxHashMap;

use crate::ffi::{Config, SavedConfig};

/// The config each directory gives the modules in it, from its `.luaurc` or
/// `.config.luau` and the ones above it, read the first time a module below
/// it asks, as `luau-analyze` keeps them, which a [`Host`](crate::Host) sets
/// on each module.
#[derive(Debug, Default)]
pub struct DirectoryConfigs(RefCell<FxHashMap<PathBuf, SavedConfig>>);

impl DirectoryConfigs {
    /// Sets on `config` what governs the module at `path`: what the nearest
    /// directory above it kept, then the `.luaurc` or `.config.luau` of each
    /// directory below that one, outermost first, as `read` gives a file's
    /// text, which each of those directories keeps. As luau-analyze reads
    /// them, a directory with both applies neither, and Luau applies the keys
    /// of a config before one it can't read. It stops once the check that
    /// asked is cancelled.
    ///
    /// Returns the E014 of each config Luau couldn't read, by its path.
    pub fn apply(
        &self,
        path: &Path,
        config: &mut Config<'_>,
        read: impl Fn(&Path) -> Option<String>,
    ) -> Vec<(PathBuf, Diagnostic)> {
        let mut configs = self.0.borrow_mut();
        let mut unread = Vec::new();
        for directory in path.ancestors().skip(1) {
            if let Some(saved) = configs.get(directory) {
                config.set(saved);
                break;
            }
            unread.push(directory);
        }
        let mut errors = Vec::new();
        for directory in unread.into_iter().rev() {
            let luaurc = directory.join(".luaurc");
            let luau_config = directory.join(".config.luau");
            let applied = match (read(&luaurc), read(&luau_config)) {
                (Some(text), Some(_)) => {
                    let both = Err("Both .luaurc and .config.luau files exist".to_owned());
                    Some((luaurc, text, both))
                }
                (Some(text), None) => {
                    let applied = config.apply_luaurc(&text);
                    Some((luaurc, text, applied))
                }
                (None, Some(source)) => {
                    let applied = config.apply_luau_config(&source);
                    Some((luau_config, source, applied))
                }
                (None, None) => None,
            };
            if let Some((config_path, text, Err(error))) = applied {
                errors.push((config_path, Diagnostic::e014(error_span(&error, &text), &error)));
            }
            let Some(saved) = config.saved() else {
                break;
            };
            configs.insert(directory.to_path_buf(), saved);
        }
        errors
    }

    /// Forgets what every directory kept, so each reads its configs again.
    pub fn clear(&mut self) {
        self.0.get_mut().clear();
    }
}

/// The line Luau's error for a config names in `text`, as `at line 3` for a
/// `.luaurc` and `config:3:` for a `.config.luau`.
fn error_span(error: &str, text: &str) -> Option<Span> {
    let after = error.split_once(" at line ").map(|(_, after)| after);
    let after = after.or_else(|| error.strip_prefix("config:"))?;
    let line = after.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let start = LineIndex::new(text.as_bytes()).offset(line, 1)?;
    let line_text = text.get(offset_to_index(start)..)?.lines().next().unwrap_or_default();
    Some(Span::new(start, start + index_to_offset(line_text.len())))
}
