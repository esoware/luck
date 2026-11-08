//! `.editorconfig` support for the formatter.
//!
//! `ec4rs` handles the spec (the upward walk, `root = true`, and section
//! matching), and this module maps its properties onto the [`FormatConfig`]
//! fields they cover. It lives here so the CLI and the LSP share one
//! precedence order: built-in defaults < `.editorconfig` < the luck.json
//! `format` section.

use std::path::Path;

use ec4rs::property::{
    EndOfLine, IndentSize, IndentStyle as EditorconfigIndentStyle, MaxLineLen, TabWidth,
};

use super::format_config::{FormatConfig, IndentStyle, LineEndings};

/// Resolves the [`FormatConfig`] for a file, with the luck.json `format`
/// section winning over the `.editorconfig` chain of `file_path`.
///
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

/// Resolves a [`FormatConfig`] for `file_path` from the `.editorconfig` chain.
///
/// Returns [`FormatConfig::default`] (all `None`) when no `.editorconfig` is
/// found or none match. Only `indent_style`, `indent_width`, `line_endings`,
/// and `line_width` are ever set.
fn format_config_for(file_path: &Path) -> FormatConfig {
    let Ok(properties) = ec4rs::properties_of(file_path) else {
        return FormatConfig::default();
    };

    let indent_style = match properties.get::<EditorconfigIndentStyle>() {
        Ok(EditorconfigIndentStyle::Tabs) => Some(IndentStyle::Tabs),
        Ok(EditorconfigIndentStyle::Spaces) => Some(IndentStyle::Spaces),
        Err(_) => None,
    };

    // `indent_size` takes precedence over `tab_width`; either alone works.
    let indent_width = match properties.get::<IndentSize>() {
        Ok(IndentSize::Value(width)) => Some(width),
        Ok(IndentSize::UseTabWidth) | Err(_) => match properties.get::<TabWidth>() {
            Ok(TabWidth::Value(width)) => Some(width),
            Err(_) => None,
        },
    };

    let line_endings = match properties.get::<EndOfLine>() {
        Ok(EndOfLine::Lf) => Some(LineEndings::Unix),
        Ok(EndOfLine::CrLf) => Some(LineEndings::Windows),
        // `LineEndings` has no bare-CR option.
        Ok(EndOfLine::Cr) | Err(_) => None,
    };

    let line_width = match properties.get::<MaxLineLen>() {
        Ok(MaxLineLen::Value(length)) => u16::try_from(length).ok(),
        // `off` disables the limit; leave `line_width` unset.
        Ok(MaxLineLen::Off) | Err(_) => None,
    };

    FormatConfig {
        line_width,
        indent_style,
        indent_width: indent_width.and_then(|width| u8::try_from(width).ok()),
        line_endings,
        ..FormatConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

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
        // Parent sets indent_size = 8; it must be ignored because the child is root.
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
        // Child overrides indent_size but not max_line_length.
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
    fn no_editorconfig_yields_all_none() {
        let directory = tempfile::tempdir().expect("tempdir");
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
    fn inline_comment_leaves_the_value_unparsed() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join(".editorconfig"),
            "# comment\n; also comment\n\n[*.lua]\n\nindent_size = 5 # inline kept as value? no\n",
        )
        .expect("write");
        // The inline `# ...` is part of the value, and `5 # ...` doesn't
        // parse as u8, so indent_width stays None.
        let config = format_config_for(&directory.path().join("main.lua"));
        assert!(config.indent_width.is_none());
    }
}
