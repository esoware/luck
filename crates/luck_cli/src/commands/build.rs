//! `luck build` - drive the full bundle (and optional minify) pipeline from
//! `luck.json`, with `--watch`, `--dry-run`, and profile selection.

use core::time::Duration;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use clap::Args;
use luck_bundler::bundle;
use luck_core::{BuildConfig, discover_config, native_path, resolve_build_config};
use notify_debouncer_mini::{DebounceEventResult, new_debouncer, notify::RecursiveMode};

use crate::exit::{Exit, operation_error, usage_error};
use crate::output::{format_size, write_output_file};
use crate::render::{print_diags, print_document_diags};
use crate::verbosity::Verbosity;

#[derive(Args)]
#[expect(clippy::struct_excessive_bools, reason = "clap derive needs one bool field per flag")]
pub(crate) struct BuildArgs {
    /// Path to config file [default: discover luck.json upward from cwd].
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Print what would be written without writing.
    #[arg(long, conflicts_with = "watch")]
    dry_run: bool,

    /// Build profile to use (e.g. release, dev).
    #[arg(short = 'P', long, value_name = "NAME")]
    profile: Option<String>,

    /// Shorthand for --profile release.
    #[arg(long, conflicts_with = "profile")]
    release: bool,

    /// Shorthand for --profile dev.
    #[arg(long, conflicts_with = "profile")]
    dev: bool,

    /// Watch source files and rebuild on changes.
    #[arg(short, long)]
    watch: bool,
}

/// Why one build config didn't produce output.
enum BuildFailure {
    /// Bundling or minifying failed and the diagnostics are already rendered.
    /// Watch mode keeps going.
    Failed(&'static str),
    /// The output couldn't be written. The run ends with this exit code, even
    /// in watch mode.
    Fatal(Exit),
}

impl BuildArgs {
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config, and [`Exit::Failure`] when a
    /// build fails, its output can't be written, or the file watcher can't
    /// start.
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        if self.watch { self.rebuild_on_change(verbosity) } else { self.build_once(verbosity) }
    }

    /// Fold `--release`/`--dev` shorthands into the explicit `--profile` name.
    fn resolved_profile(&self) -> Option<&str> {
        self.profile
            .as_deref()
            .or_else(|| self.release.then_some("release"))
            .or_else(|| self.dev.then_some("dev"))
    }

    /// Build every config once.
    ///
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config, and [`Exit::Failure`] when a
    /// build fails or its output can't be written.
    fn build_once(&self, verbosity: Verbosity) -> Result<(), Exit> {
        let total_start = Instant::now();
        let configs = resolve_build_config(self.config.as_deref(), self.resolved_profile())
            .map_err(usage_error)?;

        if verbosity != Verbosity::Quiet
            && let Some(first_config) = configs.first()
        {
            print_build_header(first_config);
        }

        for config in &configs {
            match self.build_one(config, verbosity) {
                Ok(_) => {}
                Err(BuildFailure::Failed(message)) => {
                    eprintln!("\n  \x1b[31m✗\x1b[0m {message}");
                    return Err(Exit::Failure);
                }
                Err(BuildFailure::Fatal(exit)) => return Err(exit),
            }
        }

        if verbosity != Verbosity::Quiet {
            print_build_footer(total_start.elapsed());
        }
        Ok(())
    }

    /// Build, then rebuild whenever a watched file changes, until the watcher
    /// closes.
    ///
    /// # Errors
    ///
    /// Returns [`Exit::Failure`] when the file watcher can't start or a
    /// build's output can't be written.
    fn rebuild_on_change(&self, verbosity: Verbosity) -> Result<(), Exit> {
        let mut source_files = self.build_for_watch(verbosity)?;

        loop {
            let directories = self.watched_directories(&source_files);

            let (sender, receiver) = mpsc::channel();
            let mut debouncer =
                new_debouncer(Duration::from_millis(200), sender).map_err(|error| {
                    operation_error(format!("failed to create file watcher: {error}"))
                })?;

            for directory in &directories {
                if directory.exists()
                    && let Err(error) =
                        debouncer.watcher().watch(directory, RecursiveMode::Recursive)
                {
                    eprintln!("Warning: failed to watch `{}`: {error}", directory.display());
                }
            }

            eprintln!("\n[watching for changes...]");

            if !wait_for_change(&receiver) {
                return Ok(());
            }

            eprintln!();

            source_files = self.build_for_watch(verbosity)?;
        }
    }

    /// The directories to watch: each source file's, the config's, and, with
    /// neither, the working directory.
    fn watched_directories(&self, source_files: &[String]) -> Vec<PathBuf> {
        let config_path = self.config.as_deref();
        let mut directories: Vec<PathBuf> = source_files
            .iter()
            .filter_map(|file| native_path(file).parent().map(Path::to_path_buf))
            .collect();
        if let Some(parent) = config_path.and_then(Path::parent) {
            directories.push(parent.to_path_buf());
        }
        // Watch the discovered config too, or editing it never triggers a
        // rebuild.
        if config_path.is_none()
            && let Ok(working_directory) = env::current_dir()
            && let Ok(Some(discovered)) = discover_config(&working_directory)
        {
            directories.push(discovered.root);
        }
        // With no sources and no config to watch, such as before a luck.json
        // exists, watch the working directory so a fix still triggers a
        // rebuild.
        if directories.is_empty()
            && let Ok(working_directory) = env::current_dir()
        {
            directories.push(working_directory);
        }
        directories.sort_unstable();
        directories.dedup();
        directories
    }

    /// One watch-mode build. Returns every config's sources, for the next
    /// round to watch.
    ///
    /// # Errors
    ///
    /// Returns [`Exit::Failure`] when a build's output can't be written. A
    /// failed build or a bad config isn't an error here: watch mode reports it
    /// and keeps watching.
    fn build_for_watch(&self, verbosity: Verbosity) -> Result<Vec<String>, Exit> {
        let configs = match resolve_build_config(self.config.as_deref(), self.resolved_profile()) {
            Ok(configs) => configs,
            Err(error) => {
                eprintln!("Error: {error}");
                return Ok(Vec::new());
            }
        };

        let mut all_source_files = Vec::new();
        for config in &configs {
            match self.build_one(config, verbosity) {
                Ok(files) => all_source_files.extend(files),
                Err(BuildFailure::Failed(_)) => {}
                Err(BuildFailure::Fatal(exit)) => return Err(exit),
            }
        }

        Ok(all_source_files)
    }

    /// Bundle, minify if configured, and write one build config, or only say
    /// what would be written under `--dry-run`, which clap keeps out of watch
    /// mode. Returns its source files, which watch mode watches.
    ///
    /// # Errors
    ///
    /// Returns [`BuildFailure::Failed`] after rendering bundle or minify
    /// diagnostics, and [`BuildFailure::Fatal`] when the output can't be
    /// written.
    fn build_one(
        &self,
        config: &BuildConfig,
        verbosity: Verbosity,
    ) -> Result<Vec<String>, BuildFailure> {
        let start = Instant::now();
        let version = config.target.lua_version();

        let result = bundle(
            &config.entry,
            version,
            &config.search_paths,
            &config.project_root,
            config.dynamic_require,
        )
        .map_err(|errors| {
            print_diags(&errors);
            BuildFailure::Failed("build failed")
        })?;

        if verbosity != Verbosity::Quiet {
            print_diags(&result.warnings);
        }

        let source_files = result.source_files;
        let mut output = result.output;
        let pre_minify_size = output.len();

        if config.minify {
            let file_path = config.entry.display().to_string();
            output = luck_minifier::minify(&output, version, &config.transforms, &file_path)
                .map_err(|errors| {
                    print_document_diags(file_path, output, &errors);
                    BuildFailure::Failed("minification failed")
                })?;
        }

        let preamble = build_preamble(config);
        if !preamble.is_empty() {
            output = format!("{preamble}\n{output}");
        }

        if self.dry_run {
            eprintln!("[dry-run] Would write: {}", config.output.display());
        } else {
            write_output_file(&config.output, &output).map_err(BuildFailure::Fatal)?;
        }

        let duration = start.elapsed();
        if verbosity != Verbosity::Quiet {
            let original = config.minify.then_some(pre_minify_size);
            print_build_stat(&config.output, output.len(), original, duration);
        }

        Ok(source_files)
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
        let action = if config.minify { "Bundled & minified" } else { "Bundled" };
        let version = env!("CARGO_PKG_VERSION");
        lines.push(format!("-- {action} with luck v{version} // https://github.com/esoware/luck"));
    }

    lines.join("\n")
}

/// Blocks until a relevant file changes, then drains what's queued behind it.
/// Returns false once the watcher closes.
fn wait_for_change(receiver: &Receiver<DebounceEventResult>) -> bool {
    loop {
        match receiver.recv() {
            Ok(Ok(events)) => {
                if events.iter().any(|event| is_relevant_change(&event.path)) {
                    break;
                }
            }
            Ok(Err(error)) => eprintln!("watch error: {error}"),
            Err(_) => return false,
        }
    }
    while receiver.try_recv().is_ok() {}
    true
}

/// Only `.lua`, `.luau`, `.json`, and `.luaurc` changes trigger a rebuild, so
/// editor temp files and `.git` churn don't.
fn is_relevant_change(path: &Path) -> bool {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("lua" | "luau" | "json") => true,
        _ => path.file_name().is_some_and(|name| name == ".luaurc" || name == "luck.json"),
    }
}

fn format_duration(duration: Duration) -> String {
    let milliseconds = duration.as_millis();
    if milliseconds >= 1000 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{milliseconds}ms")
    }
}

fn relative_display(path: &Path) -> String {
    if let Ok(working_directory) = env::current_dir()
        && let Ok(relative) = path.strip_prefix(&working_directory)
    {
        return relative.display().to_string().replace('\\', "/");
    }
    path.display().to_string().replace('\\', "/")
}

fn print_build_header(config: &BuildConfig) {
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("\x1b[1;36mluck\x1b[0m \x1b[2mv{version}\x1b[0m\n");
    eprintln!("  \x1b[2mTarget:\x1b[0m  {}", config.target);
    eprintln!("  \x1b[2mEntry:\x1b[0m   {}", relative_display(&config.entry));
    if config.minify {
        eprintln!("  \x1b[2mMinify:\x1b[0m  \x1b[32menabled\x1b[0m");
    } else {
        eprintln!("  \x1b[2mMinify:\x1b[0m  \x1b[2mdisabled\x1b[0m");
    }
    eprintln!();
}

fn print_build_stat(
    output_path: &Path,
    output_size: usize,
    original_size: Option<usize>,
    duration: Duration,
) {
    let size = format_size(output_size);
    let path = relative_display(output_path);
    let time = format_duration(duration);

    if let Some(original) = original_size
        && original > 0
    {
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            clippy::float_arithmetic,
            reason = "display math, and its exact rounding is part of the output"
        )]
        let percent = ((original as f64 - output_size as f64) / original as f64 * 100.0f64) as i64;
        eprintln!(
            "  \x1b[1m{path}\x1b[0m  \x1b[2m{}\x1b[0m \x1b[2m→\x1b[0m \x1b[32m{size}\x1b[0m  \x1b[2m(-{percent}%)\x1b[0m  \x1b[2m{time}\x1b[0m",
            format_size(original),
        );
        return;
    }
    eprintln!("  \x1b[1m{path}\x1b[0m  \x1b[32m{size}\x1b[0m  \x1b[2m{time}\x1b[0m");
}

fn print_build_footer(total_duration: Duration) {
    eprintln!("\n  \x1b[32m✓\x1b[0m Done in \x1b[1m{}\x1b[0m", format_duration(total_duration));
}
