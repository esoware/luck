//! `luck lint` - config-driven, oxlint-style linting with `-A`/`-W`/`-D`
//! overrides, `--fix`, JSON output, and stdin mode.

use std::fs;
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use luck_core::{Diagnostic, LuaTarget, LuckConfig, read_source_file};
use luck_linter::{Category, LintConfig, LintDiagnostic, RuleSetting, Severity};

use crate::exit::{Exit, usage_error};
use crate::output::read_stdin_source;
use crate::project::{outcomes_in_order, project_files, resolve_project};
use crate::render::render_document_diags;
use crate::verbosity::Verbosity;

#[derive(Clone, ValueEnum)]
enum LintFormat {
    Default,
    Json,
}

#[derive(Args)]
#[expect(clippy::struct_excessive_bools, reason = "clap derive needs one bool field per flag")]
pub(crate) struct LintArgs {
    /// Files or directories to lint [default: current directory].
    paths: Vec<String>,

    /// Path to config file [default: discover luck.json upward from cwd].
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Turn off a rule, or a category other than correctness (repeatable).
    #[arg(short = 'A', long, value_name = "NAME")]
    allow: Vec<String>,

    /// Enable a rule as a warning, or a category's rules at their default
    /// severity (repeatable).
    #[arg(short = 'W', long, value_name = "NAME")]
    warn: Vec<String>,

    /// Enable a rule as an error, or a category's rules at their default
    /// severity (repeatable).
    #[arg(short = 'D', long, value_name = "NAME")]
    deny: Vec<String>,

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

    /// Print the registered rule names and categories, then exit.
    #[arg(long)]
    rules: bool,

    /// Print the effective lint configuration as JSON, then exit.
    #[arg(long)]
    print_config: bool,

    /// Read source from stdin, using PATH only to pick the target and
    /// label diagnostics; --fix writes the fixed source to stdout.
    #[arg(long, value_name = "PATH")]
    stdin_filepath: Option<String>,
}

/// One document's rendered diagnostics, in the stream their format writes to.
enum Rendered {
    /// ariadne output, for stderr.
    Pretty(Vec<u8>),
    /// One JSON object per line, for stdout.
    Json(String),
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
    /// Returns [`Exit::Usage`] on a bad config, rule name, or path, and
    /// [`Exit::Failure`] when the findings fail the run under the exit-code
    /// policy.
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let project = resolve_project(self.config.as_deref())?;
        let lint_config = self.lint_config(&project.config)?;

        if self.rules {
            print_lint_rules();
            return Ok(());
        }

        if self.print_config {
            print_lint_config(&lint_config);
            return Ok(());
        }

        // Stdin mode ignores positional paths.
        if let Some(stdin_path) = self.stdin_filepath.as_deref() {
            return self.run_stdin(stdin_path, &lint_config, &project.config);
        }

        let files = project_files(&project, &self.paths)?;

        let mut total_errors = 0usize;
        let mut total_warnings = 0usize;
        let mut total_fixed = 0usize;
        {
            // One lock and one flush per stream for the whole run.
            let mut stderr = BufWriter::new(io::stderr().lock());
            let mut stdout = BufWriter::new(io::stdout().lock());
            let outcomes = outcomes_in_order(&files, |file_path, target| {
                self.lint_file(file_path, target, &lint_config)
            });
            for outcome in outcomes {
                if let Some(note) = &outcome.note {
                    let _unused = writeln!(stderr, "{note}");
                }
                let _unused = match &outcome.rendered {
                    Some(Rendered::Pretty(pretty)) => stderr.write_all(pretty),
                    Some(Rendered::Json(lines)) => stdout.write_all(lines.as_bytes()),
                    None => Ok(()),
                };
                total_errors = total_errors.saturating_add(outcome.error_count);
                total_warnings = total_warnings.saturating_add(outcome.warning_count);
                total_fixed = total_fixed.saturating_add(usize::from(outcome.is_fixed));
            }
        }

        if !self.silent && self.fix && total_fixed > 0 && verbosity != Verbosity::Quiet {
            eprintln!("Fixed {total_fixed} file(s)");
        }

        if !self.silent && verbosity != Verbosity::Quiet && (total_errors > 0 || total_warnings > 0)
        {
            eprintln!(
                "{total_errors} error(s), {total_warnings} warning(s) in {} file(s)",
                files.len()
            );
        }

        self.exit_status(total_errors, total_warnings)
    }

    /// Lint one document from stdin as if it were the `--stdin-filepath` file.
    ///
    /// `--fix` writes the fixed source to stdout, never to disk. Output formats
    /// and the exit-code policy match the file walk.
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

        if !diags.is_empty() && !self.silent {
            match self.render(stdin_path, current_source, &diags) {
                Rendered::Pretty(pretty) => {
                    let _unused = io::stderr().write_all(&pretty);
                }
                Rendered::Json(lines) => print!("{lines}"),
            }
        }

        self.exit_status(total_errors, total_warnings)
    }

    /// Shared lint exit-code policy.
    ///
    /// # Errors
    ///
    /// Returns [`Exit::Failure`] on any error, and on warnings under
    /// `--deny-warnings` or when they exceed `--max-warnings`.
    fn exit_status(&self, total_errors: usize, total_warnings: usize) -> Result<(), Exit> {
        let has_too_many_warnings =
            self.max_warnings.is_some_and(|max_warnings| total_warnings > max_warnings);
        if total_errors > 0 || (self.deny_warnings && total_warnings > 0) || has_too_many_warnings {
            Err(Exit::Failure)
        } else {
            Ok(())
        }
    }

    /// Lint one file, applying and writing fixes if asked, and render its
    /// diagnostics in the requested format.
    fn lint_file(
        &self,
        file_path: &Path,
        target: LuaTarget,
        lint_config: &LintConfig,
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

        let (error_count, warning_count) = severity_counts(&diags);
        let rendered = (!diags.is_empty() && !self.silent)
            .then(|| self.render(&file_path.to_string_lossy(), current_source, &diags));
        LintOutcome { error_count, warning_count, is_fixed, note, rendered }
    }

    /// Renders one document's diagnostics in the requested format.
    fn render(&self, file_label: &str, source: String, diags: &[LintDiagnostic]) -> Rendered {
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
                ))
            }
            LintFormat::Json => {
                let mut lines = String::new();
                for diag in diags {
                    let json_value = serde_json::json!({
                        "file": file_label,
                        "rule": diag.rule,
                        "severity": diag.severity.name(),
                        "message": diag.message,
                        "help": diag.help,
                        "start": diag.span.start,
                        "end": diag.span.end,
                    });
                    lines.push_str(&json_value.to_string());
                    lines.push('\n');
                }
                Rendered::Json(lines)
            }
        }
    }

    /// The effective `LintConfig`: the project's, with the CLI overrides on
    /// top.
    ///
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] on an unknown rule
    /// name.
    fn lint_config(&self, luck_config: &LuckConfig) -> Result<LintConfig, Exit> {
        let mut lint_config = luck_config.lint.clone().unwrap_or_default();

        // CLI overrides win over the config. All -A apply first, then -W, then -D,
        // so for one name -D beats -W beats -A, whatever the command-line order.
        for name in &self.allow {
            if let Some(category) = Category::from_name(name) {
                lint_config.categories.retain(|existing| *existing != category);
            } else {
                lint_config
                    .rule_overrides
                    .insert(name.clone(), RuleSetting { enabled: Some(false), severity: None });
            }
        }
        for (names, severity) in [(&self.warn, Severity::Warning), (&self.deny, Severity::Error)] {
            for name in names {
                if let Some(category) = Category::from_name(name) {
                    if !lint_config.categories.contains(&category) {
                        lint_config.categories.push(category);
                    }
                } else {
                    lint_config.rule_overrides.insert(
                        name.clone(),
                        RuleSetting { enabled: Some(true), severity: Some(severity) },
                    );
                }
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

/// Count `diags` by severity, as `(errors, warnings)`.
fn severity_counts(diags: &[LintDiagnostic]) -> (usize, usize) {
    let count = |severity: Severity| diags.iter().filter(|diag| diag.severity == severity).count();
    (count(Severity::Error), count(Severity::Warning))
}

/// Print every registered rule with its category, sorted by name.
fn print_lint_rules() {
    let mut rules: Vec<(&str, &str)> =
        luck_linter::rules().map(|rule| (rule.name(), rule.category().name())).collect();
    rules.sort_unstable();
    for (name, category) in rules {
        println!("{name}\t{category}");
    }
}

/// Print the resolved effective lint configuration as JSON.
fn print_lint_config(lint_config: &LintConfig) {
    let overrides: serde_json::Map<String, serde_json::Value> = lint_config
        .rule_overrides
        .iter()
        .map(|(name, setting)| {
            let entry = serde_json::json!({
                "enabled": setting.enabled,
                "severity": setting.severity.map(Severity::name),
            });
            (name.clone(), entry)
        })
        .collect();
    let categories: Vec<&str> =
        lint_config.categories.iter().map(|category| category.name()).collect();
    let value = serde_json::json!({
        "rule_overrides": serde_json::Value::Object(overrides),
        "extra_globals": lint_config.extra_globals,
        "restricted_module_paths": lint_config.restricted_module_paths,
        "max_cyclomatic_complexity": lint_config.max_cyclomatic_complexity,
        "disable_default_rules": lint_config.disable_default_rules,
        "categories": categories,
    });
    println!("{value:#}");
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::{Cli, Command};

    /// `luck lint` parsed with `flags`, as clap builds it.
    fn lint_args(flags: &[&str]) -> LintArgs {
        let command_line = ["luck", "lint"].iter().chain(flags);
        let Command::Lint(args) = Cli::try_parse_from(command_line).expect("lint parses").command
        else {
            panic!("expected Command::Lint");
        };
        args
    }

    /// A project whose `luck.json` holds `config`, and its path as a
    /// command-line argument.
    fn project_with_config(config: &str) -> (tempfile::TempDir, String) {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, config).expect("write luck.json");
        let config_arg = config_path.to_str().expect("temp paths are UTF-8").to_owned();
        (directory, config_arg)
    }

    /// The lint config `args` resolve to, through the project config they name.
    fn resolved_lint_config(args: &LintArgs) -> LintConfig {
        let project = resolve_project(args.config.as_deref()).expect("config resolves");
        args.lint_config(&project.config).expect("lint config resolves")
    }

    #[test]
    fn lint_is_top_level_command_with_oxlint_flags() {
        let LintArgs { paths, deny, allow, warn, fix, config, .. } = lint_args(&[
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
        assert_eq!(deny, vec!["undefined_variable".to_owned()]);
        assert_eq!(allow, vec!["style".to_owned()]);
        assert_eq!(warn, vec!["shadowing".to_owned()]);
        assert!(fix);
        assert_eq!(config, Some(PathBuf::from("luck.json")));
    }

    #[test]
    fn lint_long_flag_aliases_parse() {
        let LintArgs { max_warnings, deny_warnings, silent, rules, print_config, format, .. } =
            lint_args(&[
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
                "--rules",
                "--print-config",
                "--format",
                "json",
            ]);
        assert_eq!(max_warnings, Some(3));
        assert!(deny_warnings);
        assert!(silent);
        assert!(rules);
        assert!(print_config);
        assert!(matches!(format, LintFormat::Json));
    }

    #[test]
    fn lint_stdin_filepath_parses() {
        assert_eq!(
            lint_args(&["--stdin-filepath", "buf.luau"]).stdin_filepath.as_deref(),
            Some("buf.luau")
        );
    }

    #[test]
    fn lint_config_honors_luck_json_rule_overrides() {
        // A config that turns the suspicious `empty_block` rule on as an error
        // must surface in the resolved LintConfig.
        let (_directory, config_arg) = project_with_config(
            r#"{"lua":"lua54","lint":{"rule_overrides":{"empty_block":{"enabled":true,"severity":"error"}}}}"#,
        );

        let lint_config = resolved_lint_config(&lint_args(&["-c", &config_arg]));
        let setting =
            lint_config.rule_overrides.get("empty_block").expect("empty_block override present");
        assert_eq!(setting.enabled, Some(true));
        assert_eq!(setting.severity, Some(Severity::Error));
    }

    #[test]
    fn lint_cli_overrides_win_over_config() {
        let (_directory, config_arg) = project_with_config(
            r#"{"lua":"lua54","lint":{"rule_overrides":{"empty_block":{"enabled":true,"severity":"error"}}}}"#,
        );

        let lint_config =
            resolved_lint_config(&lint_args(&["-c", &config_arg, "-A", "empty_block"]));
        let setting = lint_config.rule_overrides.get("empty_block").expect("override present");
        assert_eq!(setting.enabled, Some(false));
    }

    #[test]
    fn lint_warn_flag_adds_a_category() {
        let (_directory, config_arg) = project_with_config(r#"{"lua":"lua54"}"#);

        let lint_config = resolved_lint_config(&lint_args(&["-c", &config_arg, "-W", "style"]));
        assert!(lint_config.categories.contains(&Category::Style));
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
