use core::ops::Range;
use std::fs;
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use clap::Args;
use luck_core::{LuaTarget, LuckConfig, read_source_file, resolved_format_config};

use crate::exit::{Exit, usage_error};
use crate::output::read_stdin_source;
use crate::project::{config_for_one_shot, outcomes_in_order, project_files};
use crate::render::render_source_errors;
use crate::verbosity::Verbosity;

#[derive(Args)]
#[expect(clippy::struct_excessive_bools, reason = "clap derive needs one bool field per flag")]
pub(crate) struct FmtArgs {
    /// Files, directories, or globs to format, and `!` globs to leave out
    /// [default: current directory].
    paths: Vec<String>,

    /// Format and write files in place (the default mode).
    #[arg(long)]
    write: bool,

    /// Check if files are formatted; exit 1 and list files that would change.
    #[arg(long, conflicts_with = "write")]
    check: bool,

    /// List files that would change without writing them.
    #[arg(long, conflicts_with_all = ["write", "check"])]
    list_different: bool,

    /// Use this config for every file [default: the nearest luck.json above
    /// each file].
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Exit 0 when a glob, or the whole run, matches no file.
    #[arg(long)]
    no_error_on_unmatched_pattern: bool,

    /// Ignore any `.editorconfig` files when resolving formatting defaults.
    #[arg(long)]
    no_editorconfig: bool,

    /// Read source from stdin, using PATH to pick the target and
    /// `.editorconfig` settings and to label diagnostics; the result is
    /// written to stdout.
    #[arg(long, value_name = "PATH")]
    stdin_filepath: Option<String>,

    /// Format only statements overlapping the byte range starting at this
    /// offset; requires a single input file or --stdin-filepath [default: 0].
    #[arg(long, value_name = "BYTE")]
    range_start: Option<usize>,

    /// End of the format range as an exclusive byte offset, clamped to the
    /// file length; requires a single input file or --stdin-filepath
    /// [default: end of file].
    #[arg(long, value_name = "BYTE")]
    range_end: Option<usize>,

    /// Re-parse the output and fail if it isn't structurally equivalent to
    /// the input. Costs an extra parse; incompatible with range formatting.
    #[arg(long, conflicts_with_all = ["range_start", "range_end"])]
    verify: bool,
}

enum FormatOutcome {
    Unchanged,
    Changed(String),
    /// The source didn't parse, or `--verify` found the output structurally
    /// different from the input. Holds the errors ready for stderr, buffered
    /// so parallel workers never interleave them.
    Failed(Vec<u8>),
}

enum FileOutcome {
    /// The file couldn't be read, parsed, or verified. Holds the errors ready
    /// for stderr.
    Failed(Vec<u8>),
    RangeError(String),
    Unchanged,
    /// The file would change, under `--check` or `--list-different`.
    Changed(String),
    WriteError(String),
    Written(String),
}

impl FmtArgs {
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config, path, or range, or when no
    /// file matches, and [`Exit::Failure`] when a file can't be formatted or
    /// `--check` finds one that would change.
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        // Stdin mode ignores positional paths.
        if let Some(stdin_path) = self.stdin_filepath.as_deref() {
            let config = config_for_one_shot(self.config.as_deref(), stdin_path)?;
            return self.run_stdin(stdin_path, &config);
        }

        // Range formatting is per-document: byte offsets are meaningless across
        // a directory walk or multiple files.
        let has_range = self.range_start.is_some() || self.range_end.is_some();
        if has_range && !matches!(self.paths.as_slice(), [path] if Path::new(path).is_file()) {
            return Err(usage_error(
                "`--range-start`/`--range-end` require exactly one input file or \
                 `--stdin-filepath`",
            ));
        }

        let files = project_files(
            self.config.as_deref(),
            &self.paths,
            self.no_error_on_unmatched_pattern,
            |_| Ok(()),
        )?;

        // Writing in place is the default, so --write needs no handling here.
        let _: bool = self.write;

        let mut changed_paths: Vec<String> = Vec::new();
        let mut has_failed_file = false;
        {
            // One lock and one flush per stream for the whole run (the early
            // returns flush on drop).
            let mut stderr = BufWriter::new(io::stderr().lock());
            let mut stdout = BufWriter::new(io::stdout().lock());
            let outcomes = outcomes_in_order(&files, |file| {
                self.format_file(&file.path, file.target, &file.project.config)
            });
            for outcome in outcomes {
                match outcome {
                    FileOutcome::Failed(rendered) => {
                        let _unused = stderr.write_all(&rendered);
                        has_failed_file = true;
                    }
                    FileOutcome::RangeError(message) => {
                        let _unused = writeln!(stderr, "{message}");
                        return Err(Exit::Usage);
                    }
                    FileOutcome::Unchanged => {}
                    FileOutcome::Changed(path) => changed_paths.push(path),
                    FileOutcome::WriteError(message) => {
                        let _unused = writeln!(stderr, "{message}");
                        return Err(Exit::Failure);
                    }
                    FileOutcome::Written(path) => {
                        if verbosity != Verbosity::Quiet {
                            let _unused = writeln!(stdout, "{path}");
                        }
                    }
                }
            }
        }

        if self.is_report_only() {
            for path in &changed_paths {
                println!("{path}");
            }
            // Only --check fails on files that would change; --list-different
            // just lists them.
            if self.check && !changed_paths.is_empty() {
                return Err(Exit::Failure);
            }
        }

        // A file that couldn't be read, parsed, or verified fails the run in
        // every mode.
        if has_failed_file {
            return Err(Exit::Failure);
        }

        Ok(())
    }

    /// The default mode writes the result to stdout. `--check` and
    /// `--list-different` only report, and `--check` fails when the document
    /// would change.
    ///
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad range or dialect, and
    /// [`Exit::Failure`] when stdin can't be read or formatted or `--check`
    /// finds a difference.
    fn run_stdin(&self, stdin_path: &str, luck_config: &LuckConfig) -> Result<(), Exit> {
        let source = read_stdin_source()?;

        let range = self.format_range(source.len()).map_err(|message| {
            eprintln!("{message}");
            Exit::Usage
        })?;

        let file_path = PathBuf::from(stdin_path);
        let target = luck_config.target_for_path(&file_path).map_err(usage_error)?;

        match self.format_document(luck_config, &source, &file_path, target, range) {
            FormatOutcome::Failed(rendered) => {
                let _unused = io::stderr().write_all(&rendered);
                Err(Exit::Failure)
            }
            FormatOutcome::Unchanged => {
                // Editors expect the (already-formatted) source echoed back;
                // report modes stay silent on an unchanged document.
                if !self.is_report_only() {
                    print!("{source}");
                }
                Ok(())
            }
            FormatOutcome::Changed(output) => {
                if self.list_different {
                    println!("{stdin_path}");
                    Ok(())
                } else if self.check {
                    // --check reports through the exit code alone.
                    Err(Exit::Failure)
                } else {
                    print!("{output}");
                    Ok(())
                }
            }
        }
    }

    const fn is_report_only(&self) -> bool {
        self.check || self.list_different
    }

    fn format_file(
        &self,
        file_path: &Path,
        target: LuaTarget,
        luck_config: &LuckConfig,
    ) -> FileOutcome {
        let source = match read_source_file(file_path) {
            Ok(text) => text,
            Err(error) => {
                return FileOutcome::Failed(
                    format!("error: cannot read `{}`: {error}\n", file_path.display()).into_bytes(),
                );
            }
        };

        let range = match self.format_range(source.len()) {
            Ok(range) => range,
            Err(message) => return FileOutcome::RangeError(message),
        };

        let formatted = match self.format_document(luck_config, &source, file_path, target, range) {
            FormatOutcome::Failed(rendered) => return FileOutcome::Failed(rendered),
            FormatOutcome::Unchanged => return FileOutcome::Unchanged,
            FormatOutcome::Changed(output) => output,
        };

        let path = file_path.display().to_string();
        if self.is_report_only() {
            FileOutcome::Changed(path)
        } else if let Err(error) = fs::write(file_path, &formatted) {
            FileOutcome::WriteError(format!("error: cannot write `{path}`: {error}"))
        } else {
            FileOutcome::Written(path)
        }
    }

    /// Writes nothing; callers decide whether the result goes to disk or stdout.
    fn format_document(
        &self,
        luck_config: &LuckConfig,
        source: &str,
        file_path: &Path,
        target: LuaTarget,
        range: Option<Range<usize>>,
    ) -> FormatOutcome {
        let editorconfig_path = (!self.no_editorconfig).then_some(file_path);
        let options = luck_formatter::FormatOptions::from(&resolved_format_config(
            luck_config.format,
            editorconfig_path,
        ));
        let version = target.lua_version();
        // Clap rejects `--verify` with a range, so a verified run formats the
        // whole document.
        let result = if self.verify {
            match luck_formatter::format_and_verify(source, version, &options) {
                Ok(result) => result,
                Err((_result, diff)) => {
                    return FormatOutcome::Failed(
                        format!(
                            "error: `{}`: formatting verification failed at `{}`: {}\n",
                            file_path.to_string_lossy(),
                            diff.path,
                            diff.reason
                        )
                        .into_bytes(),
                    );
                }
            }
        } else {
            range.map_or_else(
                || luck_formatter::format(source, version, &options),
                |byte_range| luck_formatter::format_range(source, version, &options, byte_range),
            )
        };
        if !result.errors.is_empty() {
            return FormatOutcome::Failed(render_source_errors(
                file_path.to_string_lossy().into_owned(),
                source,
                &result.errors,
                version,
            ));
        }

        if result.output == source {
            FormatOutcome::Unchanged
        } else {
            FormatOutcome::Changed(result.output)
        }
    }

    /// A missing start is 0, a missing end is the end of the source, and an
    /// explicit end is clamped to the source length.
    ///
    /// # Errors
    ///
    /// Returns a ready-to-print message when the resolved start exceeds the
    /// resolved end.
    fn format_range(&self, source_length: usize) -> Result<Option<Range<usize>>, String> {
        if self.range_start.is_none() && self.range_end.is_none() {
            return Ok(None);
        }
        let start = self.range_start.unwrap_or(0);
        let end = self.range_end.unwrap_or(source_length).min(source_length);
        if start > end {
            return Err(format!(
                "error: invalid format range: start ({start}) is greater than end ({end})"
            ));
        }
        Ok(Some(start..end))
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::{Cli, Command};

    fn fmt_args(flags: &[&str]) -> FmtArgs {
        let command_line = ["luck", "fmt"].iter().chain(flags);
        let Command::Fmt(args) = Cli::try_parse_from(command_line).expect("fmt parses").command
        else {
            panic!("expected Command::Fmt");
        };
        args
    }

    fn path_arg(path: &Path) -> &str {
        path.to_str().expect("temp paths are UTF-8")
    }

    #[test]
    fn fmt_is_top_level_command_with_oxfmt_flags() {
        let FmtArgs { paths, check, list_different, write, config, .. } =
            fmt_args(&["src", "main.lua", "--check", "-c", "luck.json"]);
        assert_eq!(paths, vec!["src".to_owned(), "main.lua".to_owned()]);
        assert!(check);
        assert!(!list_different);
        assert!(!write);
        assert_eq!(config, Some(PathBuf::from("luck.json")));
    }

    #[test]
    fn fmt_list_different_and_write_parse() {
        assert!(fmt_args(&["--list-different"]).list_different);
        assert!(fmt_args(&["--write"]).write);
        assert!(Cli::try_parse_from(["luck", "fmt", "--check", "--write"]).is_err());
    }

    #[test]
    fn fmt_verify_flag_parses() {
        assert!(fmt_args(&["--verify"]).verify);
        // --verify formats whole documents, so it can't combine with a range.
        assert!(Cli::try_parse_from(["luck", "fmt", "--verify", "--range-start", "0"]).is_err());
        assert!(Cli::try_parse_from(["luck", "fmt", "--verify", "--range-end", "4"]).is_err());
    }

    #[test]
    fn fmt_verify_appears_in_help() {
        use clap::CommandFactory as _;
        let mut cli = Cli::command();
        let fmt = cli.find_subcommand_mut("fmt").expect("fmt subcommand exists");
        let help = fmt.render_long_help().to_string();
        assert!(help.contains("--verify"), "help missing --verify:\n{help}");
    }

    #[test]
    fn format_document_verify_passes_on_valid_source() {
        // The formatter is structure-preserving, so verified formatting of any
        // parseable input succeeds and reports the ordinary Changed/Unchanged
        // outcomes rather than a verification failure.
        let outcome = fmt_args(&["--no-editorconfig", "--verify"]).format_document(
            &LuckConfig::default(),
            "local  x=1\n",
            Path::new("stdin.lua"),
            LuaTarget::Lua54,
            None,
        );
        assert!(
            matches!(outcome, FormatOutcome::Changed(_)),
            "verified formatting of messy-but-valid source should reformat"
        );
    }

    #[test]
    fn fmt_stdin_filepath_parses() {
        assert_eq!(
            fmt_args(&["--stdin-filepath", "buf.lua"]).stdin_filepath.as_deref(),
            Some("buf.lua")
        );
    }

    #[test]
    fn fmt_uses_editorconfig_when_luck_json_has_no_format() {
        // A project whose luck.json has no `format` section falls back to the
        // `.editorconfig` defaults: two-space indentation here.
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, r#"{"lua":"lua54"}"#).expect("write luck.json");
        fs::write(
            directory.path().join(".editorconfig"),
            "root = true\n[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write .editorconfig");
        let source_path = directory.path().join("main.lua");
        fs::write(&source_path, "local function f()\nreturn 1\nend\n").expect("write source");

        fmt_args(&[path_arg(&source_path), "-c", path_arg(&config_path)])
            .run(Verbosity::Quiet)
            .expect("fmt succeeds");

        let formatted = fs::read_to_string(&source_path).expect("read back");
        assert!(
            formatted.contains("\n  return 1"),
            "expected two-space indent from .editorconfig, got:\n{formatted}"
        );
    }

    #[test]
    fn fmt_luck_json_format_wins_over_editorconfig() {
        // With both present, the luck.json `format` section overrides the
        // `.editorconfig` value: four-space indentation wins over two.
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(
            &config_path,
            r#"{"lua":"lua54","format":{"indent_style":"spaces","indent_width":4}}"#,
        )
        .expect("write luck.json");
        fs::write(
            directory.path().join(".editorconfig"),
            "root = true\n[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write .editorconfig");
        let source_path = directory.path().join("main.lua");
        fs::write(&source_path, "local function f()\nreturn 1\nend\n").expect("write source");

        fmt_args(&[path_arg(&source_path), "-c", path_arg(&config_path)])
            .run(Verbosity::Quiet)
            .expect("fmt succeeds");

        let formatted = fs::read_to_string(&source_path).expect("read back");
        assert!(
            formatted.contains("\n    return 1"),
            "expected four-space indent from luck.json, got:\n{formatted}"
        );
    }

    #[test]
    fn format_document_without_editorconfig_uses_only_luck_json() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join(".editorconfig"),
            "root = true\n[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write .editorconfig");
        let source_path = directory.path().join("main.lua");
        let outcome = fmt_args(&["--no-editorconfig"]).format_document(
            &LuckConfig::default(),
            "local function f()\nreturn 1\nend\n",
            &source_path,
            LuaTarget::Lua54,
            None,
        );
        let FormatOutcome::Changed(formatted) = outcome else {
            panic!("unindented source should reformat to Changed");
        };
        assert!(
            formatted.contains("\n\treturn 1"),
            "expected the default tab indent, not .editorconfig's, got:\n{formatted}"
        );
    }

    #[test]
    fn format_document_reports_changed_unchanged_and_parse_error() {
        // The shared per-document formatter helper backs both the file-walk and
        // the --stdin-filepath paths, so cover its three outcomes directly.
        let args = fmt_args(&["--no-editorconfig"]);
        let luck_config = LuckConfig::default();
        let format = |source: &str| {
            args.format_document(
                &luck_config,
                source,
                Path::new("stdin.lua"),
                LuaTarget::Lua54,
                None,
            )
        };

        let FormatOutcome::Changed(formatted) = format("local  x=1\n") else {
            panic!("messy source should reformat to Changed");
        };

        // Re-running over already-formatted text yields Unchanged (idempotency).
        assert!(matches!(format(&formatted), FormatOutcome::Unchanged));

        assert!(matches!(format("local x ="), FormatOutcome::Failed(_)));
    }
}
