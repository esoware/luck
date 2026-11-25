mod glob;

use std::fs;
use std::path::{self, Path, PathBuf};
use std::sync::Arc;

use luck_formatter::{FormatConfig, IndentStyle, LineEndings};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use self::glob::SectionGlob;

pub const EDITORCONFIG_FILE_NAME: &str = ".editorconfig";

/// The longest section name read, past the 1024 bytes the spec requires. This
/// bounds what a hostile section can cost to compile and match.
const MAX_SECTION_NAME_BYTES: usize = 4096;

/// The `.editorconfig` files a run has read, each once, however many files
/// share it. An edit made during the run isn't seen.
#[derive(Debug, Default)]
pub struct Editorconfigs {
    /// The files that apply in each directory, farthest first.
    chains: Mutex<FxHashMap<PathBuf, Chain>>,
}

type Chain = Arc<[Arc<Editorconfig>]>;

/// One `.editorconfig`, parsed and with its section globs compiled.
#[derive(Debug)]
struct Editorconfig {
    directory: PathBuf,
    /// Whether a `root = true` before the first section stops the walk up
    /// at this file.
    is_root: bool,
    sections: Vec<Section>,
}

#[derive(Debug)]
struct Section {
    /// `None` for a name too long to read, which matches nothing.
    glob: Option<SectionGlob>,
    /// In file order, with lowercased keys.
    pairs: Vec<(String, String)>,
}

impl Editorconfigs {
    /// The files that apply in `directory`, an absolute path, farthest first.
    /// Two threads asking at once may both read a file, and either answer is
    /// kept.
    fn chain(&self, directory: &Path) -> Chain {
        let cached = self.chains.lock().get(directory).cloned();
        if let Some(chain) = cached {
            return chain;
        }
        let own = fs::read(directory.join(EDITORCONFIG_FILE_NAME))
            .ok()
            .map(|bytes| Editorconfig::parse(directory, &String::from_utf8_lossy(&bytes)));
        let chain: Chain = match own {
            Some(file) if file.is_root => Arc::new([Arc::new(file)]),
            Some(file) => {
                let parent_chain = directory.parent().map(|parent| self.chain(parent));
                parent_chain
                    .iter()
                    .flat_map(|chain| chain.iter().cloned())
                    .chain([Arc::new(file)])
                    .collect()
            }
            None => directory.parent().map_or_else(|| Chain::from([]), |parent| self.chain(parent)),
        };
        self.chains.lock().insert(directory.to_path_buf(), Arc::clone(&chain));
        chain
    }

    /// The formatter settings the `.editorconfig` files above `file_path` give
    /// it.
    pub(crate) fn format_config_for(&self, file_path: &Path) -> FormatConfig {
        // A relative path would hide the directories above the working
        // directory from the walk and from sections anchored at them.
        let Ok(absolute_path) = path::absolute(file_path) else {
            return FormatConfig::default();
        };
        let chain = absolute_path.parent().map(|directory| self.chain(directory));
        let properties =
            editorconfig_properties(&absolute_path, chain.as_deref().unwrap_or_default());
        let lowercase_value =
            |key: &str| properties.get(key).map(|value| value.to_ascii_lowercase());
        let number_value = |key: &str| properties.get(key)?.parse::<usize>().ok();

        let indent_style = match lowercase_value("indent_style").as_deref() {
            Some("tab") => Some(IndentStyle::Tabs),
            Some("space") => Some(IndentStyle::Spaces),
            _ => None,
        };

        let indent_width = number_value("indent_size").or_else(|| number_value("tab_width"));

        let line_endings = match lowercase_value("end_of_line").as_deref() {
            Some("lf") => Some(LineEndings::Unix),
            Some("crlf") => Some(LineEndings::Windows),
            // `LineEndings` has no bare-CR option.
            _ => None,
        };

        FormatConfig {
            // `off`, like any other non-number, leaves `line_width` unset.
            line_width: number_value("max_line_length").and_then(|width| u16::try_from(width).ok()),
            indent_style,
            indent_width: indent_width.and_then(|width| u8::try_from(width).ok()),
            line_endings,
            ..FormatConfig::default()
        }
    }
}

impl Editorconfig {
    /// Pairs before the first section, such as `root`, apply to no file, and
    /// the last `root` among them decides whether the walk stops here.
    fn parse(directory: &Path, contents: &str) -> Self {
        let mut is_root = false;
        let mut sections: Vec<Section> = Vec::new();
        let lines = contents.strip_prefix('\u{feff}').unwrap_or(contents).lines();
        for line in lines.filter_map(parse_line) {
            match line {
                EditorconfigLine::Section(name) => sections.push(Section {
                    glob: (name.len() <= MAX_SECTION_NAME_BYTES).then(|| SectionGlob::new(name)),
                    pairs: Vec::new(),
                }),
                EditorconfigLine::Pair(key, value) => match sections.last_mut() {
                    Some(section) => {
                        section.pairs.push((key.to_ascii_lowercase(), value.to_owned()));
                    }
                    None if key.eq_ignore_ascii_case("root") => {
                        is_root = value.eq_ignore_ascii_case("true");
                    }
                    None => {}
                },
            }
        }
        Self { directory: directory.to_path_buf(), is_root, sections }
    }
}

/// The properties `chain` gives `file_path`, an absolute path, keyed by
/// lowercased name. Nearer files override farther ones, and within a file
/// later sections override earlier ones.
fn editorconfig_properties<'chain>(
    file_path: &Path,
    chain: &'chain [Arc<Editorconfig>],
) -> FxHashMap<&'chain str, &'chain str> {
    let mut properties = FxHashMap::default();
    let mut subject = Vec::new();
    for file in chain {
        let relative_path = file_path.strip_prefix(&file.directory).unwrap_or(file_path);
        subject.clear();
        for component in relative_path.components() {
            subject.push('/');
            subject.extend(component.as_os_str().to_string_lossy().chars());
        }
        for section in &file.sections {
            if section.glob.as_ref().is_some_and(|glob| glob.matches(&subject)) {
                properties.extend(
                    section.pairs.iter().map(|(key, value)| (key.as_str(), value.as_str())),
                );
            }
        }
    }
    properties
}

#[derive(Debug, PartialEq, Eq)]
enum EditorconfigLine<'text> {
    Section(&'text str),
    Pair(&'text str, &'text str),
}

fn parse_line(untrimmed: &str) -> Option<EditorconfigLine<'_>> {
    let line = untrimmed.trim();
    if line.is_empty() || line.starts_with(['#', ';']) {
        return None;
    }
    if let Some(name) = section_name(line) {
        return Some(EditorconfigLine::Section(name));
    }
    let (raw_key, value) = line.split_once('=')?;
    let key = raw_key.trim_end();
    (!key.is_empty()).then_some(EditorconfigLine::Pair(key, value.trim_start()))
}

/// A name may hold `]`, and only a comment may follow the last one.
fn section_name(line: &str) -> Option<&str> {
    let (name, after_closing) = line.strip_prefix('[')?.rsplit_once(']')?;
    let trailing = after_closing.trim_start();
    let has_only_a_comment_after = trailing.is_empty() || trailing.starts_with(['#', ';']);
    (!name.is_empty() && has_only_a_comment_after).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::project_with;

    fn format_config_for(file_path: &Path) -> FormatConfig {
        Editorconfigs::default().format_config_for(file_path)
    }

    #[test]
    fn reads_basic_editorconfig() {
        let directory = project_with(&[(
            ".editorconfig",
            "\
[*.luau]
indent_style = space
indent_size = 2
end_of_line = lf
max_line_length = 80
",
        )]);
        let config = format_config_for(&directory.path().join("main.luau"));
        assert_eq!(config.indent_style, Some(IndentStyle::Spaces));
        assert_eq!(config.indent_width, Some(2));
        assert_eq!(config.line_endings, Some(LineEndings::Unix));
        assert_eq!(config.line_width, Some(80));
        assert!(config.quote_style.is_none());
    }

    #[test]
    fn tab_style_and_crlf_map_to_tabs_and_windows() {
        let directory = project_with(&[(
            ".editorconfig",
            "\
[*]
indent_style = tab
end_of_line = crlf
",
        )]);
        let config = format_config_for(&directory.path().join("main.luau"));
        assert_eq!(config.indent_style, Some(IndentStyle::Tabs));
        assert_eq!(config.line_endings, Some(LineEndings::Windows));
    }

    #[test]
    fn root_true_stops_the_walk() {
        let parent = project_with(&[
            (
                ".editorconfig",
                "\
[*]
indent_size = 8
",
            ),
            (
                "project/.editorconfig",
                "\
root = true
[*]
indent_size = 2
",
            ),
        ]);
        let config = format_config_for(&parent.path().join("project/main.luau"));
        assert_eq!(config.indent_width, Some(2));
    }

    #[test]
    fn brace_alternation_matches_luau() {
        let directory = project_with(&[(
            ".editorconfig",
            "\
[*.{lua,luau}]
indent_size = 3
",
        )]);
        let config = format_config_for(&directory.path().join("init.luau"));
        assert_eq!(config.indent_width, Some(3));
    }

    #[test]
    fn nearer_file_overrides_farther() {
        let parent = project_with(&[
            (
                ".editorconfig",
                "\
[*]
indent_size = 8
max_line_length = 100
",
            ),
            (
                "project/.editorconfig",
                "\
[*]
indent_size = 2
",
            ),
        ]);
        let config = format_config_for(&parent.path().join("project/main.luau"));
        assert_eq!(config.indent_width, Some(2));
        assert_eq!(config.line_width, Some(100));
    }

    #[test]
    fn later_section_overrides_earlier() {
        let directory = project_with(&[(
            ".editorconfig",
            "\
[*]
indent_size = 4
[*.luau]
indent_size = 2
",
        )]);
        let config = format_config_for(&directory.path().join("main.luau"));
        assert_eq!(config.indent_width, Some(2));
    }

    #[test]
    fn max_line_length_off_is_ignored() {
        let directory = project_with(&[(
            ".editorconfig",
            "\
[*]
max_line_length = off
",
        )]);
        let config = format_config_for(&directory.path().join("main.luau"));
        assert!(config.line_width.is_none());
    }

    #[test]
    fn editorconfig_that_sets_nothing_yields_all_none() {
        let directory = project_with(&[(".editorconfig", "root = true\n")]);
        let config = format_config_for(&directory.path().join("main.luau"));
        assert!(config.indent_style.is_none());
        assert!(config.indent_width.is_none());
        assert!(config.line_endings.is_none());
        assert!(config.line_width.is_none());
    }

    #[test]
    fn tab_width_used_when_indent_size_absent() {
        let directory = project_with(&[(
            ".editorconfig",
            "\
[*]
tab_width = 6
",
        )]);
        let config = format_config_for(&directory.path().join("main.luau"));
        assert_eq!(config.indent_width, Some(6));
    }

    #[test]
    fn invalid_lines_are_skipped_without_losing_the_rest() {
        let directory = project_with(&[(
            ".editorconfig",
            "\
[*]
not a pair
indent_size = 2
",
        )]);
        let config = format_config_for(&directory.path().join("main.luau"));
        assert_eq!(config.indent_width, Some(2));
    }

    #[test]
    fn unset_clears_a_value_from_a_farther_file() {
        let parent = project_with(&[
            (
                ".editorconfig",
                "\
[*]
indent_size = 8
",
            ),
            (
                "project/.editorconfig",
                "\
[*]
indent_size = unset
",
            ),
        ]);
        let config = format_config_for(&parent.path().join("project/main.luau"));
        assert!(config.indent_width.is_none());
    }

    #[test]
    fn section_header_may_hold_brackets_and_a_trailing_comment() {
        assert_eq!(parse_line("[[ab]].g] ; note"), Some(EditorconfigLine::Section("[ab]].g")));
        assert_eq!(parse_line("[]"), None);
        assert_eq!(parse_line("key ="), Some(EditorconfigLine::Pair("key", "")));
    }

    #[test]
    fn inline_comment_leaves_the_value_unparsed() {
        let directory = project_with(&[(
            ".editorconfig",
            "\
root = true
# comment
; also comment

[*.luau]

indent_size = 5 # inline kept as value? no
",
        )]);
        // The inline `# ...` is part of the value, and `5 # ...` doesn't
        // parse as a number, so indent_width stays None.
        let config = format_config_for(&directory.path().join("main.luau"));
        assert!(config.indent_width.is_none());
    }
}
