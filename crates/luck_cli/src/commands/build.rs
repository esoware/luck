use core::time::Duration;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use clap::Args;
use luck_bundler::{BundleOptions, bundle};
use luck_config::{
    BuildConfig, check_output_paths, config_directory, discover_config, load_or_discover_config,
    resolve_build_config,
};
use luck_resolver::{native_path, normalize_path};
use luck_token::{LuaVersion, chunk_start, has_shebang};
use notify_debouncer_mini::{DebounceEventResult, new_debouncer, notify::RecursiveMode};

use crate::exit::{Exit, operation_error, usage_error};
use crate::output::{format_size, write_output_file};
use crate::project::current_directory;
use crate::render::{print_diags, print_source_errors};
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
    #[arg(long, conflicts_with_all = ["profile", "dev"])]
    release: bool,

    /// Shorthand for --profile dev.
    #[arg(long, conflicts_with = "profile")]
    dev: bool,

    /// Watch source files and rebuild on changes.
    #[arg(short, long)]
    watch: bool,
}

enum BuildFailure {
    /// Bundling or minifying failed and the diagnostics are already rendered.
    /// Watch mode keeps going and watches `source_files`, since an edit to one
    /// of them may fix the build.
    Failed { message: &'static str, source_files: Vec<String> },
    /// The output can't be written. The run ends with this exit code, even in
    /// watch mode.
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

    fn resolved_profile(&self) -> Option<&str> {
        self.profile
            .as_deref()
            .or_else(|| self.release.then_some("release"))
            .or_else(|| self.dev.then_some("dev"))
    }

    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] when the working
    /// directory is unavailable, no `luck.json` is found or it fails to load,
    /// or the build settings are invalid.
    fn resolved_configs(&self) -> Result<Vec<BuildConfig>, Exit> {
        let working_directory = current_directory()?;
        let project = load_or_discover_config(self.config.as_deref(), &working_directory)
            .map_err(usage_error)?
            .ok_or_else(|| {
                usage_error(
                    "no `luck.json` found, so run from a project directory or use `--config`",
                )
            })?;
        resolve_build_config(&project, self.resolved_profile()).map_err(usage_error)
    }

    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config or an output that would replace
    /// a bundled file, and [`Exit::Failure`] when a build fails or its output
    /// can't be written.
    fn build_once(&self, verbosity: Verbosity) -> Result<(), Exit> {
        let total_start = Instant::now();
        let configs = self.resolved_configs()?;

        if verbosity != Verbosity::Quiet
            && let Some(first_config) = configs.first()
        {
            print_build_header(first_config);
        }

        for config in &configs {
            match self.build_one(config, verbosity) {
                Ok(_) => {}
                Err(BuildFailure::Failed { message, .. }) => {
                    eprintln!("\n  \x1b[31merror:\x1b[0m {message}");
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

    /// # Errors
    ///
    /// Returns [`Exit::Failure`] when the file watcher can't start, and the
    /// exit code when a build's output can't be written.
    fn rebuild_on_change(&self, verbosity: Verbosity) -> Result<(), Exit> {
        let mut source_files = self.build_for_watch(verbosity)?;

        loop {
            let directories = self.watched_directories(&source_files);

            let (sender, receiver) = mpsc::channel();
            let mut debouncer = new_debouncer(Duration::from_millis(200), sender)
                .map_err(|e| operation_error(format!("failed to create file watcher: {e}")))?;

            for directory in &directories {
                if directory.exists()
                    && let Err(error) =
                        debouncer.watcher().watch(directory, RecursiveMode::Recursive)
                {
                    eprintln!("warning: failed to watch `{}`: {error}", directory.display());
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

    fn watched_directories(&self, source_files: &[String]) -> Vec<PathBuf> {
        let config_path = self.config.as_deref();
        let mut directories: Vec<PathBuf> = source_files
            .iter()
            .filter_map(|file| native_path(file).parent().map(Path::to_path_buf))
            .collect();
        directories.extend(config_path.map(config_directory));
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

    /// # Errors
    ///
    /// Returns the exit code when a build's output can't be written. A failed
    /// build or a bad config isn't an error here: watch mode reports it and
    /// keeps watching.
    fn build_for_watch(&self, verbosity: Verbosity) -> Result<Vec<String>, Exit> {
        let Ok(configs) = self.resolved_configs() else {
            return Ok(Vec::new());
        };

        let mut all_source_files = Vec::new();
        for config in &configs {
            match self.build_one(config, verbosity) {
                Ok(source_files) | Err(BuildFailure::Failed { source_files, .. }) => {
                    all_source_files.extend(source_files);
                }
                Err(BuildFailure::Fatal(exit)) => return Err(exit),
            }
        }

        Ok(all_source_files)
    }

    /// # Errors
    ///
    /// Returns [`BuildFailure::Failed`] after rendering bundle or minify
    /// diagnostics, and [`BuildFailure::Fatal`] when the output can't be
    /// written or would replace a bundled file.
    fn build_one(
        &self,
        config: &BuildConfig,
        verbosity: Verbosity,
    ) -> Result<Vec<String>, BuildFailure> {
        let start = Instant::now();
        let version = config.target.lua_version();

        let result = bundle(
            &config.entry,
            &BundleOptions {
                version,
                search_paths: &config.search_paths,
                project_root: &config.project_root,
                dynamic_require: config.dynamic_require,
            },
        )
        .map_err(|errors| {
            print_diags(&errors, version);
            let source_files = errors
                .iter()
                .map(|diag| diag.file_path.clone())
                .chain([normalize_path(&config.entry)])
                .collect();
            BuildFailure::Failed { message: "build failed", source_files }
        })?;

        check_output_paths(&result.source_files, [&config.output])
            .map_err(|message| BuildFailure::Fatal(usage_error(message)))?;

        if verbosity != Verbosity::Quiet {
            print_diags(&result.warnings, version);
        }

        let source_files = result.source_files;
        let mut output = result.output;
        let pre_minify_size = output.len();

        if config.minify {
            output =
                luck_minifier::minify(&output, version, &config.transforms).map_err(|errors| {
                    print_source_errors(
                        config.entry.display().to_string(),
                        &output,
                        &errors,
                        version,
                    );
                    BuildFailure::Failed {
                        message: "minification failed",
                        source_files: source_files.clone(),
                    }
                })?;
        }

        output = with_preamble(config, output, version);

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

/// `output` with `config`'s preamble on top, after the BOM and shebang line a
/// loader only skips at the very start of a chunk.
fn with_preamble(config: &BuildConfig, output: String, version: LuaVersion) -> String {
    let mut lines = Vec::new();

    if let Some(text) = &config.preamble {
        for line in text.lines() {
            lines.push(format!("-- {line}"));
        }
    }

    if config.luck_preamble {
        let action = if config.minify { "Bundled & minified" } else { "Bundled" };
        let luck_version = env!("CARGO_PKG_VERSION");
        lines.push(format!(
            "-- {action} with luck v{luck_version} // https://github.com/esoware/luck"
        ));
    }

    if lines.is_empty() {
        return output;
    }
    let preamble = lines.join("\n");

    let (bom, chunk) = output.split_at(chunk_start(&output, version));
    if !has_shebang(chunk.as_bytes(), version) {
        return format!("{bom}{preamble}\n{chunk}");
    }
    let (shebang, body) = chunk.split_once('\n').unwrap_or((chunk, ""));
    format!("{bom}{shebang}\n{preamble}\n{body}")
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
        let percent = ((output_size as f64 - original as f64) / original as f64 * 100.0f64) as i64;
        eprintln!(
            "  \x1b[1m{path}\x1b[0m  \x1b[2m{}\x1b[0m \x1b[2m->\x1b[0m \x1b[32m{size}\x1b[0m  \x1b[2m({percent:+}%)\x1b[0m  \x1b[2m{time}\x1b[0m",
            format_size(original),
        );
        return;
    }
    eprintln!("  \x1b[1m{path}\x1b[0m  \x1b[32m{size}\x1b[0m  \x1b[2m{time}\x1b[0m");
}

fn print_build_footer(total_duration: Duration) {
    eprintln!("\n  \x1b[32mdone in\x1b[0m \x1b[1m{}\x1b[0m", format_duration(total_duration));
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn watches_the_current_directory_for_a_bare_config_path() {
        let args = BuildArgs {
            config: Some(PathBuf::from("luck.json")),
            dry_run: false,
            profile: None,
            release: false,
            dev: false,
            watch: true,
        };
        assert_eq!(args.watched_directories(&[]), [PathBuf::from(".")]);
    }

    #[test]
    fn watch_keeps_watching_a_failed_entry_outside_the_config_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let project = directory.path().join("project");
        let sources = directory.path().join("sources");
        fs::create_dir_all(&project).expect("mkdir project");
        fs::create_dir_all(&sources).expect("mkdir sources");
        fs::write(sources.join("main.lua"), "local =\n").expect("write entry");
        let config = project.join("luck.json");
        fs::write(&config, r#"{"entry":"../sources/main.lua","output":"bundle.lua"}"#)
            .expect("write luck.json");
        let args = BuildArgs {
            config: Some(config),
            dry_run: false,
            profile: None,
            release: false,
            dev: false,
            watch: true,
        };

        let source_files = args.build_for_watch(Verbosity::Quiet).expect("watch keeps going");
        let directories = args.watched_directories(&source_files);

        let canonical_sources = sources.canonicalize().expect("canonicalize sources");
        assert!(
            directories
                .iter()
                .any(|watched| watched.canonicalize().is_ok_and(|path| path == canonical_sources)),
            "fixing the broken entry must trigger a rebuild, but only {directories:?} are watched"
        );
    }
}
