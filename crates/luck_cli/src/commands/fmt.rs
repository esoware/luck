//! `luck fmt` - config-driven, oxfmt-style formatting with `--check`,
//! `--list-different`, range formatting, and stdin mode.

use crate::output::parallel_chunk_size;
use crate::project::{collect_target_files, project_filter, resolve_project_config};
use crate::render::{FileCache, render_diagnostics_to_buffer};
use crate::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE, Verbosity};
use clap::Args;
use luck_core::config::LuckConfig;
use luck_core::diagnostics::Diagnostic;
use luck_core::types::LuaTarget;
use std::path::{Path, PathBuf};
use std::process;
use std::process::ExitCode;

#[derive(Args)]
pub(crate) struct FmtArgs {
    /// Files or directories to format [default: current directory]
    paths: Vec<String>,

    /// Format and write files in place (the default mode)
    #[arg(long)]
    write: bool,

    /// Check if files are formatted; exit 1 and list files that would change
    #[arg(long, conflicts_with = "write")]
    check: bool,

    /// List files that would change without writing them
    #[arg(long, conflicts_with_all = ["write", "check"])]
    list_different: bool,

    /// Path to config file [default: discover luck.json upward from cwd]
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Ignore any `.editorconfig` files when resolving formatting defaults
    #[arg(long)]
    no_editorconfig: bool,

    /// Read source from stdin, using PATH only to pick the target and
    /// label diagnostics; the formatted result is written to stdout
    #[arg(long, value_name = "PATH")]
    stdin_filepath: Option<String>,

    /// Format only statements overlapping the byte range starting at this
    /// offset; requires a single input file or --stdin-filepath [default: 0]
    #[arg(long, value_name = "BYTE")]
    range_start: Option<usize>,

    /// End of the format range as an exclusive byte offset, clamped to the
    /// file length; requires a single input file or --stdin-filepath
    /// [default: end of file]
    #[arg(long, value_name = "BYTE")]
    range_end: Option<usize>,

    /// Re-parse the formatted output and verify it is structurally
    /// equivalent to the input, failing the run if formatting changed the
    /// program's meaning; a safety net at the cost of an extra parse.
    /// Incompatible with range formatting
    #[arg(long, conflicts_with_all = ["range_start", "range_end"])]
    verify: bool,
}

impl FmtArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> ExitCode {
        let (luck_config, config_dir) = resolve_project_config(self.config.as_deref());

        // Resolve the parse target lazily per file from the config dialects.
        let target_for = |path: &Path| -> LuaTarget {
            luck_config.target_for_path(path).unwrap_or_else(|message| {
                eprintln!("Error: {message}");
                process::exit(EXIT_USAGE as i32);
            })
        };

        // Stdin mode formats a single virtual document and writes the result to
        // stdout, ignoring positional paths (the editor "format stdin" contract).
        if let Some(stdin_path) = self.stdin_filepath.as_deref() {
            return self.run_stdin(stdin_path, &luck_config, target_for);
        }

        // Range formatting is per-document: byte offsets are meaningless across
        // a directory walk or multiple files.
        let has_range = self.range_start.is_some() || self.range_end.is_some();
        if has_range && (self.paths.len() != 1 || !Path::new(&self.paths[0]).is_file()) {
            eprintln!(
                "Error: --range-start/--range-end require exactly one input file or --stdin-filepath"
            );
            return ExitCode::from(EXIT_USAGE);
        }

        let filter = project_filter(&config_dir, &luck_config);
        let files = collect_target_files(&self.paths, &filter);

        // --check and --list-different only report; --write (or no mode flag at
        // all) writes in place. --write is the implicit default, so its presence
        // does not change behavior beyond documenting intent.
        let _ = self.write;
        let report_only = self.check || self.list_different;

        let opts = FormatOpts {
            luck_config: &luck_config,
            use_editorconfig: !self.no_editorconfig,
            verify: self.verify,
        };

        // Files format in parallel; per-file output is buffered and flushed
        // in input order so results match the sequential loop byte-for-byte.
        use rayon::prelude::*;
        let mut changed: Vec<String> = Vec::new();
        let mut had_parse_error = false;
        {
            // One lock and one flush per stream for the whole run; the early
            // returns flush through BufWriter's drop. Bounded sorted groups
            // keep peak outcome memory flat on huge projects.
            use std::io::Write;
            let mut stderr = std::io::BufWriter::new(std::io::stderr().lock());
            let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
            for chunk in files.chunks(parallel_chunk_size()) {
                let outcomes: Vec<FileOutcome> = chunk
                    .par_iter()
                    .map(|file_path| {
                        format_file(
                            file_path,
                            target_for(file_path),
                            self.range_start,
                            self.range_end,
                            report_only,
                            opts,
                        )
                    })
                    .collect();

                for outcome in outcomes {
                    match outcome {
                        FileOutcome::ReadError(message) => {
                            let _ = writeln!(stderr, "{message}");
                            // A skipped file must fail the run, same as a parse error.
                            had_parse_error = true;
                        }
                        FileOutcome::RangeError(message) => {
                            let _ = writeln!(stderr, "{message}");
                            return ExitCode::from(EXIT_USAGE);
                        }
                        FileOutcome::ParseError(rendered) => {
                            let _ = stderr.write_all(&rendered);
                            had_parse_error = true;
                        }
                        FileOutcome::VerifyError(message) => {
                            let _ = writeln!(stderr, "{message}");
                            // A verification failure fails the run like a parse error.
                            had_parse_error = true;
                        }
                        FileOutcome::Unchanged => {}
                        FileOutcome::Changed(path_str) => changed.push(path_str),
                        FileOutcome::WriteError(message) => {
                            let _ = writeln!(stderr, "{message}");
                            return ExitCode::from(EXIT_FAILURE);
                        }
                        FileOutcome::Written(path_str) => {
                            if verbosity != Verbosity::Quiet {
                                let _ = writeln!(stdout, "{path_str}");
                            }
                        }
                    }
                }
            }
        }

        if report_only {
            for path_str in &changed {
                println!("{path_str}");
            }
            // --check exits non-zero when any file would change; --list-different
            // is purely informational and always exits 0.
            if self.check && !changed.is_empty() {
                return ExitCode::from(EXIT_FAILURE);
            }
        }

        // An unformattable file is a failure in every mode, aligning with
        // `--check`'s non-zero convention.
        if had_parse_error {
            return ExitCode::from(EXIT_FAILURE);
        }

        ExitCode::from(EXIT_SUCCESS)
    }

    /// Format a single document read from stdin, labeled and targeted by the
    /// `--stdin-filepath` value. The standard mode writes the formatted result
    /// to stdout; `--check`/`--list-different` only report (and `--check` sets
    /// a non-zero exit when the document would change).
    fn run_stdin(
        &self,
        stdin_path: &str,
        luck_config: &LuckConfig,
        target_for: impl Fn(&Path) -> LuaTarget,
    ) -> ExitCode {
        let source = match crate::output::read_stdin_source() {
            Ok(text) => text,
            Err(code) => return code,
        };

        let range = match resolve_format_range(self.range_start, self.range_end, source.len()) {
            Ok(range) => range,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(EXIT_USAGE);
            }
        };

        let file_path = PathBuf::from(stdin_path);
        let target = target_for(&file_path);
        let report_only = self.check || self.list_different;
        let opts = FormatOpts {
            luck_config,
            use_editorconfig: !self.no_editorconfig,
            verify: self.verify,
        };

        match format_document(&source, stdin_path, &file_path, target, range, opts) {
            FormatOutcome::ParseError(rendered) => {
                use std::io::Write;
                let _ = std::io::stderr().write_all(&rendered);
                ExitCode::from(EXIT_FAILURE)
            }
            FormatOutcome::VerifyError(message) => {
                eprintln!("{message}");
                ExitCode::from(EXIT_FAILURE)
            }
            FormatOutcome::Unchanged => {
                // Editors expect the (already-formatted) source echoed back;
                // report modes stay silent on an unchanged document.
                if !report_only {
                    print!("{source}");
                }
                ExitCode::from(EXIT_SUCCESS)
            }
            FormatOutcome::Changed(output) => {
                if self.list_different {
                    println!("{stdin_path}");
                    ExitCode::from(EXIT_SUCCESS)
                } else if self.check {
                    // --check prints nothing to stdout and signals the
                    // difference through the exit code alone.
                    ExitCode::from(EXIT_FAILURE)
                } else {
                    print!("{output}");
                    ExitCode::from(EXIT_SUCCESS)
                }
            }
        }
    }
}

/// Outcome of formatting one file on the parallel walk, including the IO steps.
enum FileOutcome {
    ReadError(String),
    RangeError(String),
    ParseError(Vec<u8>),
    VerifyError(String),
    Unchanged,
    Changed(String),
    WriteError(String),
    Written(String),
}

/// Run-wide formatting knobs shared by the file-walk and stdin paths, threaded
/// as one value to keep the format helpers under the argument-count budget.
#[derive(Clone, Copy)]
struct FormatOpts<'a> {
    luck_config: &'a LuckConfig,
    use_editorconfig: bool,
    verify: bool,
}

/// Read, format, and (unless `report_only`) write one file.
fn format_file(
    file_path: &Path,
    target: LuaTarget,
    range_start: Option<usize>,
    range_end: Option<usize>,
    report_only: bool,
    opts: FormatOpts,
) -> FileOutcome {
    let source = match luck_core::source_io::read_source_file(file_path) {
        Ok(text) => text,
        Err(error) => {
            return FileOutcome::ReadError(format!(
                "Error: cannot read {}: {error}",
                file_path.display()
            ));
        }
    };

    let range = match resolve_format_range(range_start, range_end, source.len()) {
        Ok(range) => range,
        Err(message) => return FileOutcome::RangeError(message),
    };

    let file_label = file_path.to_string_lossy().to_string();
    let formatted = match format_document(&source, &file_label, file_path, target, range, opts) {
        FormatOutcome::ParseError(rendered) => return FileOutcome::ParseError(rendered),
        FormatOutcome::VerifyError(message) => return FileOutcome::VerifyError(message),
        FormatOutcome::Unchanged => return FileOutcome::Unchanged,
        FormatOutcome::Changed(output) => output,
    };

    let path_str = file_path.display().to_string();
    if report_only {
        FileOutcome::Changed(path_str)
    } else if let Err(error) = std::fs::write(file_path, &formatted) {
        FileOutcome::WriteError(format!("Error: cannot write {path_str}: {error}"))
    } else {
        FileOutcome::Written(path_str)
    }
}

/// Outcome of formatting one in-memory document, shared by the file-walk and
/// stdin paths.
enum FormatOutcome {
    /// The source parsed and the formatted result is identical to the input.
    Unchanged,
    /// The source parsed and formatting produced different output.
    Changed(String),
    /// The source did not parse; carries the rendered diagnostics (buffered so
    /// parallel workers never interleave stderr).
    ParseError(Vec<u8>),
    /// `--verify` re-parsed the formatted output and found it structurally
    /// divergent from the input; carries a ready-to-print error message.
    VerifyError(String),
}

/// Format one in-memory document, rendering parse-error diagnostics under
/// `path_label` when the source does not parse. Does no IO of its own; callers
/// decide whether to write the result to disk or stdout.
fn format_document(
    source: &str,
    path_label: &str,
    file_path: &Path,
    target: LuaTarget,
    range: Option<std::ops::Range<usize>>,
    opts: FormatOpts,
) -> FormatOutcome {
    // `.editorconfig` provides formatting defaults below luck.json; the
    // luck.json `format` section always wins.
    let resolved_format = luck_core::editorconfig::resolved_format_config(
        opts.luck_config.format.as_ref(),
        file_path,
        opts.use_editorconfig,
    );
    let options = luck_formatter::FormatOptions::from(&resolved_format);
    let version = target.lua_version();
    // `--verify` conflicts with range formatting at the arg layer, so a
    // verified run always formats the whole document through the safety net.
    let result = if opts.verify {
        match luck_formatter::format_and_verify(source, version, &options) {
            Ok(result) => result,
            Err((_result, diff)) => {
                return FormatOutcome::VerifyError(format!(
                    "Error: {path_label}: formatting verification failed at {}: {}",
                    diff.path, diff.reason
                ));
            }
        }
    } else {
        match range {
            Some(range) => luck_formatter::format_range(source, version, &options, range),
            None => luck_formatter::format(source, version, &options),
        }
    };
    if !result.errors.is_empty() {
        let diagnostics: Vec<Diagnostic> = result
            .errors
            .iter()
            .map(|err| {
                luck_core::diagnostics::errors::e008(path_label, err.span.into(), &err.message)
            })
            .collect();
        let mut cache = FileCache::new();
        cache.add_file(path_label.to_string(), source.to_string());
        let rendered = render_diagnostics_to_buffer(&diagnostics, &mut cache);
        return FormatOutcome::ParseError(rendered);
    }

    if result.output == source {
        FormatOutcome::Unchanged
    } else {
        FormatOutcome::Changed(result.output)
    }
}

/// Resolve `--range-start`/`--range-end` into a byte range over a source of
/// `source_len` bytes: a missing start is 0, a missing end is the end of the
/// source, and an explicit end is clamped to the source length. Errors when
/// the resolved start exceeds the resolved end.
fn resolve_format_range(
    range_start: Option<usize>,
    range_end: Option<usize>,
    source_len: usize,
) -> Result<Option<std::ops::Range<usize>>, String> {
    if range_start.is_none() && range_end.is_none() {
        return Ok(None);
    }
    let start = range_start.unwrap_or(0);
    let end = range_end.unwrap_or(source_len).min(source_len);
    if start > end {
        return Err(format!(
            "Error: invalid format range: start ({start}) is greater than end ({end})"
        ));
    }
    Ok(Some(start..end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Cli, Command};
    use clap::Parser;

    #[test]
    fn fmt_is_top_level_command_with_oxfmt_flags() {
        let cli = Cli::try_parse_from([
            "luck",
            "fmt",
            "src",
            "main.lua",
            "--check",
            "-c",
            "luck.json",
        ])
        .expect("fmt parses");
        match cli.command {
            Command::Fmt(FmtArgs {
                paths,
                check,
                list_different,
                write,
                config,
                ..
            }) => {
                assert_eq!(paths, vec!["src".to_string(), "main.lua".to_string()]);
                assert!(check);
                assert!(!list_different);
                assert!(!write);
                assert_eq!(config, Some(PathBuf::from("luck.json")));
            }
            _ => panic!("expected Command::Fmt"),
        }
    }

    #[test]
    fn fmt_list_different_and_write_parse() {
        let listed = Cli::try_parse_from(["luck", "fmt", "--list-different"])
            .expect("fmt --list-different parses");
        assert!(matches!(
            listed.command,
            Command::Fmt(FmtArgs {
                list_different: true,
                ..
            })
        ));
        let written = Cli::try_parse_from(["luck", "fmt", "--write"]).expect("fmt --write parses");
        assert!(matches!(
            written.command,
            Command::Fmt(FmtArgs { write: true, .. })
        ));
        // --check and --write are mutually exclusive.
        assert!(Cli::try_parse_from(["luck", "fmt", "--check", "--write"]).is_err());
    }

    #[test]
    fn fmt_verify_flag_parses() {
        let fmt = Cli::try_parse_from(["luck", "fmt", "--verify"]).expect("fmt --verify parses");
        assert!(matches!(
            fmt.command,
            Command::Fmt(FmtArgs { verify: true, .. })
        ));
        // --verify formats whole documents, so it is mutually exclusive with
        // range formatting.
        assert!(Cli::try_parse_from(["luck", "fmt", "--verify", "--range-start", "0"]).is_err());
        assert!(Cli::try_parse_from(["luck", "fmt", "--verify", "--range-end", "4"]).is_err());
    }

    #[test]
    fn fmt_verify_appears_in_help() {
        use clap::CommandFactory;
        let mut cli = Cli::command();
        let fmt = cli
            .find_subcommand_mut("fmt")
            .expect("fmt subcommand exists");
        let help = fmt.render_long_help().to_string();
        assert!(help.contains("--verify"), "help missing --verify:\n{help}");
    }

    #[test]
    fn format_document_verify_passes_on_valid_source() {
        // The formatter is structure-preserving, so verified formatting of any
        // parseable input succeeds and reports the ordinary Changed/Unchanged
        // outcomes rather than a VerifyError.
        let luck_config = LuckConfig::default();
        let path = PathBuf::from("stdin.lua");
        let opts = FormatOpts {
            luck_config: &luck_config,
            use_editorconfig: false,
            verify: true,
        };
        let outcome = format_document(
            "local  x=1\n",
            "stdin.lua",
            &path,
            LuaTarget::Lua54,
            None,
            opts,
        );
        assert!(
            matches!(outcome, FormatOutcome::Changed(_)),
            "verified formatting of messy-but-valid source should reformat"
        );
    }

    #[test]
    fn fmt_stdin_filepath_parses() {
        let fmt = Cli::try_parse_from(["luck", "fmt", "--stdin-filepath", "buf.lua"])
            .expect("fmt --stdin-filepath parses");
        assert!(matches!(
            fmt.command,
            Command::Fmt(FmtArgs { stdin_filepath: Some(ref path), .. }) if path == "buf.lua"
        ));
    }

    #[test]
    fn fmt_format_options_read_from_luck_json() {
        // A config requesting space indentation must produce spaces-based
        // FormatOptions, with no per-option CLI flags involved.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(
            &config_path,
            r#"{"lua":"lua54","format":{"indent_style":"spaces","indent_width":2}}"#,
        )
        .expect("write luck.json");

        let config = luck_core::config::load_with_extends(&config_path).expect("config loads");
        let options = luck_formatter::FormatOptions::from(&config.format.unwrap_or_default());
        assert!(matches!(
            options.indent_style,
            luck_formatter::IndentStyle::Spaces
        ));
        assert_eq!(options.indent_width, 2);
    }

    #[test]
    fn fmt_format_options_default_without_format_section() {
        let config = luck_core::config::FormatConfig::default();
        let options = luck_formatter::FormatOptions::from(&config);
        let defaults = luck_formatter::FormatOptions::default();
        assert_eq!(options.indent_width, defaults.indent_width);
        assert_eq!(options.line_width, defaults.line_width);
    }

    /// Build a `FmtArgs` for the given single path with everything defaulted.
    fn fmt_args(path: PathBuf, config: PathBuf) -> FmtArgs {
        FmtArgs {
            paths: vec![path.to_string_lossy().into_owned()],
            write: true,
            check: false,
            list_different: false,
            config: Some(config),
            no_editorconfig: false,
            stdin_filepath: None,
            range_start: None,
            range_end: None,
            verify: false,
        }
    }

    #[test]
    fn fmt_uses_editorconfig_when_luck_json_has_no_format() {
        // A project whose luck.json has no `format` section falls back to the
        // `.editorconfig` defaults: two-space indentation here.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(&config_path, r#"{"lua":"lua54"}"#).expect("write luck.json");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root = true\n[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write .editorconfig");
        let source_path = dir.path().join("main.lua");
        std::fs::write(&source_path, "local function f()\nreturn 1\nend\n").expect("write source");

        fmt_args(source_path.clone(), config_path).run(Verbosity::Quiet);

        let formatted = std::fs::read_to_string(&source_path).expect("read back");
        assert!(
            formatted.contains("\n  return 1"),
            "expected two-space indent from .editorconfig, got:\n{formatted}"
        );
    }

    #[test]
    fn fmt_luck_json_format_wins_over_editorconfig() {
        // With both present, the luck.json `format` section overrides the
        // `.editorconfig` value: four-space indentation wins over two.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(
            &config_path,
            r#"{"lua":"lua54","format":{"indent_style":"spaces","indent_width":4}}"#,
        )
        .expect("write luck.json");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root = true\n[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write .editorconfig");
        let source_path = dir.path().join("main.lua");
        std::fs::write(&source_path, "local function f()\nreturn 1\nend\n").expect("write source");

        fmt_args(source_path.clone(), config_path).run(Verbosity::Quiet);

        let formatted = std::fs::read_to_string(&source_path).expect("read back");
        assert!(
            formatted.contains("\n    return 1"),
            "expected four-space indent from luck.json, got:\n{formatted}"
        );
    }

    #[test]
    fn format_document_reports_changed_unchanged_and_parse_error() {
        // The shared per-document formatter helper backs both the file-walk and
        // the --stdin-filepath paths, so cover its three outcomes directly.
        let luck_config = LuckConfig::default();
        let path = PathBuf::from("stdin.lua");
        let opts = FormatOpts {
            luck_config: &luck_config,
            use_editorconfig: false,
            verify: false,
        };

        let unformatted = format_document(
            "local  x=1\n",
            "stdin.lua",
            &path,
            LuaTarget::Lua54,
            None,
            opts,
        );
        let formatted = match unformatted {
            FormatOutcome::Changed(output) => output,
            _ => panic!("messy source should reformat to Changed"),
        };

        // Re-running over already-formatted text yields Unchanged (idempotency).
        assert!(matches!(
            format_document(&formatted, "stdin.lua", &path, LuaTarget::Lua54, None, opts),
            FormatOutcome::Unchanged
        ));

        assert!(matches!(
            format_document(
                "local x =",
                "stdin.lua",
                &path,
                LuaTarget::Lua54,
                None,
                opts
            ),
            FormatOutcome::ParseError(_)
        ));
    }
}
