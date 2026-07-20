//! `luck lint` - config-driven, oxlint-style linting with `-A`/`-W`/`-D`
//! overrides, `--fix`, JSON output, and stdin mode.

use crate::output::parallel_chunk_size;
use crate::project::{collect_target_files, project_filter, resolve_project_config};
use crate::render::{FileCache, render_diagnostics_to_buffer};
use crate::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE, Verbosity};
use clap::{Args, ValueEnum};
use luck_core::diagnostics::Diagnostic;
use luck_core::types::LuaTarget;
use luck_linter::diagnostic::{Category, LintDiagnostic, Severity};
use luck_linter::{LintConfig, RuleSetting};
use std::path::{Path, PathBuf};
use std::process;
use std::process::ExitCode;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LintFormat {
    Default,
    Json,
}

#[derive(Args)]
pub(crate) struct LintArgs {
    /// Files or directories to lint [default: current directory]
    paths: Vec<String>,

    /// Path to config file [default: discover luck.json upward from cwd]
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Allow a rule or category, turning it off (repeatable)
    #[arg(short = 'A', long, value_name = "NAME")]
    allow: Vec<String>,

    /// Enable a rule or category as a warning (repeatable)
    #[arg(short = 'W', long, value_name = "NAME")]
    warn: Vec<String>,

    /// Enable a rule or category as an error (repeatable)
    #[arg(short = 'D', long, value_name = "NAME")]
    deny: Vec<String>,

    /// Additional allowed global names (repeatable)
    #[arg(long = "global", value_name = "NAME")]
    globals: Vec<String>,

    /// Apply auto-fixes and write them back to source files
    #[arg(long)]
    fix: bool,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = LintFormat::Default)]
    format: LintFormat,

    /// Exit with an error if there are more than this many warnings
    #[arg(long, value_name = "INT")]
    max_warnings: Option<usize>,

    /// Treat warnings as errors for the exit code
    #[arg(long)]
    deny_warnings: bool,

    /// Suppress all diagnostic output (exit code still reflects results)
    #[arg(long)]
    silent: bool,

    /// Print the registered rule names and categories, then exit
    #[arg(long)]
    rules: bool,

    /// Print the resolved effective lint configuration, then exit
    #[arg(long)]
    print_config: bool,

    /// Read source from stdin, using PATH only to pick the target and
    /// label diagnostics; --fix writes the fixed source to stdout
    #[arg(long, value_name = "PATH")]
    stdin_filepath: Option<String>,
}

impl LintArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> ExitCode {
        let (lint_config, luck_config, config_dir) = resolve_lint_config(&self);

        if self.rules {
            print_lint_rules();
            return ExitCode::from(EXIT_SUCCESS);
        }

        if self.print_config {
            print_lint_config(&lint_config);
            return ExitCode::from(EXIT_SUCCESS);
        }

        // Resolve the target lazily per file, defaulting to the config dialects.
        let target_for = |path: &Path| -> LuaTarget {
            luck_config.target_for_path(path).unwrap_or_else(|message| {
                eprintln!("Error: {message}");
                process::exit(EXIT_USAGE as i32);
            })
        };

        // Stdin mode lints a single virtual document; --fix writes the fixed
        // source to stdout instead of disk, ignoring positional paths.
        if let Some(stdin_path) = self.stdin_filepath.as_deref() {
            return self.run_stdin(stdin_path, &lint_config, target_for);
        }

        let filter = project_filter(&config_dir, &luck_config);
        let files = collect_target_files(&self.paths, &filter);

        // Files lint in parallel; each worker renders its diagnostics into
        // buffers which are then flushed IN INPUT ORDER, so output is
        // byte-identical to the sequential loop.
        use rayon::prelude::*;
        let mut total_errors = 0u32;
        let mut total_warnings = 0u32;
        let mut total_fixed = 0u32;
        {
            // One lock and one flush per stream for the whole run; files are
            // processed in bounded sorted groups so peak outcome memory stays
            // flat on huge projects while output order is preserved.
            use std::io::Write;
            let mut stderr = std::io::BufWriter::new(std::io::stderr().lock());
            let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
            for chunk in files.chunks(parallel_chunk_size()) {
                let outcomes: Vec<LintOutcome> = chunk
                    .par_iter()
                    .map(|file_path| {
                        lint_file(
                            file_path,
                            target_for(file_path),
                            &lint_config,
                            self.fix,
                            self.silent,
                            self.format,
                        )
                    })
                    .collect();

                for outcome in &outcomes {
                    for note in &outcome.stderr_notes {
                        let _ = writeln!(stderr, "{note}");
                    }
                    let _ = stderr.write_all(&outcome.stderr_render);
                    let _ = stdout.write_all(outcome.stdout_lines.as_bytes());
                    total_errors += outcome.errors;
                    total_warnings += outcome.warnings;
                    total_fixed += u32::from(outcome.fixed);
                }
            }
        }

        if !self.silent && self.fix && total_fixed > 0 && verbosity == Verbosity::Normal {
            eprintln!("Fixed {total_fixed} file(s)");
        }

        if !self.silent
            && verbosity == Verbosity::Normal
            && (total_errors > 0 || total_warnings > 0)
        {
            eprintln!(
                "{total_errors} error(s), {total_warnings} warning(s) in {} file(s)",
                files.len()
            );
        }

        self.exit_code(total_errors, total_warnings)
    }

    /// Lint a single document read from stdin, labeled and targeted by the
    /// `--stdin-filepath` value. `--fix` writes the fixed source to stdout
    /// (never to disk); diagnostics, formats, and the exit-code policy match
    /// the file path.
    fn run_stdin(
        &self,
        stdin_path: &str,
        lint_config: &LintConfig,
        target_for: impl Fn(&Path) -> LuaTarget,
    ) -> ExitCode {
        let source = match crate::output::read_stdin_source() {
            Ok(text) => text,
            Err(code) => return code,
        };

        let target = target_for(&PathBuf::from(stdin_path));
        let mut diagnostics = luck_linter::lint_target(&source, target, lint_config);

        if self.fix {
            // `--fix` over stdin must ALWAYS echo the resulting buffer to
            // stdout: it is the editor's / pipeline's new file content.
            // Emitting nothing on a no-op would truncate
            // `luck lint --fix --stdin-filepath f < f > f`. Apply fixes when
            // any exist, otherwise echo the original unchanged.
            let fixed_source = if diagnostics.iter().any(|diag| diag.fix.is_some()) {
                luck_linter::apply_fixes(&source, &diagnostics, target.lua_version())
            } else {
                source.clone()
            };
            print!("{fixed_source}");
            diagnostics = luck_linter::lint_target(&fixed_source, target, lint_config);
        }

        let mut total_errors = 0u32;
        let mut total_warnings = 0u32;
        for diag in &diagnostics {
            match diag.severity {
                Severity::Error => total_errors += 1,
                Severity::Warning => total_warnings += 1,
            }
        }

        if !diagnostics.is_empty() && !self.silent {
            let (render, lines) =
                render_lint_diagnostics(self.format, stdin_path, source, &diagnostics);
            if !render.is_empty() {
                use std::io::Write;
                let _ = std::io::stderr().write_all(&render);
            }
            if !lines.is_empty() {
                print!("{lines}");
            }
        }

        self.exit_code(total_errors, total_warnings)
    }

    /// Shared lint exit-code policy: any error fails; warnings fail only under
    /// `--deny-warnings` or when they exceed `--max-warnings`.
    fn exit_code(&self, total_errors: u32, total_warnings: u32) -> ExitCode {
        let warnings_exceed = self
            .max_warnings
            .is_some_and(|max| total_warnings as usize > max);
        if total_errors > 0 || (self.deny_warnings && total_warnings > 0) || warnings_exceed {
            ExitCode::from(EXIT_FAILURE)
        } else {
            ExitCode::from(EXIT_SUCCESS)
        }
    }
}

/// Per-file lint result, buffered so parallel workers never interleave output.
struct LintOutcome {
    errors: u32,
    warnings: u32,
    fixed: bool,
    stderr_notes: Vec<String>,
    stderr_render: Vec<u8>,
    stdout_lines: String,
}

/// Lint one file, optionally applying and writing fixes, rendering its
/// diagnostics into buffers keyed to the requested format.
fn lint_file(
    file_path: &Path,
    target: LuaTarget,
    lint_config: &LintConfig,
    fix: bool,
    silent: bool,
    format: LintFormat,
) -> LintOutcome {
    let mut outcome = LintOutcome {
        errors: 0,
        warnings: 0,
        fixed: false,
        stderr_notes: Vec::new(),
        stderr_render: Vec::new(),
        stdout_lines: String::new(),
    };

    let source = match luck_core::source_io::read_source_file(file_path) {
        Ok(text) => text,
        Err(error) => {
            outcome.stderr_notes.push(format!(
                "Error: cannot read {}: {error}",
                file_path.display()
            ));
            // A skipped file must fail the run - CI going green while files
            // silently went unlinted is worse than a hard stop.
            outcome.errors += 1;
            return outcome;
        }
    };

    let mut diagnostics = luck_linter::lint_target(&source, target, lint_config);
    let mut current_source = source;

    if fix && diagnostics.iter().any(|diag| diag.fix.is_some()) {
        let fixed_source =
            luck_linter::apply_fixes(&current_source, &diagnostics, target.lua_version());
        if fixed_source != current_source {
            if let Err(error) = std::fs::write(file_path, &fixed_source) {
                outcome.stderr_notes.push(format!(
                    "Error: cannot write {}: {error}",
                    file_path.display()
                ));
            } else {
                outcome.fixed = true;
                diagnostics = luck_linter::lint_target(&fixed_source, target, lint_config);
                current_source = fixed_source;
            }
        }
    }

    for diag in &diagnostics {
        match diag.severity {
            Severity::Error => outcome.errors += 1,
            Severity::Warning => outcome.warnings += 1,
        }
    }

    if !diagnostics.is_empty() && !silent {
        let file_label = file_path.to_string_lossy().to_string();
        let (render, lines) =
            render_lint_diagnostics(format, &file_label, current_source, &diagnostics);
        outcome.stderr_render = render;
        outcome.stdout_lines = lines;
    }
    outcome
}

/// Renders one file's diagnostics: pretty output as stderr bytes, JSON as
/// stdout lines. Callers flush the buffers in input order.
fn render_lint_diagnostics(
    format: LintFormat,
    file_label: &str,
    source: String,
    diagnostics: &[LintDiagnostic],
) -> (Vec<u8>, String) {
    match format {
        LintFormat::Default => {
            let rendered: Vec<Diagnostic> = diagnostics
                .iter()
                .map(|diag| match diag.severity {
                    Severity::Error => Diagnostic::error_at(
                        diag.rule,
                        diag.message.clone(),
                        file_label.to_string(),
                        diag.span,
                    ),
                    Severity::Warning => Diagnostic::warning_at(
                        diag.rule,
                        diag.message.clone(),
                        file_label.to_string(),
                        diag.span,
                    ),
                })
                .collect();
            let mut cache = FileCache::new();
            cache.add_file(file_label.to_string(), source);
            (
                render_diagnostics_to_buffer(&rendered, &mut cache),
                String::new(),
            )
        }
        LintFormat::Json => {
            let mut lines = String::new();
            for diag in diagnostics {
                let severity = match diag.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                };
                let json_value = serde_json::json!({
                    "file": file_label,
                    "rule": diag.rule,
                    "severity": severity,
                    "message": diag.message,
                    "start": diag.span.start,
                    "end": diag.span.end,
                });
                lines.push_str(&json_value.to_string());
                lines.push('\n');
            }
            (Vec::new(), lines)
        }
    }
}

/// Resolve the effective `LintConfig` from the project config plus CLI
/// overrides, and return it alongside the resolved `LuckConfig` (for target
/// selection and include/exclude globs) and the config directory.
fn resolve_lint_config(args: &LintArgs) -> (LintConfig, luck_core::config::LuckConfig, PathBuf) {
    let (luck_config, config_dir) = resolve_project_config(args.config.as_deref());

    let mut lint_config = luck_config.lint.clone().unwrap_or_default();

    // CLI overrides win over config, applied left-to-right per oxlint.
    for name in &args.allow {
        if let Some(category) = category_from_name(name) {
            lint_config
                .categories
                .retain(|existing| *existing != category);
        } else {
            lint_config.rule_overrides.insert(
                name.clone(),
                RuleSetting {
                    enabled: Some(false),
                    severity: None,
                },
            );
        }
    }
    for name in &args.warn {
        if let Some(category) = category_from_name(name) {
            if !lint_config.categories.contains(&category) {
                lint_config.categories.push(category);
            }
        } else {
            lint_config.rule_overrides.insert(
                name.clone(),
                RuleSetting {
                    enabled: Some(true),
                    severity: Some(Severity::Warning),
                },
            );
        }
    }
    for name in &args.deny {
        if let Some(category) = category_from_name(name) {
            if !lint_config.categories.contains(&category) {
                lint_config.categories.push(category);
            }
        } else {
            lint_config.rule_overrides.insert(
                name.clone(),
                RuleSetting {
                    enabled: Some(true),
                    severity: Some(Severity::Error),
                },
            );
        }
    }

    lint_config
        .extra_globals
        .extend(args.globals.iter().cloned());

    // Validate every rule-override name (config-file and CLI-supplied) against
    // the linter registry now that the final config is assembled. Unknown names
    // would otherwise be silently ignored.
    let unknown = luck_linter::unknown_rule_names(&lint_config);
    if !unknown.is_empty() {
        eprintln!("Error: unknown lint rule name(s): {}", unknown.join(", "));
        process::exit(EXIT_USAGE as i32);
    }

    (lint_config, luck_config, config_dir)
}

/// Map an `-A`/`-W`/`-D` name to a lint category, if it names one.
fn category_from_name(name: &str) -> Option<Category> {
    match name.to_ascii_lowercase().as_str() {
        "correctness" => Some(Category::Correctness),
        "suspicious" => Some(Category::Suspicious),
        "style" => Some(Category::Style),
        "performance" | "perf" => Some(Category::Performance),
        _ => None,
    }
}

fn category_label(category: Category) -> &'static str {
    match category {
        Category::Correctness => "correctness",
        Category::Suspicious => "suspicious",
        Category::Style => "style",
        Category::Performance => "performance",
    }
}

/// Print every registered rule with its category, sorted by name.
fn print_lint_rules() {
    let rules = luck_linter::rules::all_rules();
    let mut listing: Vec<(&str, &str)> = rules
        .iter()
        .map(|rule| (rule.name(), category_label(rule.category())))
        .collect();
    listing.sort_unstable();
    for (name, category) in listing {
        println!("{name}\t{category}");
    }
}

/// Print the resolved effective lint configuration as JSON.
fn print_lint_config(lint_config: &LintConfig) {
    let mut overrides = serde_json::Map::new();
    for (name, setting) in &lint_config.rule_overrides {
        let severity = setting.severity.map(|severity| match severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        });
        overrides.insert(
            name.clone(),
            serde_json::json!({
                "enabled": setting.enabled,
                "severity": severity,
            }),
        );
    }
    let categories: Vec<&str> = lint_config
        .categories
        .iter()
        .map(|category| category_label(*category))
        .collect();
    let value = serde_json::json!({
        "rule_overrides": serde_json::Value::Object(overrides),
        "extra_globals": lint_config.extra_globals,
        "restricted_module_paths": lint_config.restricted_module_paths,
        "max_cyclomatic_complexity": lint_config.max_cyclomatic_complexity,
        "disable_default_rules": lint_config.disable_default_rules,
        "categories": categories,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("failed to serialize lint config")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Cli, Command};
    use clap::Parser;

    /// Build a `LintArgs` with everything defaulted.
    fn lint_args(config: Option<PathBuf>) -> LintArgs {
        LintArgs {
            paths: Vec::new(),
            config,
            allow: Vec::new(),
            warn: Vec::new(),
            deny: Vec::new(),
            globals: Vec::new(),
            fix: false,
            format: LintFormat::Default,
            max_warnings: None,
            deny_warnings: false,
            silent: true,
            rules: false,
            print_config: false,
            stdin_filepath: None,
        }
    }

    #[test]
    fn lint_is_top_level_command_with_oxlint_flags() {
        let cli = Cli::try_parse_from([
            "luck",
            "lint",
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
        ])
        .expect("lint parses");
        match cli.command {
            Command::Lint(LintArgs {
                paths,
                deny,
                allow,
                warn,
                fix,
                config,
                ..
            }) => {
                assert_eq!(paths, vec!["src".to_string(), "main.lua".to_string()]);
                assert_eq!(deny, vec!["undefined_variable".to_string()]);
                assert_eq!(allow, vec!["style".to_string()]);
                assert_eq!(warn, vec!["shadowing".to_string()]);
                assert!(fix);
                assert_eq!(config, Some(PathBuf::from("luck.json")));
            }
            _ => panic!("expected Command::Lint"),
        }
    }

    #[test]
    fn lint_long_flag_aliases_parse() {
        let cli = Cli::try_parse_from([
            "luck",
            "lint",
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
        ])
        .expect("lint long flags parse");
        match cli.command {
            Command::Lint(LintArgs {
                max_warnings,
                deny_warnings,
                silent,
                rules,
                print_config,
                format,
                ..
            }) => {
                assert_eq!(max_warnings, Some(3));
                assert!(deny_warnings);
                assert!(silent);
                assert!(rules);
                assert!(print_config);
                assert!(matches!(format, LintFormat::Json));
            }
            _ => panic!("expected Command::Lint"),
        }
    }

    #[test]
    fn lint_stdin_filepath_parses() {
        let lint = Cli::try_parse_from(["luck", "lint", "--stdin-filepath", "buf.luau"])
            .expect("lint --stdin-filepath parses");
        assert!(matches!(
            lint.command,
            Command::Lint(LintArgs { stdin_filepath: Some(ref path), .. }) if path == "buf.luau"
        ));
    }

    #[test]
    fn category_name_classification() {
        assert_eq!(category_from_name("style"), Some(Category::Style));
        assert_eq!(category_from_name("perf"), Some(Category::Performance));
        assert_eq!(category_from_name("undefined_variable"), None);
    }

    #[test]
    fn lint_config_honors_luck_json_rule_overrides() {
        // A config that turns the suspicious `empty_block` rule on as an error
        // must surface in the resolved LintConfig.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(
            &config_path,
            r#"{"lua":"lua54","lint":{"rule_overrides":{"empty_block":{"enabled":true,"severity":"error"}}}}"#,
        )
        .expect("write luck.json");

        let (lint_config, _, _) = resolve_lint_config(&lint_args(Some(config_path)));
        let setting = lint_config
            .rule_overrides
            .get("empty_block")
            .expect("empty_block override present");
        assert_eq!(setting.enabled, Some(true));
        assert_eq!(setting.severity, Some(Severity::Error));
    }

    #[test]
    fn lint_cli_overrides_win_over_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(
            &config_path,
            r#"{"lua":"lua54","lint":{"rule_overrides":{"empty_block":{"enabled":true,"severity":"error"}}}}"#,
        )
        .expect("write luck.json");

        let mut args = lint_args(Some(config_path));
        args.allow = vec!["empty_block".to_string()];
        let (lint_config, _, _) = resolve_lint_config(&args);
        let setting = lint_config
            .rule_overrides
            .get("empty_block")
            .expect("override present");
        assert_eq!(setting.enabled, Some(false));
    }

    #[test]
    fn lint_warn_category_accumulates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(&config_path, r#"{"lua":"lua54"}"#).expect("write luck.json");

        let mut args = lint_args(Some(config_path));
        args.warn = vec!["style".to_string()];
        let (lint_config, _, _) = resolve_lint_config(&args);
        assert!(lint_config.categories.contains(&Category::Style));
    }

    #[test]
    fn lint_roblox_luau_does_not_flag_game_global() {
        // With `"luau":"roblox"`, a `.luau` file using `game` must lint clean,
        // because target selection is per-file from the config dialect.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(&config_path, r#"{"luau":"roblox"}"#).expect("write luck.json");

        let (lint_config, luck_config, _) = resolve_lint_config(&lint_args(Some(config_path)));

        let luau_file = dir.path().join("main.luau");
        let target = luck_config
            .target_for_path(&luau_file)
            .expect("luau target resolves");
        assert_eq!(target, LuaTarget::LuauRoblox);

        let diagnostics = luck_linter::lint_target("print(game)", target, &lint_config);
        assert!(
            diagnostics
                .iter()
                .all(|diag| diag.rule != "undefined_variable"),
            "roblox global `game` should not be undefined"
        );

        // Standalone Luau (default) would flag it - sanity check the contrast.
        let standalone = luck_linter::lint_target("print(game)", LuaTarget::Luau, &lint_config);
        assert!(
            standalone
                .iter()
                .any(|diag| diag.rule == "undefined_variable")
        );
    }
}
