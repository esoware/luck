use core::time::Duration;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use clap::Args;
use luck_allocator::ThreadAllocators;
use luck_bundler::{bundle, bundle_minified};
use luck_config::{BuildConfig, Project, check_output_paths, find_config};
use luck_diagnostics::FileDiagnostics;
use luck_token::{LuaVersion, chunk_start, has_shebang};
use notify_debouncer_mini::{DebounceEventResult, new_debouncer, notify::RecursiveMode};

use crate::exit::{Exit, config_error, operation_error, usage_error};
use crate::output::{format_size, write_output_file};
use crate::project::{current_directory, load_project};
use crate::reporter::print_files;
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
    /// Bundling failed and the diagnostics are already rendered. Watch mode
    /// keeps going and watches `source_files`, since an edit to one of them
    /// may fix the build.
    Failed { source_files: Vec<PathBuf> },
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

    /// `-c`, or else the first `luck.json` found walking up from cwd.
    ///
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] when the working
    /// directory is unavailable or no `luck.json` is found.
    fn config_path(&self) -> Result<PathBuf, Exit> {
        match &self.config {
            Some(path) => Ok(path.clone()),
            None => find_config(&current_directory()?).ok_or_else(|| {
                usage_error(
                    "no `luck.json` found, so run from a project directory or use `--config`",
                )
            }),
        }
    }

    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config or an output that would replace
    /// a bundled file, and [`Exit::Failure`] when a build fails or its output
    /// can't be written.
    fn build_once(&self, verbosity: Verbosity) -> Result<(), Exit> {
        let total_start = Instant::now();
        let project = load_project(&self.config_path()?)?;
        let builds = project.builds(self.resolved_profile()).map_err(usage_error)?;

        if verbosity != Verbosity::Quiet
            && let Some(first_build) = builds.first()
        {
            print_build_header(first_build);
        }

        let mut arenas = ThreadAllocators::new();
        for build in &builds {
            match self.build_one(&mut arenas, &project, build, verbosity) {
                Ok(_) => {}
                Err(BuildFailure::Failed { .. }) => {
                    eprintln!("\n  \x1b[31merror:\x1b[0m build failed");
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
        let mut arenas = ThreadAllocators::new();
        let mut read_files = self.build_for_watch(&mut arenas, verbosity)?;

        loop {
            let directories = watched_directories(&read_files);

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

            read_files = self.build_for_watch(&mut arenas, verbosity)?;
        }
    }

    /// Every file the builds read, config files included, so an edit to any
    /// of them triggers a rebuild. A config that doesn't load counts as read,
    /// with the file at fault, since fixing either fixes the build.
    ///
    /// # Errors
    ///
    /// Returns the exit code when a build's output can't be written. A failed
    /// build or a bad config isn't an error here: watch mode reports it and
    /// keeps watching.
    fn build_for_watch(
        &self,
        arenas: &mut ThreadAllocators,
        verbosity: Verbosity,
    ) -> Result<Vec<PathBuf>, Exit> {
        let Ok(config_path) = self.config_path() else {
            return Ok(Vec::new());
        };
        let project = match Project::load(&config_path) {
            Ok(project) => project,
            Err(error) => {
                config_error(&error);
                return Ok(vec![config_path, error.path]);
            }
        };
        let mut read_files = project.files().to_vec();
        let builds = match project.builds(self.resolved_profile()) {
            Ok(builds) => builds,
            Err(message) => {
                usage_error(message);
                return Ok(read_files);
            }
        };
        for build in &builds {
            match self.build_one(arenas, &project, build, verbosity) {
                Ok(source_files) | Err(BuildFailure::Failed { source_files }) => {
                    read_files.extend(source_files);
                }
                Err(BuildFailure::Fatal(exit)) => return Err(exit),
            }
        }
        Ok(read_files)
    }

    /// # Errors
    ///
    /// Returns [`BuildFailure::Failed`] after rendering bundle diagnostics,
    /// and [`BuildFailure::Fatal`] when the output can't be written or would
    /// replace a bundled file.
    fn build_one(
        &self,
        arenas: &mut ThreadAllocators,
        project: &Project,
        config: &BuildConfig,
        verbosity: Verbosity,
    ) -> Result<Vec<PathBuf>, BuildFailure> {
        let start = Instant::now();
        // The arenas outlive each build, so a rebuild reuses their memory.
        arenas.reset();
        let version = config.target.version();
        let options = project.bundle_options(version);
        let bundle_failure = |errors: Vec<FileDiagnostics>| {
            print_files(&errors, version);
            let source_files =
                errors.into_iter().map(|file| file.path).chain([config.entry.clone()]).collect();
            BuildFailure::Failed { source_files }
        };
        let check_output = |source_files: &[PathBuf]| {
            check_output_paths(source_files, [&config.output])
                .map_err(|message| BuildFailure::Fatal(usage_error(message)))
        };

        // A minified build compares its size with the sources', since it
        // never writes the bundle unminified.
        let (bundled, source_files, original_size) = if config.minify {
            let minified = bundle_minified(arenas, &config.entry, &options, &config.transforms)
                .map_err(bundle_failure)?;
            check_output(&minified.source_files)?;
            if verbosity != Verbosity::Quiet {
                print_files(&minified.warnings, version);
            }
            (minified.output, minified.source_files, Some(minified.source_byte_count))
        } else {
            let result = bundle(arenas, &config.entry, &options).map_err(bundle_failure)?;
            check_output(&result.source_files)?;
            if verbosity != Verbosity::Quiet {
                print_files(&result.warnings, version);
            }
            (result.output, result.source_files, None)
        };

        let output = with_preamble(config, bundled, version);

        if self.dry_run {
            eprintln!("[dry-run] Would write: {}", config.output.display());
        } else {
            write_output_file(&config.output, &output).map_err(BuildFailure::Fatal)?;
        }

        let duration = start.elapsed();
        if verbosity != Verbosity::Quiet {
            print_build_stat(&config.output, output.len(), original_size, duration);
        }

        Ok(source_files)
    }
}

/// The directories holding `files`, or with none, the working directory, so
/// a `luck.json` written there still triggers a rebuild.
fn watched_directories(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = files
        .iter()
        .filter_map(|file| file.parent())
        // A bare file name's parent is empty, which can't be watched.
        .map(|parent| if parent.as_os_str().is_empty() { Path::new(".") } else { parent })
        .map(Path::to_path_buf)
        .collect();
    if directories.is_empty()
        && let Ok(working_directory) = env::current_dir()
    {
        directories.push(working_directory);
    }
    directories.sort_unstable();
    directories.dedup();
    directories
}

/// `output` with `config`'s preamble on top, after the BOM and shebang line a
/// loader only skips at the very start of a chunk.
fn with_preamble(config: &BuildConfig, output: Vec<u8>, version: LuaVersion) -> Vec<u8> {
    let mut lines = Vec::new();

    if let Some(text) = &config.preamble {
        // A short comment ends at a CR as well as at an LF, on every target.
        for line in text.lines().flat_map(|line| line.split('\r')) {
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
    if !has_shebang(chunk, version) {
        return [bom, preamble.as_bytes(), b"\n", chunk].concat();
    }
    let shebang_length = chunk.iter().position(|&b| b == b'\n').unwrap_or(chunk.len());
    let (shebang, from_line_break) = chunk.split_at(shebang_length);
    let body = from_line_break.get(1..).unwrap_or_default();
    [bom, shebang, b"\n", preamble.as_bytes(), b"\n", body].concat()
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

    use luck_resolver::{FileSystem as _, FileSystemOs};

    use super::*;

    fn watching(config: PathBuf) -> BuildArgs {
        BuildArgs {
            config: Some(config),
            dry_run: false,
            profile: None,
            release: false,
            dev: false,
            watch: true,
        }
    }

    /// The directories watched for what `args` builds, canonical.
    fn watched_for(args: &BuildArgs) -> Vec<PathBuf> {
        let read_files = args
            .build_for_watch(&mut ThreadAllocators::new(), Verbosity::Quiet)
            .expect("watch goes on");
        watched_directories(&read_files)
            .iter()
            .map(|directory| FileSystemOs.canonicalize_existing(directory))
            .collect()
    }

    #[test]
    fn watches_the_current_directory_for_a_bare_config_path() {
        assert_eq!(watched_directories(&[PathBuf::from("luck.json")]), [PathBuf::from(".")]);
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

        let directories = watched_for(&watching(config));
        let canonical_sources = FileSystemOs.canonicalize_existing(&sources);
        assert!(
            directories.contains(&canonical_sources),
            "fixing the broken entry must trigger a rebuild, but only {directories:?} are watched"
        );
    }

    /// Editing a file the config extends changes the build as much as editing
    /// the config does, and so does fixing a config that doesn't load.
    #[test]
    fn watch_keeps_watching_every_config_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        let shared = directory.path().join("shared");
        let project = directory.path().join("project");
        fs::create_dir_all(&shared).expect("mkdir shared");
        fs::create_dir_all(&project).expect("mkdir project");
        fs::write(shared.join("base.json"), "{}").expect("write base.json");
        fs::write(project.join("main.lua"), "return 1\n").expect("write entry");
        let config = project.join("luck.json");
        let extending =
            r#"{"extends":["../shared/base.json"],"entry":"main.lua","output":"out.lua"}"#;
        fs::write(&config, extending).expect("write luck.json");
        let canonical_shared = FileSystemOs.canonicalize_existing(&shared);

        let loading = watched_for(&watching(config.clone()));
        assert!(loading.contains(&canonical_shared), "{loading:?}");

        fs::write(shared.join("base.json"), "{ broken").expect("break base.json");
        let broken = watched_for(&watching(config));
        assert!(broken.contains(&canonical_shared), "{broken:?}");
    }
}
