//! Lives here so the CLI and the LSP share one precedence order: built-in
//! defaults < `.editorconfig` < the luck.json `format` section.

mod glob;

use std::fs;
use std::path::{self, Path};

use rustc_hash::FxHashMap;

use self::glob::SectionGlob;
use crate::format_config::{FormatConfig, IndentStyle, LineEndings};

const EDITORCONFIG_FILE_NAME: &str = ".editorconfig";

/// The longest section name read, past the 1024 bytes the spec requires. This
/// bounds what a hostile section can cost to compile and match.
const MAX_SECTION_NAME_BYTES: usize = 4096;

/// Without a `file_path`, for a document that isn't on disk or a caller that
/// ignores `.editorconfig`, the `format` section stands alone. The formatter
/// fills in its own defaults for whatever neither sets.
#[must_use]
pub fn resolved_format_config(
    project_format: Option<FormatConfig>,
    file_path: Option<&Path>,
) -> FormatConfig {
    let Some(path) = file_path else {
        return project_format.unwrap_or_default();
    };
    let editorconfig = format_config_for(path);
    project_format.map_or(editorconfig, |format| format.merge_onto(editorconfig))
}

fn format_config_for(file_path: &Path) -> FormatConfig {
    // A relative path would hide the directories above the working directory
    // from the walk and from sections anchored at them.
    let Ok(absolute_path) = path::absolute(file_path) else {
        return FormatConfig::default();
    };
    let properties = editorconfig_properties(&absolute_path);
    let lowercase_value = |k: &str| properties.get(k).map(|v| v.to_ascii_lowercase());
    let number_value = |k: &str| properties.get(k)?.parse::<usize>().ok();

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
        line_width: number_value("max_line_length").and_then(|w| u16::try_from(w).ok()),
        indent_style,
        indent_width: indent_width.and_then(|w| u8::try_from(w).ok()),
        line_endings,
        ..FormatConfig::default()
    }
}

/// The properties applying to `file_path`, an absolute path, keyed by
/// lowercased name. Nearer files override farther ones, and within a file
/// later sections override earlier ones.
fn editorconfig_properties(file_path: &Path) -> FxHashMap<String, String> {
    let mut files = Vec::new();
    for directory in file_path.ancestors().skip(1) {
        if let Ok(bytes) = fs::read(directory.join(EDITORCONFIG_FILE_NAME)) {
            let contents = String::from_utf8_lossy(&bytes).into_owned();
            let is_root = is_root_file(&contents);
            files.push((directory, contents));
            if is_root {
                break;
            }
        }
    }

    let mut properties = FxHashMap::default();
    for (directory, contents) in files.iter().rev() {
        let relative_path = file_path.strip_prefix(directory).unwrap_or(file_path);
        let mut subject = Vec::new();
        for component in relative_path.components() {
            subject.push('/');
            subject.extend(component.as_os_str().to_string_lossy().chars());
        }
        apply_file(contents, &subject, &mut properties);
    }
    properties
}

fn is_root_file(contents: &str) -> bool {
    parsed_lines(contents)
        .map_while(|line| match line {
            EditorconfigLine::Pair(key, value) => Some((key, value)),
            EditorconfigLine::Section(_) => None,
        })
        .filter(|(k, _)| k.eq_ignore_ascii_case("root"))
        .last()
        .is_some_and(|(_, v)| v.eq_ignore_ascii_case("true"))
}

fn apply_file(contents: &str, subject: &[char], properties: &mut FxHashMap<String, String>) {
    let mut is_applicable = false;
    for line in parsed_lines(contents) {
        match line {
            EditorconfigLine::Section(name) => {
                is_applicable =
                    name.len() <= MAX_SECTION_NAME_BYTES && SectionGlob::new(name).matches(subject);
            }
            EditorconfigLine::Pair(key, value) if is_applicable => {
                properties.insert(key.to_ascii_lowercase(), value.to_owned());
            }
            EditorconfigLine::Pair(..) => {}
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EditorconfigLine<'text> {
    Section(&'text str),
    Pair(&'text str, &'text str),
}

fn parsed_lines(contents: &str) -> impl Iterator<Item = EditorconfigLine<'_>> {
    contents.strip_prefix('\u{feff}').unwrap_or(contents).lines().filter_map(parse_line)
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

    #[test]
    fn reads_basic_editorconfig() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join(".editorconfig"),
            "[*.lua]\nindent_style = space\nindent_size = 2\nend_of_line = lf\nmax_line_length = 80\n",
        )
        .expect("write");
        let config = format_config_for(&directory.path().join("main.lua"));
        assert_eq!(config.indent_style, Some(IndentStyle::Spaces));
        assert_eq!(config.indent_width, Some(2));
        assert_eq!(config.line_endings, Some(LineEndings::Unix));
        assert_eq!(config.line_width, Some(80));
        assert!(config.quote_style.is_none());
    }

    #[test]
    fn tab_style_and_crlf_map_to_tabs_and_windows() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join(".editorconfig"),
            "[*]\nindent_style = tab\nend_of_line = crlf\n",
        )
        .expect("write");
        let config = format_config_for(&directory.path().join("main.lua"));
        assert_eq!(config.indent_style, Some(IndentStyle::Tabs));
        assert_eq!(config.line_endings, Some(LineEndings::Windows));
    }

    #[test]
    fn root_true_stops_the_walk() {
        let parent = tempfile::tempdir().expect("tempdir");
        fs::write(parent.path().join(".editorconfig"), "[*]\nindent_size = 8\n")
            .expect("write parent");
        let child = parent.path().join("project");
        fs::create_dir_all(&child).expect("mkdir");
        fs::write(child.join(".editorconfig"), "root = true\n[*]\nindent_size = 2\n")
            .expect("write child");
        let config = format_config_for(&child.join("main.lua"));
        assert_eq!(config.indent_width, Some(2));
    }

    #[test]
    fn brace_alternation_matches_luau() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join(".editorconfig"), "[*.{lua,luau}]\nindent_size = 3\n")
            .expect("write");
        let config = format_config_for(&directory.path().join("init.luau"));
        assert_eq!(config.indent_width, Some(3));
    }

    #[test]
    fn nearer_file_overrides_farther() {
        let parent = tempfile::tempdir().expect("tempdir");
        fs::write(
            parent.path().join(".editorconfig"),
            "[*]\nindent_size = 8\nmax_line_length = 100\n",
        )
        .expect("write parent");
        let child = parent.path().join("project");
        fs::create_dir_all(&child).expect("mkdir");
        fs::write(child.join(".editorconfig"), "[*]\nindent_size = 2\n").expect("write child");
        let config = format_config_for(&child.join("main.lua"));
        assert_eq!(config.indent_width, Some(2));
        assert_eq!(config.line_width, Some(100));
    }

    #[test]
    fn later_section_overrides_earlier() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join(".editorconfig"),
            "[*]\nindent_size = 4\n[*.lua]\nindent_size = 2\n",
        )
        .expect("write");
        let config = format_config_for(&directory.path().join("main.lua"));
        assert_eq!(config.indent_width, Some(2));
    }

    #[test]
    fn max_line_length_off_is_ignored() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join(".editorconfig"), "[*]\nmax_line_length = off\n")
            .expect("write");
        let config = format_config_for(&directory.path().join("main.lua"));
        assert!(config.line_width.is_none());
    }

    #[test]
    fn editorconfig_that_sets_nothing_yields_all_none() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join(".editorconfig"), "root = true\n").expect("write");
        let config = format_config_for(&directory.path().join("main.lua"));
        assert!(config.indent_style.is_none());
        assert!(config.indent_width.is_none());
        assert!(config.line_endings.is_none());
        assert!(config.line_width.is_none());
    }

    #[test]
    fn luck_json_wins_over_editorconfig() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join(".editorconfig"), "[*.lua]\nindent_size = 2\n")
            .expect("write");
        let file_path = directory.path().join("main.lua");
        let project = FormatConfig { indent_width: Some(4), ..FormatConfig::default() };
        let resolved = resolved_format_config(Some(project), Some(&file_path));
        assert_eq!(resolved.indent_width, Some(4));
    }

    #[test]
    fn tab_width_used_when_indent_size_absent() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join(".editorconfig"), "[*]\ntab_width = 6\n").expect("write");
        let config = format_config_for(&directory.path().join("main.lua"));
        assert_eq!(config.indent_width, Some(6));
    }

    #[test]
    fn invalid_lines_are_skipped_without_losing_the_rest() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join(".editorconfig"), "[*]\nnot a pair\nindent_size = 2\n")
            .expect("write");
        let config = format_config_for(&directory.path().join("main.lua"));
        assert_eq!(config.indent_width, Some(2));
    }

    #[test]
    fn unset_clears_a_value_from_a_farther_file() {
        let parent = tempfile::tempdir().expect("tempdir");
        fs::write(parent.path().join(".editorconfig"), "[*]\nindent_size = 8\n")
            .expect("write parent");
        let child = parent.path().join("project");
        fs::create_dir_all(&child).expect("mkdir");
        fs::write(child.join(".editorconfig"), "[*]\nindent_size = unset\n").expect("write child");
        let config = format_config_for(&child.join("main.lua"));
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
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join(".editorconfig"),
            "root = true\n# comment\n; also comment\n\n[*.lua]\n\nindent_size = 5 # inline kept as value? no\n",
        )
        .expect("write");
        // The inline `# ...` is part of the value, and `5 # ...` doesn't
        // parse as u8, so indent_width stays None.
        let config = format_config_for(&directory.path().join("main.lua"));
        assert!(config.indent_width.is_none());
    }
}
