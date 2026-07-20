//! `luck build` - drive the full bundle (and optional minify) pipeline from
//! `luck.json`, with `--watch`, `--dry-run`, and profile selection.

use crate::output::{build_file_cache, format_size};
use crate::render::render_diagnostics;
use crate::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE, Verbosity};
use clap::Args;
use luck_bundler::bundle;
use luck_core::config::{BuildConfig, resolve_build_config};
use notify_debouncer_mini::new_debouncer;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

#[derive(Args)]
pub(crate) struct BuildArgs {
    /// Path to config file
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Print what would be written without writing
    #[arg(long, conflicts_with = "watch")]
    dry_run: bool,

    /// Build profile to use (e.g. release, dev)
    #[arg(short = 'P', long, value_name = "NAME")]
    profile: Option<String>,

    /// Shorthand for --profile release
    #[arg(long, conflicts_with = "profile")]
    release: bool,

    /// Shorthand for --profile dev
    #[arg(long, conflicts_with = "profile")]
    dev: bool,

    /// Watch source files and rebuild on changes
    #[arg(short, long)]
    watch: bool,
}

impl BuildArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> ExitCode {
        let profile = self.resolved_profile();
        if self.watch {
            run_build_watch(self.config.as_deref(), profile, verbosity)
        } else {
            run_build(self.config.as_deref(), self.dry_run, profile, verbosity)
        }
    }

    /// Fold `--release`/`--dev` shorthands into the explicit `--profile` name.
    fn resolved_profile(&self) -> Option<&str> {
        if let Some(name) = self.profile.as_deref() {
            Some(name)
        } else if self.release {
            Some("release")
        } else if self.dev {
            Some("dev")
        } else {
            None
        }
    }
}

fn run_build(
    config_path: Option<&Path>,
    dry_run: bool,
    profile: Option<&str>,
    verbosity: Verbosity,
) -> ExitCode {
    let total_start = Instant::now();
    let configs = match resolve_build_config(config_path, profile) {
        Ok(configs) => configs,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    if verbosity != Verbosity::Quiet {
        print_build_header(&configs[0]);
    }

    for config in &configs {
        if let Err(message) = build_one(config, dry_run, verbosity) {
            eprintln!("\n  \x1b[31m✗\x1b[0m {message}");
            return ExitCode::from(EXIT_FAILURE);
        }
    }

    if verbosity != Verbosity::Quiet {
        print_build_footer(total_start.elapsed());
    }
    ExitCode::from(EXIT_SUCCESS)
}

/// Bundle, minify, and write one build config. Returns its source file paths
/// on success (used by watch mode to decide what to watch).
fn build_one(
    config: &BuildConfig,
    dry_run: bool,
    verbosity: Verbosity,
) -> Result<Vec<String>, String> {
    let start = Instant::now();

    match bundle(
        &config.entry,
        config.target,
        &config.search_paths,
        &config.rc_dir,
    ) {
        Ok(result) => {
            if !result.warnings.is_empty() && verbosity != Verbosity::Quiet {
                let mut cache = build_file_cache(&result.warnings);
                render_diagnostics(&result.warnings, &mut cache);
            }

            let source_files = result.source_files;
            let mut output = result.output;
            let pre_minify_size = output.len();

            if config.minify {
                let file_path = config.entry.display().to_string();
                match luck_minifier::minify(&output, config.target, &config.transforms, &file_path)
                {
                    Ok(minified) => output = minified,
                    Err(errors) => {
                        let mut cache = build_file_cache(&errors);
                        cache.add_file(file_path, output);
                        render_diagnostics(&errors, &mut cache);
                        return Err("minification failed".to_string());
                    }
                }
            }

            let preamble = build_preamble(config);
            if !preamble.is_empty() {
                output = format!("{preamble}\n{output}");
            }

            if dry_run {
                eprintln!("[dry-run] Would write: {}", config.output.display());
            } else {
                crate::output::write_output_file(&config.output, &output);
            }

            let duration = start.elapsed();
            if verbosity != Verbosity::Quiet {
                let original = if config.minify {
                    Some(pre_minify_size)
                } else {
                    None
                };
                print_build_stat(&config.output, output.len(), original, duration);
            }

            Ok(source_files)
        }
        Err(errors) => {
            let mut cache = build_file_cache(&errors);
            render_diagnostics(&errors, &mut cache);
            Err("build failed".to_string())
        }
    }
}

fn build_preamble(config: &BuildConfig) -> String {
    let mut lines = Vec::new();

    if let Some(text) = &config.preamble {
        for line in text.lines() {
            lines.push(format!("-- {line}"));
        }
    }

    if config.luck_preamble {
        let action = if config.minify {
            "Bundled & minified"
        } else {
            "Bundled"
        };
        let version = luck::VERSION;
        lines.push(format!(
            "-- {action} with luck v{version} // https://github.com/esoware/luck"
        ));
    }

    lines.join("\n")
}

fn run_build_watch(
    config_path: Option<&Path>,
    profile: Option<&str>,
    verbosity: Verbosity,
) -> ExitCode {
    let config_path_owned = config_path.map(Path::to_path_buf);

    let mut watched_paths =
        run_build_collect_paths(config_path_owned.as_deref(), profile, verbosity);

    loop {
        let mut dirs: HashSet<PathBuf> = HashSet::new();
        for file in &watched_paths {
            let path = PathBuf::from(file.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = path.parent() {
                dirs.insert(parent.to_path_buf());
            }
        }
        if let Some(config_path) = &config_path_owned
            && let Some(parent) = config_path.parent()
        {
            dirs.insert(parent.to_path_buf());
        }
        // The DISCOVERED config (no -c) must be watched too, or config edits
        // never retrigger a rebuild.
        if config_path_owned.is_none()
            && let Ok(cwd) = std::env::current_dir()
            && let Ok(Some((discovered_path, _))) = luck_core::config::discover_config(&cwd)
            && let Some(parent) = discovered_path.parent()
        {
            dirs.insert(parent.to_path_buf());
        }
        // A failed first build used to leave NOTHING watched: the session sat
        // on `[watching for changes...]` forever with no way to recover. Fall
        // back to watching the working directory.
        if dirs.is_empty()
            && let Ok(cwd) = std::env::current_dir()
        {
            dirs.insert(cwd);
        }

        let (sender, receiver) = std::sync::mpsc::channel();
        let mut debouncer = match new_debouncer(Duration::from_millis(200), sender) {
            Ok(debouncer) => debouncer,
            Err(error) => {
                eprintln!("Error: failed to create file watcher: {error}");
                return ExitCode::from(EXIT_FAILURE);
            }
        };

        for dir in &dirs {
            if dir.exists()
                && let Err(error) = debouncer
                    .watcher()
                    .watch(dir, notify::RecursiveMode::Recursive)
            {
                eprintln!("Warning: failed to watch {}: {error}", dir.display());
            }
        }

        eprintln!("\n[watching for changes...]");

        loop {
            match receiver.recv() {
                Ok(Ok(events)) => {
                    if events.iter().any(|event| is_relevant_change(&event.path)) {
                        break;
                    }
                }
                Ok(Err(errors)) => {
                    eprintln!("Watch error: {errors}");
                }
                Err(_) => return ExitCode::from(EXIT_SUCCESS), // channel closed
            }
        }

        while receiver.try_recv().is_ok() {}

        eprintln!();

        watched_paths = run_build_collect_paths(config_path_owned.as_deref(), profile, verbosity);
    }
}

/// Only source/config changes trigger a rebuild - editor temp files and .git
/// churn used to retrigger constantly.
fn is_relevant_change(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("lua" | "luau" | "json") => true,
        _ => path
            .file_name()
            .is_some_and(|name| name == ".luaurc" || name == "luck.json"),
    }
}

fn run_build_collect_paths(
    config_path: Option<&Path>,
    profile: Option<&str>,
    verbosity: Verbosity,
) -> Vec<String> {
    let configs = match resolve_build_config(config_path, profile) {
        Ok(configs) => configs,
        Err(error) => {
            eprintln!("Error: {error}");
            return Vec::new();
        }
    };

    let mut all_source_files = Vec::new();
    for config in &configs {
        if let Ok(files) = build_one(config, false, verbosity) {
            all_source_files.extend(files);
        }
    }

    if let Some(config_path) = config_path {
        all_source_files.push(config_path.display().to_string());
    }

    all_source_files
}

fn format_duration(duration: Duration) -> String {
    let ms = duration.as_millis();
    if ms >= 1000 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{ms}ms")
    }
}

fn relative_display(path: &Path) -> String {
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(rel) = path.strip_prefix(&cwd)
    {
        return rel.display().to_string().replace('\\', "/");
    }
    path.display().to_string().replace('\\', "/")
}

fn print_build_header(config: &BuildConfig) {
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("\x1b[1;36mluck\x1b[0m \x1b[2mv{version}\x1b[0m\n");
    eprintln!("  \x1b[2mTarget:\x1b[0m  {}", config.target);
    eprintln!(
        "  \x1b[2mEntry:\x1b[0m   {}",
        relative_display(&config.entry)
    );
    if config.minify {
        eprintln!("  \x1b[2mMinify:\x1b[0m  \x1b[32menabled\x1b[0m");
    } else {
        eprintln!("  \x1b[2mMinify:\x1b[0m  \x1b[2mdisabled\x1b[0m");
    }
    eprintln!();
}

fn print_build_stat(
    path: &Path,
    output_size: usize,
    original_size: Option<usize>,
    duration: Duration,
) {
    let size_str = format_size(output_size);
    let path_str = relative_display(path);
    let time_str = format_duration(duration);

    if let Some(orig) = original_size
        && orig > 0
    {
        let pct = ((orig as f64 - output_size as f64) / orig as f64 * 100.0) as i64;
        eprintln!(
            "  \x1b[1m{path_str}\x1b[0m  \x1b[2m{}\x1b[0m \x1b[2m→\x1b[0m \x1b[32m{size_str}\x1b[0m  \x1b[2m(-{pct}%)\x1b[0m  \x1b[2m{time_str}\x1b[0m",
            format_size(orig),
        );
        return;
    }
    eprintln!("  \x1b[1m{path_str}\x1b[0m  \x1b[32m{size_str}\x1b[0m  \x1b[2m{time_str}\x1b[0m");
}

fn print_build_footer(total_duration: Duration) {
    eprintln!(
        "\n  \x1b[32m✓\x1b[0m Done in \x1b[1m{}\x1b[0m",
        format_duration(total_duration)
    );
}
