use core::fmt::Write as _;
use std::fs;
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, ArgMatches, Args, FromArgMatches, ValueEnum};
use luck_core::{Diagnostic, LuaTarget, LuckConfig, read_source_file};
use luck_linter::{Category, LintConfig, LintDiagnostic, RuleLevel, Severity};
use luck_token::LuaVersion;

use crate::exit::{Exit, operation_error, usage_error};
use crate::output::read_stdin_source;
use crate::project::{config_for_one_shot, outcomes_in_order, project_files, resolve_project};
use crate::render::{LineIndex, render_document_diags};
use crate::verbosity::Verbosity;

#[derive(Clone, ValueEnum)]
enum LintFormat {
    Default,
    /// One JSON object per finding, a line each.
    Json,
    /// GitHub Actions annotations.
    Github,
}

/// Each flag's id and long name, short name, level, and help.
const LEVEL_FLAGS: [(&str, char, RuleLevel, &str); 3] = [
    (
        "allow",
        'A',
        RuleLevel::Off,
        "Turn off a rule or a category (repeatable). -A, -W, and -D apply in the order given, \
         and a rule's own level wins over its category's",
    ),
    (
        "warn",
        'W',
        RuleLevel::Warn,
        "Report a rule, or every rule in a category, as a warning (repeatable)",
    ),
    (
        "deny",
        'D',
        RuleLevel::Error,
        "Report a rule, or every rule in a category, as an error (repeatable)",
    ),
];

/// The `-A`, `-W`, and `-D` flags in command-line order, which a derived
/// `Vec` per flag would lose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LevelFlags(Vec<(RuleLevel, String)>);

impl FromArgMatches for LevelFlags {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let mut flags: Vec<(usize, RuleLevel, String)> = Vec::new();
        for (id, _, level, _) in LEVEL_FLAGS {
            let indices = matches.indices_of(id).into_iter().flatten();
            let names = matches.get_many::<String>(id).into_iter().flatten();
            flags.extend(indices.zip(names).map(|(index, name)| (index, level, name.clone())));
        }
        flags.sort_by_key(|(index, ..)| *index);
        Ok(Self(flags.into_iter().map(|(_, level, name)| (level, name)).collect()))
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl Args for LevelFlags {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        LEVEL_FLAGS.into_iter().fold(cmd, |augmented, (id, short, _, help)| {
            augmented.arg(
                Arg::new(id)
                    .short(short)
                    .long(id)
                    .value_name("NAME")
                    .action(ArgAction::Append)
                    .help(help),
            )
        })
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

#[derive(Args)]
#[expect(clippy::struct_excessive_bools, reason = "clap derive needs one bool field per flag")]
pub(crate) struct LintArgs {
    /// Files, directories, or globs to lint, and `!` globs to leave out
    /// [default: current directory].
    paths: Vec<String>,

    /// Use this config for every file [default: the nearest luck.json above
    /// each file].
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(flatten)]
    levels: LevelFlags,

    /// Additional allowed global names (repeatable).
    #[arg(long = "global", value_name = "NAME")]
    globals: Vec<String>,

    /// Apply auto-fixes and write them back to source files.
    #[arg(long)]
    fix: bool,

    /// Output format.
    #[arg(short, long, value_enum, default_value_t = LintFormat::Default)]
    format: LintFormat,

    /// Exit with an error if there are more than this many warnings.
    #[arg(long, value_name = "INT")]
    max_warnings: Option<usize>,

    /// Treat warnings as errors for the exit code.
    #[arg(long)]
    deny_warnings: bool,

    /// Suppress all diagnostic output (exit code still reflects results).
    #[arg(long)]
    silent: bool,

    /// Exit 0 when no file matches.
    #[arg(long)]
    no_error_on_unmatched_pattern: bool,

    /// Print the registered rule names and categories, then exit.
    #[arg(long)]
    rules: bool,

    /// Print the effective lint configuration as JSON, then exit.
    #[arg(long)]
    print_config: bool,

    /// Read source from stdin, using PATH only to pick the config and target
    /// and to label diagnostics; --fix writes the fixed source to stdout and
    /// any findings to stderr.
    #[arg(long, value_name = "PATH")]
    stdin_filepath: Option<String>,
}

enum Rendered {
    /// ariadne output, for stderr.
    Pretty(Vec<u8>),
    /// A machine-readable format, one finding per line, for stdout.
    Lines(String),
}

/// Per-file lint result, buffered so parallel workers never interleave output.
struct LintOutcome {
    error_count: usize,
    warning_count: usize,
    is_fixed: bool,
    /// A file that couldn't be read or written back, for stderr.
    note: Option<String>,
    rendered: Option<Rendered>,
}

impl LintArgs {
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config, rule name, or path, or when
    /// no file matches, and [`Exit::Failure`] when a file can't be read or
    /// written back, or the findings fail the run under the exit-code policy.
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        if self.rules {
            print_lint_rules();
            return Ok(());
        }

        if self.print_config {
            let project = resolve_project(self.config.as_deref())?;
            print_lint_config(&self.lint_config(&project.config)?);
            return Ok(());
        }

        let is_quiet = verbosity == Verbosity::Quiet;

        // Stdin mode ignores positional paths.
        if let Some(stdin_path) = self.stdin_filepath.as_deref() {
            let config = config_for_one_shot(self.config.as_deref(), stdin_path)?;
            let lint_config = self.lint_config(&config)?;
            return self.run_stdin(stdin_path, &lint_config, &config, is_quiet);
        }

        let files = project_files(
            self.config.as_deref(),
            &self.paths,
            self.no_error_on_unmatched_pattern,
            |config| self.lint_config(config),
        )?;

        let mut total_errors = 0usize;
        let mut total_warnings = 0usize;
        let mut total_fixed = 0usize;
        {
            // One lock and one flush per stream for the whole run.
            let mut stderr = BufWriter::new(io::stderr().lock());
            let mut stdout = BufWriter::new(io::stdout().lock());
            let outcomes = outcomes_in_order(&files, |file| {
                self.lint_file(&file.path, file.target, &file.project.settings, is_quiet)
            });
            for outcome in outcomes {
                if let Some(note) = &outcome.note {
                    let _unused = writeln!(stderr, "{note}");
                }
                let _unused = match &outcome.rendered {
                    Some(Rendered::Pretty(pretty)) => stderr.write_all(pretty),
                    Some(Rendered::Lines(lines)) => stdout.write_all(lines.as_bytes()),
                    None => Ok(()),
                };
                total_errors = total_errors.saturating_add(outcome.error_count);
                total_warnings = total_warnings.saturating_add(outcome.warning_count);
                total_fixed = total_fixed.saturating_add(usize::from(outcome.is_fixed));
            }
        }

        if !self.silent && self.fix && total_fixed > 0 && !is_quiet {
            eprintln!("Fixed {total_fixed} file(s)");
        }

        if !self.silent && !is_quiet && (total_errors > 0 || total_warnings > 0) {
            eprintln!(
                "{total_errors} error(s), {total_warnings} warning(s) in {} file(s)",
                files.len()
            );
        }

        self.exit_status(total_errors, total_warnings, is_quiet)
    }

    /// `--fix` writes the fixed source to stdout, never to disk, and findings
    /// to stderr whatever their format, because stdout is the new file content.
    /// Otherwise output formats and the exit-code policy match the file walk.
    ///
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad dialect, and [`Exit::Failure`] when
    /// stdin can't be read or the findings fail the run under the exit-code
    /// policy.
    fn run_stdin(
        &self,
        stdin_path: &str,
        lint_config: &LintConfig,
        luck_config: &LuckConfig,
        is_quiet: bool,
    ) -> Result<(), Exit> {
        let source = read_stdin_source()?;

        let target = luck_config.target_for_path(Path::new(stdin_path)).map_err(usage_error)?;
        let mut diags = luck_linter::lint_target(&source, target, lint_config);
        let mut current_source = source;

        if self.fix {
            // Echo the source even with nothing to fix, since editors and
            // pipelines take stdout as the new file content.
            if diags.iter().any(|diag| diag.fix.is_some()) {
                current_source =
                    luck_linter::apply_fixes(&current_source, &diags, target.lua_version());
            }
            print!("{current_source}");
            diags = luck_linter::lint_target(&current_source, target, lint_config);
        }

        let (total_errors, total_warnings) = severity_counts(&diags);

        if let Some(rendered) =
            self.rendered(stdin_path, &current_source, diags, target.lua_version(), is_quiet)
        {
            match rendered {
                Rendered::Pretty(pretty) => {
                    let _unused = io::stderr().write_all(&pretty);
                }
                Rendered::Lines(lines) if self.fix => eprint!("{lines}"),
                Rendered::Lines(lines) => print!("{lines}"),
            }
        }

        self.exit_status(total_errors, total_warnings, is_quiet)
    }

    /// # Errors
    ///
    /// Returns [`Exit::Failure`] on any error, and on warnings under
    /// `--deny-warnings` or when they exceed `--max-warnings`.
    fn exit_status(
        &self,
        total_errors: usize,
        total_warnings: usize,
        is_quiet: bool,
    ) -> Result<(), Exit> {
        if total_errors > 0 {
            return Err(Exit::Failure);
        }
        let warning_limit = if self.deny_warnings { Some(0) } else { self.max_warnings };
        if let Some(limit) = warning_limit
            && total_warnings > limit
        {
            // `--quiet` printed none of the warnings that fail the run.
            return Err(if is_quiet && !self.silent {
                operation_error(format!("{total_warnings} warning(s) exceed the limit of {limit}"))
            } else {
                Exit::Failure
            });
        }
        Ok(())
    }

    fn lint_file(
        &self,
        file_path: &Path,
        target: LuaTarget,
        lint_config: &LintConfig,
        is_quiet: bool,
    ) -> LintOutcome {
        let source = match read_source_file(file_path) {
            Ok(text) => text,
            Err(error) => {
                // A skipped file fails the run, so CI can't pass with files
                // left unlinted.
                return LintOutcome {
                    error_count: 1,
                    warning_count: 0,
                    is_fixed: false,
                    note: Some(format!("error: cannot read `{}`: {error}", file_path.display())),
                    rendered: None,
                };
            }
        };

        let mut diags = luck_linter::lint_target(&source, target, lint_config);
        let mut current_source = source;
        let mut is_fixed = false;
        let mut note = None;

        if self.fix && diags.iter().any(|diag| diag.fix.is_some()) {
            let fixed_source =
                luck_linter::apply_fixes(&current_source, &diags, target.lua_version());
            if fixed_source != current_source {
                if let Err(error) = fs::write(file_path, &fixed_source) {
                    note = Some(format!("error: cannot write `{}`: {error}", file_path.display()));
                } else {
                    is_fixed = true;
                    diags = luck_linter::lint_target(&fixed_source, target, lint_config);
                    current_source = fixed_source;
                }
            }
        }

        let (finding_errors, warning_count) = severity_counts(&diags);
        let rendered = self.rendered(
            &file_path.to_string_lossy(),
            &current_source,
            diags,
            target.lua_version(),
            is_quiet,
        );
        // A fix that couldn't be written fails the run, as an unread file does.
        let error_count = finding_errors.saturating_add(usize::from(note.is_some()));
        LintOutcome { error_count, warning_count, is_fixed, note, rendered }
    }

    /// What the run prints for `diags`, if anything. `--quiet` drops the
    /// warnings from the output only: they still count toward the exit code.
    fn rendered(
        &self,
        file_label: &str,
        source: &str,
        mut diags: Vec<LintDiagnostic>,
        version: LuaVersion,
        is_quiet: bool,
    ) -> Option<Rendered> {
        if is_quiet {
            diags.retain(|diag| diag.severity == Severity::Error);
        }
        (!diags.is_empty() && !self.silent)
            .then(|| self.render(file_label, source, &diags, version))
    }

    fn render(
        &self,
        file_label: &str,
        source: &str,
        diags: &[LintDiagnostic],
        version: LuaVersion,
    ) -> Rendered {
        match self.format {
            LintFormat::Default => {
                let renderable_diags: Vec<Diagnostic> = diags
                    .iter()
                    .map(|diag| Diagnostic {
                        code: diag.rule,
                        message: diag.message.clone(),
                        severity: diag.severity,
                        file_path: file_label.to_owned(),
                        span: diag.span,
                        help: diag.help.clone(),
                    })
                    .collect();
                Rendered::Pretty(render_document_diags(
                    file_label.to_owned(),
                    source,
                    &renderable_diags,
                    version,
                ))
            }
            LintFormat::Json => {
                let line_index = LineIndex::new(source, version);
                let mut lines = String::new();
                for diag in diags {
                    let (line, column) = line_index.line_and_column(diag.span.start);
                    let (end_line, end_column) = line_index.line_and_column(diag.span.end);
                    let json_value = serde_json::json!({
                        "file": file_label,
                        "rule": diag.rule,
                        "severity": diag.severity.name(),
                        "message": diag.message,
                        "help": diag.help,
                        "start": diag.span.start,
                        "end": diag.span.end,
                        "line": line,
                        "column": column,
                        "end_line": end_line,
                        "end_column": end_column,
                    });
                    lines.push_str(&json_value.to_string());
                    lines.push('\n');
                }
                Rendered::Lines(lines)
            }
            LintFormat::Github => {
                let line_index = LineIndex::new(source, version);
                // The runner resolves a `/` path from the workspace root on
                // every platform.
                let slashed = file_label.replace('\\', "/");
                let file = annotation_property(slashed.strip_prefix("./").unwrap_or(&slashed));
                let mut lines = String::new();
                for diag in diags {
                    let (line, column) = line_index.line_and_column(diag.span.start);
                    let (end_line, end_column) = line_index.line_and_column(diag.span.end);
                    let message = diag.help.as_ref().map_or_else(
                        || diag.message.clone(),
                        |help| format!("{}\nhelp: {help}", diag.message),
                    );
                    let _unused = writeln!(
                        lines,
                        "::{} file={file},line={line},endLine={end_line},col={column},\
                         endColumn={end_column},title={}::{}",
                        diag.severity.name(),
                        annotation_property(diag.rule),
                        annotation_data(&message),
                    );
                }
                Rendered::Lines(lines)
            }
        }
    }

    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] on an unknown rule
    /// name.
    fn lint_config(&self, luck_config: &LuckConfig) -> Result<LintConfig, Exit> {
        let mut lint_config = luck_config.lint.clone().unwrap_or_default();

        // Flags win over the config and apply in command-line order, so the
        // last one to name a rule or a category decides it.
        for (level, name) in &self.levels.0 {
            match Category::from_name(name) {
                Some(category) => lint_config.categories.set_level(category, *level),
                None => lint_config.rules.set_level(name, *level),
            }
        }

        lint_config.extra_globals.extend(self.globals.iter().cloned());

        // Check rule names from both the config and the CLI, since an unknown one
        // would otherwise be ignored silently.
        let unknown_names = luck_linter::unknown_rule_names(&lint_config);
        if !unknown_names.is_empty() {
            let quoted_names: Vec<String> =
                unknown_names.iter().map(|name| format!("`{name}`")).collect();
            return Err(usage_error(format!(
                "unknown lint rule name(s): {}",
                quoted_names.join(", ")
            )));
        }

        Ok(lint_config)
    }
}

/// The message of a GitHub workflow command, escaped the way the runner
/// unescapes it.
fn annotation_data(text: &str) -> String {
    text.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

/// A property value of a GitHub workflow command, where `:` and `,` delimit
/// too.
fn annotation_property(text: &str) -> String {
    annotation_data(text).replace(':', "%3A").replace(',', "%2C")
}

/// Count `diags` by severity, as `(errors, warnings)`.
fn severity_counts(diags: &[LintDiagnostic]) -> (usize, usize) {
    let count = |severity: Severity| diags.iter().filter(|diag| diag.severity == severity).count();
    (count(Severity::Error), count(Severity::Warning))
}

fn print_lint_rules() {
    let mut rules: Vec<(&str, &str)> =
        luck_linter::rules().map(|rule| (rule.name(), rule.category().name())).collect();
    rules.sort_unstable();
    for (name, category) in rules {
        println!("{name}\t{category}");
    }
}

/// Prints `lint_config` as a `luck.json` `lint` section.
fn print_lint_config(lint_config: &LintConfig) {
    // The config holds only strings, numbers, and string-keyed maps, which
    // always serialize.
    if let Ok(json) = serde_json::to_string_pretty(lint_config) {
        println!("{json}");
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::{Cli, Command};

    fn lint_args(flags: &[&str]) -> LintArgs {
        let command_line = ["luck", "lint"].iter().chain(flags);
        let Command::Lint(args) = Cli::try_parse_from(command_line).expect("lint parses").command
        else {
            panic!("expected Command::Lint");
        };
        args
    }

    fn project_with_config(config: &str) -> (tempfile::TempDir, String) {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, config).expect("write luck.json");
        let config_arg = config_path.to_str().expect("temp paths are UTF-8").to_owned();
        (directory, config_arg)
    }

    fn resolved_lint_config(args: &LintArgs) -> LintConfig {
        let project = resolve_project(args.config.as_deref()).expect("config resolves");
        args.lint_config(&project.config).expect("lint config resolves")
    }

    fn rendered_lines(format: &str, file_label: &str, source: &str) -> String {
        let diags = luck_linter::lint_target(source, LuaTarget::Lua54, &LintConfig::default());
        let args = lint_args(&["--format", format]);
        match args.render(file_label, source, &diags, LuaVersion::Lua54) {
            Rendered::Lines(lines) => lines,
            Rendered::Pretty(_) => panic!("{format} is a line format"),
        }
    }

    #[test]
    fn lint_is_top_level_command_with_oxlint_flags() {
        let LintArgs { paths, levels, fix, config, .. } = lint_args(&[
            "src",
            "main.lua",
            "-D",
            "undefined_variable",
            "-A",
            "style",
            "-W",
            "shadowing",
            "--fix",
            "-c",
            "luck.json",
        ]);
        assert_eq!(paths, vec!["src".to_owned(), "main.lua".to_owned()]);
        assert_eq!(
            levels.0,
            [
                (RuleLevel::Error, "undefined_variable".to_owned()),
                (RuleLevel::Off, "style".to_owned()),
                (RuleLevel::Warn, "shadowing".to_owned()),
            ]
        );
        assert!(fix);
        assert_eq!(config, Some(PathBuf::from("luck.json")));
    }

    #[test]
    fn lint_long_flag_aliases_parse() {
        let LintArgs {
            levels,
            max_warnings,
            deny_warnings,
            silent,
            no_error_on_unmatched_pattern,
            rules,
            print_config,
            format,
            ..
        } = lint_args(&[
            "--deny",
            "x",
            "--allow",
            "y",
            "--warn",
            "z",
            "--config",
            "c.json",
            "--max-warnings",
            "3",
            "--deny-warnings",
            "--silent",
            "--no-error-on-unmatched-pattern",
            "--rules",
            "--print-config",
            "--format",
            "json",
        ]);
        assert_eq!(
            levels.0,
            [
                (RuleLevel::Error, "x".to_owned()),
                (RuleLevel::Off, "y".to_owned()),
                (RuleLevel::Warn, "z".to_owned()),
            ]
        );
        assert_eq!(max_warnings, Some(3));
        assert!(deny_warnings);
        assert!(silent);
        assert!(no_error_on_unmatched_pattern);
        assert!(rules);
        assert!(print_config);
        assert!(matches!(format, LintFormat::Json));
        assert!(matches!(lint_args(&["-f", "github"]).format, LintFormat::Github));
    }

    #[test]
    fn lint_stdin_filepath_parses() {
        assert_eq!(
            lint_args(&["--stdin-filepath", "buf.luau"]).stdin_filepath.as_deref(),
            Some("buf.luau")
        );
    }

    #[test]
    fn lint_config_honors_luck_json_rules() {
        let (_directory, config_arg) =
            project_with_config(r#"{"lua":"lua54","lint":{"rules":{"empty_block":"error"}}}"#);

        let lint_config = resolved_lint_config(&lint_args(&["-c", &config_arg]));
        assert_eq!(lint_config.rules.level("empty_block"), Some(RuleLevel::Error));
    }

    #[test]
    fn lint_cli_levels_win_over_config() {
        let (_directory, config_arg) = project_with_config(
            r#"{"lua":"lua54","lint":{"categories":{"style":"off"},"rules":{"empty_block":"error"}}}"#,
        );

        let lint_config = resolved_lint_config(&lint_args(&[
            "-c",
            &config_arg,
            "-A",
            "empty_block",
            "-W",
            "style",
        ]));
        assert_eq!(lint_config.rules.level("empty_block"), Some(RuleLevel::Off));
        assert_eq!(lint_config.categories.level(Category::Style), Some(RuleLevel::Warn));
    }

    #[test]
    fn lint_level_flags_set_a_category_to_their_own_level() {
        let (_directory, config_arg) = project_with_config(r#"{"lua":"lua54"}"#);

        for (flag, level) in
            [("-A", RuleLevel::Off), ("-W", RuleLevel::Warn), ("-D", RuleLevel::Error)]
        {
            let lint_config = resolved_lint_config(&lint_args(&["-c", &config_arg, flag, "perf"]));
            assert_eq!(lint_config.categories.level(Category::Performance), Some(level));
        }
    }

    #[test]
    fn lint_level_flags_apply_in_command_line_order() {
        let (_directory, config_arg) = project_with_config(r#"{"lua":"lua54"}"#);
        let level_after = |flags: &[&str]| {
            let command_line: Vec<&str> =
                ["-c", config_arg.as_str()].iter().chain(flags).copied().collect();
            resolved_lint_config(&lint_args(&command_line)).rules.level("empty_block")
        };

        assert_eq!(level_after(&["-D", "empty_block", "-A", "empty_block"]), Some(RuleLevel::Off));
        assert_eq!(
            level_after(&["-A", "empty_block", "--deny=empty_block"]),
            Some(RuleLevel::Error)
        );
        assert_eq!(
            level_after(&["-Wempty_block", "-D", "empty_block", "-W", "empty_block"]),
            Some(RuleLevel::Warn)
        );
    }

    #[test]
    fn lint_config_rejects_an_unknown_rule_name() {
        let (_directory, config_arg) = project_with_config(r#"{"lua":"lua54"}"#);
        let args = lint_args(&["-c", &config_arg, "-D", "not_a_real_rule"]);
        let project = resolve_project(args.config.as_deref()).expect("config resolves");
        assert_eq!(args.lint_config(&project.config).unwrap_err(), Exit::Usage);
    }

    #[test]
    fn quiet_hides_warnings_but_not_errors() {
        let source = "local unused = 1\nprint(undefined_var)";
        let diags = luck_linter::lint_target(source, LuaTarget::Lua54, &LintConfig::default());
        let args = lint_args(&["--format", "json"]);
        let lines_when = |is_quiet: bool| match args.rendered(
            "a.lua",
            source,
            diags.clone(),
            LuaVersion::Lua54,
            is_quiet,
        ) {
            Some(Rendered::Lines(lines)) => lines,
            Some(Rendered::Pretty(_)) | None => panic!("json renders lines"),
        };

        let all_lines = lines_when(false);
        assert_eq!(all_lines.lines().count(), 2, "{all_lines}");
        let quiet_lines = lines_when(true);
        assert_eq!(quiet_lines.lines().count(), 1, "{quiet_lines}");
        assert!(quiet_lines.contains(r#""severity":"error""#), "{quiet_lines}");
    }

    #[test]
    fn json_format_gives_the_line_and_column_of_a_finding() {
        let lines = rendered_lines("json", "src/a.lua", "local a = 1\nlocal unused = 2\nprint(a)");
        let finding: serde_json::Value = serde_json::from_str(lines.trim()).expect("one finding");
        assert_eq!(finding["rule"], "unused_variable");
        let number = |key: &str| finding[key].as_u64().expect("a number");
        assert_eq!((number("start"), number("end")), (18, 24));
        assert_eq!((number("line"), number("column")), (2, 7));
        assert_eq!((number("end_line"), number("end_column")), (2, 13));
    }

    #[test]
    fn github_format_writes_one_annotation_per_finding() {
        let lines =
            rendered_lines("github", r"src\a.lua", "local a = 1\nlocal unused = 2\nprint(a)");
        assert_eq!(
            lines,
            "::warning file=src/a.lua,line=2,endLine=2,col=7,endColumn=13,\
             title=unused_variable::unused variable `unused`%0Ahelp: prefix with `_` to suppress \
             this warning\n"
        );
    }

    #[test]
    fn annotation_escapes_what_the_runner_would_parse() {
        assert_eq!(annotation_data("50% of a\r\nb"), "50%25 of a%0D%0Ab");
        assert_eq!(annotation_property("C:/a,b.lua"), "C%3A/a%2Cb.lua");
    }

    #[test]
    fn lint_roblox_luau_does_not_flag_game_global() {
        // With `"luau":"roblox"`, a `.luau` file using `game` must lint clean,
        // because target selection is per-file from the config dialect.
        let (directory, config_arg) = project_with_config(r#"{"luau":"roblox"}"#);

        let args = lint_args(&["-c", &config_arg]);
        let project = resolve_project(args.config.as_deref()).expect("config resolves");
        let lint_config = args.lint_config(&project.config).expect("lint config resolves");

        let luau_file = directory.path().join("main.luau");
        let target = project.config.target_for_path(&luau_file).expect("luau target resolves");
        assert_eq!(target, LuaTarget::LuauRoblox);

        let diags = luck_linter::lint_target("print(game)", target, &lint_config);
        assert!(
            diags.iter().all(|diag| diag.rule != "undefined_variable"),
            "roblox global `game` should not be undefined"
        );

        // Standalone Luau (the default) does flag it; that contrast is the point.
        let standalone_diags =
            luck_linter::lint_target("print(game)", LuaTarget::Luau, &lint_config);
        assert!(standalone_diags.iter().any(|diag| diag.rule == "undefined_variable"));
    }
}
