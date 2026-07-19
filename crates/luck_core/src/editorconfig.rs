//! `.editorconfig` support for the formatter.
//!
//! Resolves a [`FormatConfig`] for a given file via the spec-compliant
//! `ec4rs` engine (upward walk, `root = true`, section matching), then maps
//! editorconfig properties onto the subset of `FormatConfig` fields they
//! correspond to. Lives here (rather than in the CLI) so the LSP can adopt
//! the same precedence rules later.
//!
//! Precedence: built-in defaults < `.editorconfig` < the luck.json `format`
//! section. The luck.json values always win.

use crate::config::{FormatConfig, merge_format};
use crate::format_options::{IndentStyle, LineEndings};
use ec4rs::property::{EndOfLine, IndentSize, IndentStyle as EcIndentStyle, MaxLineLen, TabWidth};
use std::path::Path;

/// Resolve a [`FormatConfig`] for `file_path` from the `.editorconfig` chain.
///
/// Returns [`FormatConfig::default`] (all `None`) when no `.editorconfig` is
/// found or none match. Only `indent_style`, `indent_width`, `line_endings`,
/// and `line_width` are ever populated.
pub fn format_config_for(file_path: &Path) -> FormatConfig {
    let Ok(properties) = ec4rs::properties_of(file_path) else {
        return FormatConfig::default();
    };

    let mut config = FormatConfig::default();

    match properties.get::<EcIndentStyle>() {
        Ok(EcIndentStyle::Tabs) => config.indent_style = Some(IndentStyle::Tabs),
        Ok(EcIndentStyle::Spaces) => config.indent_style = Some(IndentStyle::Spaces),
        Err(_) => {}
    }

    // `indent_size` takes precedence over `tab_width`; either alone works.
    let indent_width = match properties.get::<IndentSize>() {
        Ok(IndentSize::Value(width)) => Some(width),
        Ok(IndentSize::UseTabWidth) | Err(_) => match properties.get::<TabWidth>() {
            Ok(TabWidth::Value(width)) => Some(width),
            Err(_) => None,
        },
    };
    if let Some(width) = indent_width
        && let Ok(width) = u8::try_from(width)
    {
        config.indent_width = Some(width);
    }

    match properties.get::<EndOfLine>() {
        Ok(EndOfLine::Lf) => config.line_endings = Some(LineEndings::Unix),
        Ok(EndOfLine::CrLf) => config.line_endings = Some(LineEndings::Windows),
        // Bare-CR files predate Lua itself; no luck option maps to them.
        Ok(EndOfLine::Cr) | Err(_) => {}
    }

    match properties.get::<MaxLineLen>() {
        Ok(MaxLineLen::Value(len)) => {
            if let Ok(len) = u16::try_from(len) {
                config.line_width = Some(len);
            }
        }
        // `off` disables the limit; leave `line_width` unset.
        Ok(MaxLineLen::Off) | Err(_) => {}
    }

    config
}

/// Resolve the final [`FormatConfig`] for a file, layering the luck.json
/// `format` section over the `.editorconfig`-derived base so luck.json wins.
///
/// When `use_editorconfig` is false the base is empty, so only luck.json (and
/// the formatter's own defaults downstream) apply.
pub fn resolved_format_config(
    project_format: Option<&FormatConfig>,
    file_path: &Path,
    use_editorconfig: bool,
) -> FormatConfig {
    let base = if use_editorconfig {
        format_config_for(file_path)
    } else {
        FormatConfig::default()
    };
    merge_format(project_format.cloned(), Some(base)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_basic_editorconfig() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*.lua]\nindent_style = space\nindent_size = 2\nend_of_line = lf\nmax_line_length = 80\n",
        )
        .expect("write");
        let config = format_config_for(&dir.path().join("main.lua"));
        assert_eq!(config.indent_style, Some(IndentStyle::Spaces));
        assert_eq!(config.indent_width, Some(2));
        assert_eq!(config.line_endings, Some(LineEndings::Unix));
        assert_eq!(config.line_width, Some(80));
        // Untouched fields stay None.
        assert!(config.quote_style.is_none());
    }

    #[test]
    fn tab_style_maps_to_tabs() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*]\nindent_style = tab\nend_of_line = crlf\n",
        )
        .expect("write");
        let config = format_config_for(&dir.path().join("main.lua"));
        assert_eq!(config.indent_style, Some(IndentStyle::Tabs));
        assert_eq!(config.line_endings, Some(LineEndings::Windows));
    }

    #[test]
    fn root_true_stops_the_walk() {
        let base = tempfile::tempdir().expect("tempdir");
        // Parent sets indent_size = 8; it must be ignored because the child is root.
        std::fs::write(base.path().join(".editorconfig"), "[*]\nindent_size = 8\n")
            .expect("write parent");
        let child = base.path().join("project");
        std::fs::create_dir_all(&child).expect("mkdir");
        std::fs::write(
            child.join(".editorconfig"),
            "root = true\n[*]\nindent_size = 2\n",
        )
        .expect("write child");
        let config = format_config_for(&child.join("main.lua"));
        assert_eq!(config.indent_width, Some(2));
    }

    #[test]
    fn brace_alternation_matches_luau() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*.{lua,luau}]\nindent_size = 3\n",
        )
        .expect("write");
        let config = format_config_for(&dir.path().join("init.luau"));
        assert_eq!(config.indent_width, Some(3));
    }

    #[test]
    fn nearer_file_overrides_farther() {
        let base = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            base.path().join(".editorconfig"),
            "[*]\nindent_size = 8\nmax_line_length = 100\n",
        )
        .expect("write parent");
        let child = base.path().join("project");
        std::fs::create_dir_all(&child).expect("mkdir");
        // Child overrides indent_size but not max_line_length.
        std::fs::write(child.join(".editorconfig"), "[*]\nindent_size = 2\n").expect("write child");
        let config = format_config_for(&child.join("main.lua"));
        assert_eq!(config.indent_width, Some(2));
        assert_eq!(config.line_width, Some(100));
    }

    #[test]
    fn later_section_overrides_earlier() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*]\nindent_size = 4\n[*.lua]\nindent_size = 2\n",
        )
        .expect("write");
        let config = format_config_for(&dir.path().join("main.lua"));
        assert_eq!(config.indent_width, Some(2));
    }

    #[test]
    fn max_line_length_off_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*]\nmax_line_length = off\n",
        )
        .expect("write");
        let config = format_config_for(&dir.path().join("main.lua"));
        assert!(config.line_width.is_none());
    }

    #[test]
    fn no_editorconfig_yields_all_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = format_config_for(&dir.path().join("main.lua"));
        assert!(config.indent_style.is_none());
        assert!(config.indent_width.is_none());
        assert!(config.line_endings.is_none());
        assert!(config.line_width.is_none());
    }

    #[test]
    fn luck_json_wins_over_editorconfig() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*.lua]\nindent_size = 2\n",
        )
        .expect("write");
        let file_path = dir.path().join("main.lua");
        let project = FormatConfig {
            indent_width: Some(4),
            ..FormatConfig::default()
        };
        let resolved = resolved_format_config(Some(&project), &file_path, true);
        assert_eq!(resolved.indent_width, Some(4));
    }

    #[test]
    fn disabled_editorconfig_only_uses_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*.lua]\nindent_size = 2\n",
        )
        .expect("write");
        let file_path = dir.path().join("main.lua");
        let resolved = resolved_format_config(None, &file_path, false);
        assert!(resolved.indent_width.is_none());
    }

    #[test]
    fn tab_width_used_when_indent_size_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".editorconfig"), "[*]\ntab_width = 6\n").expect("write");
        let config = format_config_for(&dir.path().join("main.lua"));
        assert_eq!(config.indent_width, Some(6));
    }

    #[test]
    fn comments_and_blanks_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "# comment\n; also comment\n\n[*.lua]\n\nindent_size = 5 # inline kept as value? no\n",
        )
        .expect("write");
        // The inline `# ...` is part of the value; `5 # ...` won't parse as u8,
        // so indent_width stays None. This documents the simple parser behavior.
        let config = format_config_for(&dir.path().join("main.lua"));
        assert!(config.indent_width.is_none());
    }
}
