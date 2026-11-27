use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, ArgMatches, Args, FromArgMatches, ValueEnum};
use luck_allocator::Allocator;
use luck_config::Project;
use luck_diagnostics::{Diagnostic, Severity};
use luck_linter::{
    Category, FixError, FixKind, Fixed, LintConfig, Linter, Message, RuleId, RuleLevel, RuleTimings,
};
use luck_parser::ParseResult;
use luck_resolver::{FileSystem as _, FileSystemOs, is_definition_file};
use luck_stdlib::StdlibLibrary;

use crate::exit::{Exit, operation_error, usage_error};
use crate::output::{read_stdin_source, write_output};
use crate::project::{ProjectFile, one_shot_project, project_files, working_project};
use crate::reporter::{DefaultReporter, GithubReporter, JsonReporter, Reporter, SourceFile};
use crate::thread_pool::for_each_outcome_in_order;
use crate::verbosity::Verbosity;

#[derive(Clone, Copy, ValueEnum)]
enum LintFormat {
    Default,
    /// One JSON object per finding, a line each.
    Json,
    /// GitHub Actions annotations.
    Github,
}

impl LintFormat {
    fn reporter(self) -> &'static dyn Reporter {
        match self {
            Self::Default => &DefaultReporter,
            Self::Json => &JsonReporter,
            Self::Github => &GithubReporter,
        }
    }

    /// People read the default format on stderr, and tools read the others
    /// on stdout.
    const fn is_for_tools(self) -> bool {
        match self {
            Self::Default => false,
            Self::Json | Self::Github => true,
        }
    }
}

/// A `-A`, `-W`, or `-D` flag, whose long name is also its id.
struct LevelFlag {
    id: &'static str,
    short: char,
    level: RuleLevel,
    help: &'static str,
}

const LEVEL_FLAGS: [LevelFlag; 3] = [
    LevelFlag {
        id: "allow",
        short: 'A',
        level: RuleLevel::Off,
        help: "Turn off a rule or a category (repeatable). -A, -W, and -D apply in the order \
               given, and a rule's own level wins over its category's",
    },
    LevelFlag {
        id: "warn",
        short: 'W',
        level: RuleLevel::Warn,
        help: "Report a rule, or every rule in a category, as a warning (repeatable)",
    },
    LevelFlag {
        id: "deny",
        short: 'D',
        level: RuleLevel::Error,
        help: "Report a rule, or every rule in a category, as an error (repeatable)",
    },
];

/// What a `-A`, `-W`, or `-D` flag names.
#[derive(Debug, Clone, Copy)]
enum Leveled {
    Category(Category),
    Rule(RuleId),
}

fn leveled(name: &str) -> Result<Leveled, String> {
    match Category::from_name(name) {
        Some(category) => Ok(Leveled::Category(category)),
        None => name.parse().map(Leveled::Rule),
    }
}

/// The levels the `-A`, `-W`, and `-D` flags set, applied in command-line
/// order, which a derived `Vec` per flag would lose, so the last flag to name
/// a rule or a category decides it.
struct LevelFlags(LintConfig);

impl FromArgMatches for LevelFlags {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let mut flags: Vec<(usize, RuleLevel, Leveled)> = Vec::new();
        for LevelFlag { id, level, .. } in LEVEL_FLAGS {
            let indices = matches.indices_of(id).into_iter().flatten();
            let names = matches.get_many::<Leveled>(id).into_iter().flatten();
            flags.extend(indices.zip(names).map(|(index, name)| (index, level, *name)));
        }
        flags.sort_by_key(|(index, ..)| *index);
        let mut levels = LintConfig::default();
        for (_, level, name) in flags {
            match name {
                Leveled::Category(category) => levels.categories.set_level(category, level),
                Leveled::Rule(rule) => levels.rules.set_level(rule, level),
            }
        }
        Ok(Self(levels))
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl Args for LevelFlags {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        LEVEL_FLAGS.into_iter().fold(cmd, |augmented, LevelFlag { id, short, help, .. }| {
            augmented.arg(
                Arg::new(id)
                    .short(short)
                    .long(id)
                    .value_name("NAME")
                    .value_parser(leveled)
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

    /// Apply fixes until none is left, and write them back to source files.
    #[arg(long)]
    fix: bool,

    /// Like --fix, but apply suggestions too, which may change what the code
    /// does, as renaming an unused variable to `_x` does.
    #[arg(long)]
    fix_suggestions: bool,

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

    /// Exit 0 when a glob, or the whole run, matches no file.
    #[arg(long)]
    no_error_on_unmatched_pattern: bool,

    /// Print the registered rule names, categories, and fix kinds, then exit.
    #[arg(long)]
    rules: bool,

    /// Print how long each rule took over the run, slowest first.
    #[arg(long)]
    timings: bool,

    /// Print the lint configuration of the one path or --stdin-filepath given,
    /// or else of the current directory, as JSON, then exit.
    #[arg(long)]
    print_config: bool,

    /// Read source from stdin, using PATH only to pick the config and to
    /// label diagnostics; --fix writes the fixed source to stdout and any
    /// findings to stderr.
    #[arg(long, value_name = "PATH")]
    stdin_filepath: Option<String>,
}

/// Per-file lint result, buffered so parallel workers never interleave output.
struct LintOutcome {
    error_count: usize,
    warning_count: usize,
    is_fixed: bool,
    /// A file that couldn't be read or written back, for stderr.
    note: Option<String>,
    /// The findings in the run's format.
    rendered: Vec<u8>,
}

impl LintArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        if self.rules {
            print_lint_rules();
            return Ok(());
        }

        let overlay = self.lint_overlay();
        if self.print_config {
            let config_path = self.config.as_deref();
            let path = match (self.stdin_filepath.as_deref(), self.paths.as_slice()) {
                (Some(path), _) => Some(Path::new(path)),
                (None, [path]) => Some(Path::new(path)),
                (None, []) => None,
                (None, _) => return Err(usage_error("`--print-config` takes at most one path")),
            };
            let mut project = match path {
                Some(file) => one_shot_project(config_path, file)?,
                None => working_project(config_path)?,
            };
            project.overlay_lint(&overlay);
            print_lint_config(project.linter_for(path).config());
            return Ok(());
        }

        let timings = self.timings.then(RuleTimings::new);

        // Stdin mode ignores positional paths.
        if let Some(stdin_path) = self.stdin_filepath.as_deref() {
            let mut project = one_shot_project(self.config.as_deref(), Path::new(stdin_path))?;
            project.overlay_lint(&overlay);
            let status = self.run_stdin(stdin_path, &project, timings.as_ref(), verbosity);
            if let Some(rule_timings) = &timings {
                print_rule_timings(rule_timings);
            }
            return status;
        }

        let files = project_files(
            self.config.as_deref(),
            &self.paths,
            self.no_error_on_unmatched_pattern,
            Some(&overlay),
        )?;

        let mut total_errors = 0;
        let mut total_warnings = 0;
        let mut total_fixed = 0;
        for_each_outcome_in_order(
            &files,
            |file, allocator| self.lint_file(allocator, file, timings.as_ref(), verbosity),
            |outcome, output| {
                if let Some(note) = &outcome.note {
                    let _ = writeln!(output.stderr, "{note}");
                }
                let _ = if self.format.is_for_tools() {
                    output.stdout.write_all(&outcome.rendered)
                } else {
                    output.stderr.write_all(&outcome.rendered)
                };
                total_errors += outcome.error_count;
                total_warnings += outcome.warning_count;
                total_fixed += usize::from(outcome.is_fixed);
                Ok(())
            },
        )?;

        if !self.silent && verbosity != Verbosity::Quiet {
            if total_fixed > 0 {
                eprintln!("Fixed {total_fixed} file(s)");
            }
            if total_errors > 0 || total_warnings > 0 {
                eprintln!(
                    "{total_errors} error(s), {total_warnings} warning(s) in {} file(s)",
                    files.len()
                );
            }
        }
        if let Some(rule_timings) = &timings {
            print_rule_timings(rule_timings);
        }

        self.exit_status(total_errors, total_warnings, verbosity)
    }

    /// `--fix` writes the fixed source to stdout, never to disk, and findings
    /// to stderr whatever their format, because stdout is the new file content.
    /// Otherwise output formats and the exit-code policy match the file walk.
    fn run_stdin(
        &self,
        stdin_path: &str,
        project: &Project,
        timings: Option<&RuleTimings>,
        verbosity: Verbosity,
    ) -> Result<(), Exit> {
        let source = read_stdin_source()?;

        let path = Path::new(stdin_path);
        let (definitions, linter) =
            (project.definitions_for(Some(path)), project.linter_for(Some(path)));
        let stdlib = definitions.stdlib().library();
        let allocator = Allocator::new();
        let fixes = self.fixes();
        let (current_source, messages, fix_error) =
            fixed_or_linted(&linter, &allocator, path, &source, stdlib, fixes, timings);

        if !fixes.is_empty() {
            // Echo the source even with nothing to fix, since editors and
            // pipelines take stdout as the new file content.
            write_output(None, current_source)?;
        }
        if let Some(error) = &fix_error {
            eprintln!("error: cannot fix `{stdin_path}`: {error}");
        }

        let (finding_errors, total_warnings) = severity_counts(&messages);
        let total_errors = finding_errors + usize::from(fix_error.is_some());

        let source_file = SourceFile { label: stdin_path, text: Some(current_source) };
        let diags = messages.into_iter().map(|message| message.diagnostic);
        let rendered = self.rendered(source_file, diags, verbosity);
        let _ = if self.format.is_for_tools() && fixes.is_empty() {
            io::stdout().write_all(&rendered)
        } else {
            io::stderr().write_all(&rendered)
        };

        self.exit_status(total_errors, total_warnings, verbosity)
    }

    /// The kinds of fix the run applies.
    const fn fixes(&self) -> FixKind {
        if self.fix_suggestions {
            FixKind::all()
        } else if self.fix {
            FixKind::Fix
        } else {
            FixKind::empty()
        }
    }

    fn exit_status(
        &self,
        total_errors: usize,
        total_warnings: usize,
        verbosity: Verbosity,
    ) -> Result<(), Exit> {
        if total_errors > 0 {
            return Err(Exit::Failure);
        }
        let warning_limit = if self.deny_warnings { Some(0) } else { self.max_warnings };
        if let Some(limit) = warning_limit
            && total_warnings > limit
        {
            // `--quiet` printed none of the warnings that fail the run.
            return Err(if verbosity == Verbosity::Quiet && !self.silent {
                operation_error(format!("{total_warnings} warning(s) exceed the limit of {limit}"))
            } else {
                Exit::Failure
            });
        }
        Ok(())
    }

    fn lint_file(
        &self,
        allocator: &Allocator,
        file: &ProjectFile,
        timings: Option<&RuleTimings>,
        verbosity: Verbosity,
    ) -> LintOutcome {
        let file_path = file.path.as_path();
        let label = file_path.to_string_lossy();
        let source = match FileSystemOs.read(file_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let source_file = SourceFile { label: &label, text: None };
                let diag = Diagnostic::e010(&label, &error);
                // A skipped file fails the run, so CI can't pass with files
                // left unlinted.
                return LintOutcome {
                    error_count: 1,
                    warning_count: 0,
                    is_fixed: false,
                    note: None,
                    rendered: self.rendered(source_file, [diag], verbosity),
                };
            }
        };

        let definitions = file.project.definitions_for(Some(file_path));
        let stdlib = definitions.stdlib().library();
        let linter = file.project.linter_for(Some(file_path));
        let (fixed, fixed_messages, fix_error) =
            fixed_or_linted(&linter, allocator, file_path, &source, stdlib, self.fixes(), timings);
        let mut note =
            fix_error.map(|error| format!("error: cannot fix `{}`: {error}", file_path.display()));
        let mut is_fixed = false;
        let (current_source, messages) = if fixed == source {
            (fixed, fixed_messages)
        } else if let Err(error) = fs::write(file_path, fixed) {
            note = Some(format!("error: cannot write `{}`: {error}", file_path.display()));
            (source.as_slice(), linted(&linter, allocator, file_path, &source, stdlib, timings))
        } else {
            is_fixed = true;
            (fixed, fixed_messages)
        };

        let (finding_errors, warning_count) = severity_counts(&messages);
        let source_file = SourceFile { label: &label, text: Some(current_source) };
        let diags = messages.into_iter().map(|message| message.diagnostic);
        let rendered = self.rendered(source_file, diags, verbosity);
        // Fixes that couldn't be made or written fail the run, as an unread
        // file does.
        let error_count = finding_errors + usize::from(note.is_some());
        LintOutcome { error_count, warning_count, is_fixed, note, rendered }
    }

    /// What the run prints for `diags`, all in `file`, in its format.
    /// `--quiet` drops the warnings from the output only: they still count
    /// toward the exit code.
    fn rendered(
        &self,
        file: SourceFile<'_>,
        diags: impl IntoIterator<Item = Diagnostic>,
        verbosity: Verbosity,
    ) -> Vec<u8> {
        let mut rendered = Vec::new();
        if self.silent {
            return rendered;
        }
        let shown: Vec<Diagnostic> = diags
            .into_iter()
            .filter(|diag| verbosity != Verbosity::Quiet || diag.is_error())
            .collect();
        self.format.reporter().render(file, &shown, &mut rendered);
        rendered
    }

    /// What the flags layer over every file's lint settings, the config's
    /// overrides included.
    fn lint_overlay(&self) -> LintConfig {
        LintConfig { extra_globals: self.globals.clone(), ..self.levels.0.clone() }
    }
}

/// `source` with its fixes of the kinds `fixes` holds applied, the fixed
/// text's messages, and why fixing failed, if it did.
///
/// A failure leaves `source` as it was, with its own messages. Each lint's
/// rules are timed into `timings` when given.
fn fixed_or_linted<'ast>(
    linter: &Linter,
    allocator: &'ast Allocator,
    path: &Path,
    source: &'ast [u8],
    stdlib: &StdlibLibrary<'_>,
    fixes: FixKind,
    timings: Option<&RuleTimings>,
) -> (&'ast [u8], Vec<Message>, Option<FixError>) {
    let parse = parsed(allocator, path, source);
    match linter.fix(allocator, parse, stdlib, fixes, timings) {
        Ok(Fixed { parse, messages }) => (parse.source, messages, None),
        Err(error) => {
            (source, linted(linter, allocator, path, source, stdlib, timings), Some(error))
        }
    }
}

/// `source`'s messages, parsed into `allocator` and linted against `stdlib`.
fn linted(
    linter: &Linter,
    allocator: &Allocator,
    path: &Path,
    source: &[u8],
    stdlib: &StdlibLibrary<'_>,
    timings: Option<&RuleTimings>,
) -> Vec<Message> {
    linter.run(&parsed(allocator, path, source), stdlib, FixKind::empty(), timings)
}

/// `source`'s parse into `allocator`, as a definition file when `path` names
/// one.
fn parsed<'ast>(allocator: &'ast Allocator, path: &Path, source: &'ast [u8]) -> ParseResult<'ast> {
    if is_definition_file(path) {
        luck_parser::parse_definitions(allocator, source)
    } else {
        luck_parser::parse(allocator, source)
    }
}

/// Count `messages` by severity, as `(errors, warnings)`.
fn severity_counts(messages: &[Message]) -> (usize, usize) {
    let count =
        |severity: Severity| messages.iter().filter(|message| message.severity == severity).count();
    (count(Severity::Error), count(Severity::Warning))
}

/// One line per rule: its name, its category, and the kinds its fixes come
/// in, comma-separated and empty for a rule with none.
fn print_lint_rules() {
    let mut rules = RuleId::ALL;
    rules.sort_unstable_by_key(|rule| rule.name());
    for rule in rules {
        let kinds: Vec<&str> = [(FixKind::Fix, "fix"), (FixKind::Suggestion, "suggestion")]
            .into_iter()
            .filter(|&(kind, _)| rule.fix_kind().contains(kind))
            .map(|(_, kind_name)| kind_name)
            .collect();
        println!("{}\t{}\t{}", rule.name(), rule.category().name(), kinds.join(","));
    }
}

/// Writes `timings` to stderr as oxlint's `--debug timings` does.
///
/// Each rule the run called gets a row, slowest first, with its time, its
/// share of every rule's, and how many times the linter called it.
fn print_rule_timings(timings: &RuleTimings) {
    let rows = timings.sorted();
    let rule_width = rows.iter().map(|row| row.rule.name().len()).fold("Rule".len(), usize::max);
    let calls_width =
        rows.iter().map(|row| row.calls.to_string().len()).fold("Calls".len(), usize::max);
    let total_seconds: f64 = rows.iter().map(|row| row.duration.as_secs_f64()).sum();
    let mut lines = vec![
        "Rule timings:".to_owned(),
        format!(
            "{:<rule_width$}  {:>10}  {:>8}  {:>calls_width$}",
            "Rule", "Time (ms)", "Relative", "Calls"
        ),
        format!("{:-<rule_width$}  {:-<10}  {:-<8}  {:-<calls_width$}", "", "", "", ""),
    ];
    lines.extend(rows.iter().map(|row| {
        let (name, seconds, calls) = (row.rule.name(), row.duration.as_secs_f64(), row.calls);
        let millis = seconds * 1000.0;
        let percent: f64 = if total_seconds > 0.0 { seconds / total_seconds * 100.0 } else { 0.0 };
        format!("{name:<rule_width$}  {millis:>10.3}  {percent:>7.1}%  {calls:>calls_width$}")
    }));
    eprintln!("{}", lines.join("\n"));
}

/// Prints `lint_config` as a `luck.json` `lint` section.
fn print_lint_config(lint_config: &LintConfig) {
    let json = serde_json::to_string_pretty(lint_config)
        .expect("a lint config holds only strings, numbers, and string-keyed maps");
    println!("{json}");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use clap::Parser as _;
    use clap::error::ErrorKind;
    use luck_stdlib::luau_library;

    use super::*;
    use crate::cli::Cli;
    use crate::commands::Command;

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
        let mut project = working_project(args.config.as_deref()).expect("config resolves");
        project.overlay_lint(&args.lint_overlay());
        project.linter_for(None).config().clone()
    }

    fn lint_source(source: &str, library: &StdlibLibrary<'_>, linter: &Linter) -> Vec<Message> {
        linted(linter, &Allocator::new(), Path::new("main.luau"), source.as_bytes(), library, None)
    }

    #[test]
    fn lint_is_top_level_command_with_oxlint_flags() {
        let LintArgs { paths, levels, fix, config, .. } = lint_args(&[
            "src",
            "main.luau",
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
        assert_eq!(paths, vec!["src".to_owned(), "main.luau".to_owned()]);
        assert_eq!(levels.0.rules.level(RuleId::UndefinedVariable), Some(RuleLevel::Error));
        assert_eq!(levels.0.categories.level(Category::Style), Some(RuleLevel::Off));
        assert_eq!(levels.0.rules.level(RuleId::Shadowing), Some(RuleLevel::Warn));
        assert!(fix);
        assert_eq!(config, Some(PathBuf::from("luck.json")));
    }

    #[test]
    fn lint_long_flags_parse() {
        let LintArgs {
            levels,
            max_warnings,
            deny_warnings,
            silent,
            no_error_on_unmatched_pattern,
            rules,
            print_config,
            timings,
            format,
            ..
        } = lint_args(&[
            "--deny",
            "shadowing",
            "--allow",
            "perf",
            "--warn",
            "empty_block",
            "--config",
            "c.json",
            "--max-warnings",
            "3",
            "--deny-warnings",
            "--silent",
            "--no-error-on-unmatched-pattern",
            "--rules",
            "--print-config",
            "--timings",
            "--format",
            "json",
        ]);
        assert_eq!(levels.0.rules.level(RuleId::Shadowing), Some(RuleLevel::Error));
        assert_eq!(levels.0.categories.level(Category::Performance), Some(RuleLevel::Off));
        assert_eq!(levels.0.rules.level(RuleId::EmptyBlock), Some(RuleLevel::Warn));
        assert_eq!(max_warnings, Some(3));
        assert!(deny_warnings);
        assert!(silent);
        assert!(no_error_on_unmatched_pattern);
        assert!(rules);
        assert!(print_config);
        assert!(timings);
        assert!(matches!(format, LintFormat::Json));
        assert!(matches!(lint_args(&["-f", "github"]).format, LintFormat::Github));
    }

    #[test]
    fn fix_flags_pick_the_kinds_of_fix_to_apply() {
        assert_eq!(lint_args(&[]).fixes(), FixKind::empty());
        assert_eq!(lint_args(&["--fix"]).fixes(), FixKind::Fix);
        assert_eq!(lint_args(&["--fix-suggestions"]).fixes(), FixKind::all());
        assert_eq!(lint_args(&["--fix", "--fix-suggestions"]).fixes(), FixKind::all());
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
            project_with_config(r#"{"lint":{"rules":{"empty_block":"error"}}}"#);

        let lint_config = resolved_lint_config(&lint_args(&["-c", &config_arg]));
        assert_eq!(lint_config.rules.level(RuleId::EmptyBlock), Some(RuleLevel::Error));
    }

    #[test]
    fn lint_cli_levels_win_over_config() {
        let (_directory, config_arg) = project_with_config(
            r#"{"lint":{"categories":{"style":"off"},"rules":{"empty_block":"error"}}}"#,
        );

        let lint_config = resolved_lint_config(&lint_args(&[
            "-c",
            &config_arg,
            "-A",
            "empty_block",
            "-W",
            "style",
        ]));
        assert_eq!(lint_config.rules.level(RuleId::EmptyBlock), Some(RuleLevel::Off));
        assert_eq!(lint_config.categories.level(Category::Style), Some(RuleLevel::Warn));
    }

    #[test]
    fn lint_level_flags_set_a_category_to_their_own_level() {
        let (_directory, config_arg) = project_with_config(r#"{"minify":false}"#);

        for (flag, level) in
            [("-A", RuleLevel::Off), ("-W", RuleLevel::Warn), ("-D", RuleLevel::Error)]
        {
            let lint_config = resolved_lint_config(&lint_args(&["-c", &config_arg, flag, "perf"]));
            assert_eq!(lint_config.categories.level(Category::Performance), Some(level));
        }
    }

    #[test]
    fn lint_level_flags_apply_in_command_line_order() {
        let (_directory, config_arg) = project_with_config(r#"{"minify":false}"#);
        let level_after = |flags: &[&str]| {
            let command_line: Vec<&str> =
                ["-c", config_arg.as_str()].iter().chain(flags).copied().collect();
            resolved_lint_config(&lint_args(&command_line)).rules.level(RuleId::EmptyBlock)
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
    fn level_flags_reject_a_name_no_rule_or_category_has() {
        let Err(error) = Cli::try_parse_from(["luck", "lint", "-D", "unsued_variable"]) else {
            panic!("an unknown rule must not parse");
        };
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("unknown lint rule `unsued_variable`, did you mean `unused_variable`?"),
            "{error}"
        );
    }

    #[test]
    fn quiet_hides_warnings_but_not_errors() {
        let source = "\
local unused = 1
print(undefined_var)";
        let diags = lint_source(source, luau_library(), &Linter::new(LintConfig::default()));
        let args = lint_args(&["--format", "json"]);
        let file = SourceFile { label: "a.luau", text: Some(source.as_bytes()) };
        let lines_when = |verbosity: Verbosity| {
            let shown = diags.iter().map(|message| message.diagnostic.clone());
            let rendered = args.rendered(file, shown, verbosity);
            String::from_utf8(rendered).expect("JSON is UTF-8")
        };

        let all_lines = lines_when(Verbosity::Normal);
        assert_eq!(all_lines.lines().count(), 2, "{all_lines}");
        let quiet_lines = lines_when(Verbosity::Quiet);
        assert_eq!(quiet_lines.lines().count(), 1, "{quiet_lines}");
        assert!(quiet_lines.contains(r#""severity":"error""#), "{quiet_lines}");
    }

    #[test]
    fn a_silent_run_renders_nothing() {
        let linter = Linter::new(LintConfig::default());
        let diags = lint_source("print(undefined_var)", luau_library(), &linter);
        assert!(!diags.is_empty(), "{diags:?}");
        let file = SourceFile { label: "a.luau", text: Some(b"print(undefined_var)") };
        let shown = diags.into_iter().map(|message| message.diagnostic);
        let rendered = lint_args(&["--silent"]).rendered(file, shown, Verbosity::Normal);
        assert!(rendered.is_empty(), "{}", String::from_utf8_lossy(&rendered));
    }

    /// The format's own output carries the failure, so tools reading the json
    /// or github format see it too.
    #[test]
    fn an_unreadable_file_reports_e010_in_the_run_format() {
        let directory = tempfile::tempdir().expect("tempdir");
        let project = Arc::new(Project::defaults(directory.path().to_path_buf(), FileSystemOs));
        let file = ProjectFile { path: directory.path().join("gone.luau"), project };

        let outcome =
            lint_args(&["-f", "json"]).lint_file(&Allocator::new(), &file, None, Verbosity::Normal);

        assert_eq!((outcome.error_count, outcome.note), (1, None));
        let rendered = String::from_utf8(outcome.rendered).expect("JSON is UTF-8");
        assert_eq!(rendered.lines().count(), 1, "{rendered}");
        assert!(rendered.contains(r#""code":"E010""#), "{rendered}");
    }

    #[test]
    fn lint_roblox_does_not_flag_game_global() {
        // With `@roblox` loaded, a file using `game` must lint clean.
        let (directory, config_arg) = project_with_config(r#"{"definitions":["@roblox"]}"#);

        let args = lint_args(&["-c", &config_arg]);
        let project = working_project(args.config.as_deref()).expect("config resolves");
        let linter = project.linter_for(Some(&directory.path().join("main.luau")));
        let library = project.definitions().stdlib().library();
        assert!(library.has_roblox);

        let diags = lint_source("print(game)", library, &linter);
        assert!(
            diags.iter().all(|diag| diag.code != "undefined_variable"),
            "roblox global `game` should not be undefined"
        );

        // Standalone Luau (the default) does flag it; that contrast is the point.
        let standalone_diags = lint_source("print(game)", luau_library(), &linter);
        assert!(standalone_diags.iter().any(|diag| diag.code == "undefined_variable"));
    }
}
