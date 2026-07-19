use crate::render::{FileCache, render_diagnostics, render_diagnostics_to_buffer};
use clap::{Args, Parser, Subcommand, ValueEnum};
use luck_bundler::bundle;
use luck_core::TransformConfig;
use luck_core::config::{BuildConfig, resolve_build_config};
use luck_core::diagnostics::Diagnostic;
use luck_core::types::LuaTarget;
use notify_debouncer_mini::new_debouncer;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// Exit codes. 0 = success; 1 = the operation ran but found problems
/// (diagnostics, lint findings, parse/build failure); 2 = usage/config error
/// (bad args, missing/invalid config, path not found).
pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_USAGE: u8 = 2;

/// Controls how much output the CLI prints during builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Quiet,
    Normal,
}

#[derive(Parser)]
#[command(
    name = "luck",
    version,
    about = "Zero-runtime Lua/Luau bundler",
    long_about = "Bundles multi-file Lua/Luau projects into a single output file using IIFEs.\nNo injected runtime — just your code.",
    after_help = "Examples:\n  luck init\n  luck build\n  luck bundle src/main.lua -t 54 -o dist/bundle.lua\n  luck minify input.luau -o output.luau\n  luck graph src/main.lua -t 54 --format dot"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new project with luck.json
    Init {
        /// Lua target [default: Lua54]
        #[arg(short = 't', long = "target", value_name = "TARGET")]
        target: Option<String>,
    },

    /// Build project using luck.json config
    Build {
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
    },

    /// Bundle a multi-file project into a single file
    Bundle {
        /// Entry file
        entry: String,

        /// Lua target [default: inferred from entry extension]
        #[arg(short = 't', long = "target", value_name = "TARGET")]
        target: Option<String>,

        /// Output file [default: stdout]
        #[arg(short, long, value_name = "PATH")]
        output: Option<String>,

        /// Search path template (repeatable)
        #[arg(short = 's', long = "search-path", value_name = "PATTERN")]
        search_path: Vec<String>,

        /// Minify the output
        #[arg(long)]
        minify: bool,

        /// Write a JSON line map (bundle lines -> source files) to PATH.
        /// Incompatible with --minify, which rewrites line structure.
        #[arg(long = "line-map", value_name = "PATH", conflicts_with = "minify")]
        line_map: Option<PathBuf>,

        #[command(flatten)]
        minify_flags: MinifyFlags,
    },

    /// Minify a source file
    Minify {
        /// Input file
        input: String,

        /// Lua target [default: inferred from input extension]
        #[arg(short = 't', long = "target", value_name = "TARGET")]
        target: Option<String>,

        /// Output file [default: stdout]
        #[arg(short, long, value_name = "PATH")]
        output: Option<String>,

        /// Print size statistics to stderr
        #[arg(long)]
        stats: bool,

        #[command(flatten)]
        minify_flags: MinifyFlags,
    },

    /// Show the dependency graph
    Graph {
        /// Entry file
        entry: String,

        /// Lua target [default: inferred from entry extension]
        #[arg(short = 't', long = "target", value_name = "TARGET")]
        target: Option<String>,

        /// Search path template (repeatable)
        #[arg(short = 's', long = "search-path", value_name = "PATTERN")]
        search_path: Vec<String>,

        /// Output format
        #[arg(long, value_enum, default_value_t = GraphFormat::Json)]
        format: GraphFormat,
    },

    /// Lint Lua/Luau source files (config-driven, oxlint-style)
    Lint {
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
    },

    /// Format Lua/Luau source files (config-driven, oxfmt-style)
    Fmt {
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
    },

    /// Parse and check Lua/Luau source files for errors (config-driven)
    Check {
        /// Files or directories to check [default: current directory]
        paths: Vec<String>,

        /// Path to config file [default: discover luck.json upward from cwd]
        #[arg(short, long, value_name = "PATH")]
        config: Option<PathBuf>,
    },

    /// Run the language server (LSP) over stdio, or TCP with --socket.
    Lsp {
        /// Bind 127.0.0.1:<port> and accept one client instead of stdio.
        #[arg(long)]
        socket: Option<u16>,
    },
}

#[derive(Clone, ValueEnum)]
enum GraphFormat {
    Json,
    Dot,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LintFormat {
    Default,
    Json,
}

fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn format_duration(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms >= 1000 {
        format!("{:.2}s", d.as_secs_f64())
    } else {
        format!("{}ms", ms)
    }
}

fn relative_display(path: &std::path::Path) -> String {
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
    path: &std::path::Path,
    output_size: usize,
    original_size: Option<usize>,
    duration: std::time::Duration,
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

fn print_build_footer(total_duration: std::time::Duration) {
    eprintln!(
        "\n  \x1b[32m✓\x1b[0m Done in \x1b[1m{}\x1b[0m",
        format_duration(total_duration)
    );
}

/// Parses CLI arguments and dispatches to the appropriate subcommand,
/// returning the process exit code per the documented convention.
pub fn run() -> ExitCode {
    let cli = Cli::parse();

    let verbosity = if cli.quiet {
        Verbosity::Quiet
    } else {
        Verbosity::Normal
    };

    match cli.command {
        Command::Init { target } => run_init(target.as_deref()),
        Command::Build {
            config,
            dry_run,
            profile,
            release,
            dev,
            watch,
        } => {
            let profile = if let Some(name) = profile.as_deref() {
                Some(name)
            } else if release {
                Some("release")
            } else if dev {
                Some("dev")
            } else {
                None
            };
            if watch {
                run_build_watch(config.as_deref(), profile, verbosity)
            } else {
                run_build(config.as_deref(), dry_run, profile, verbosity)
            }
        }
        Command::Bundle {
            entry,
            target,
            output,
            search_path,
            minify,
            line_map,
            minify_flags,
        } => {
            let resolved = resolve_explicit_target(target.as_deref(), &entry);
            let config = build_transform_config(&minify_flags);
            run_bundle(
                resolved,
                &entry,
                output.as_deref(),
                &search_path,
                minify.then_some(&config),
                line_map.as_deref(),
                verbosity,
            )
        }
        Command::Minify {
            input,
            target,
            output,
            stats,
            minify_flags,
        } => {
            let resolved = resolve_explicit_target(target.as_deref(), &input);
            let config = build_transform_config(&minify_flags);
            run_minify(
                resolved,
                &input,
                output.as_deref(),
                &config,
                stats,
                verbosity,
            )
        }
        Command::Graph {
            entry,
            target,
            search_path,
            format,
        } => {
            let resolved = resolve_explicit_target(target.as_deref(), &entry);
            run_graph(resolved, &entry, &search_path, format, verbosity)
        }
        Command::Lint {
            paths,
            config,
            allow,
            warn,
            deny,
            globals,
            fix,
            format,
            max_warnings,
            deny_warnings,
            silent,
            rules,
            print_config,
            stdin_filepath,
        } => run_lint(LintArgs {
            paths,
            config,
            allow,
            warn,
            deny,
            globals,
            fix,
            format,
            max_warnings,
            deny_warnings,
            silent,
            rules,
            print_config,
            stdin_filepath,
            verbosity,
        }),
        Command::Fmt {
            paths,
            write,
            check,
            list_different,
            config,
            no_editorconfig,
            stdin_filepath,
            range_start,
            range_end,
        } => run_fmt(FmtArgs {
            paths,
            write,
            check,
            list_different,
            config,
            no_editorconfig,
            stdin_filepath,
            range_start,
            range_end,
            verbosity,
        }),
        Command::Check { paths, config } => run_check(CheckArgs {
            paths,
            config,
            verbosity,
        }),
        Command::Lsp { socket } => run_lsp(socket),
    }
}

fn run_lsp(socket: Option<u16>) -> ExitCode {
    // Match the CLI's existing 16 MB worker-stack budget for deep parses.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("luck lsp: failed to start async runtime: {err}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    runtime.block_on(async {
        match socket {
            Some(port) => {
                if let Err(err) = luck_lsp::serve_socket(port).await {
                    eprintln!("luck lsp: socket transport failed: {err}");
                    return ExitCode::from(EXIT_FAILURE);
                }
                ExitCode::from(EXIT_SUCCESS)
            }
            None => {
                luck_lsp::serve_stdio().await;
                ExitCode::from(EXIT_SUCCESS)
            }
        }
    })
}

/// Build the scaffolded `luck.json` contents for a fresh project.
///
/// The config model is per-extension: `.lua` files use the `lua` axis,
/// `.luau` files use the `luau` axis. A complete scaffold sets both. The
/// chosen target overrides its own axis (lowercased canonical, e.g. `lua53`
/// or `roblox`); the other axis keeps its default (`lua54` / `luau`). The
/// `entry` and `search_paths` follow the chosen target's family.
fn init_config_content(target: LuaTarget) -> String {
    let mut lua_value = "lua54".to_string();
    let mut luau_value = "luau".to_string();
    if target.is_luau() {
        luau_value = target.to_string().to_lowercase();
    } else {
        lua_value = target.to_string().to_lowercase();
    }

    let ext = if target.is_luau() { "luau" } else { "lua" };
    let search_paths_line = if target.is_luau() {
        String::new()
    } else {
        format!("\n    \"search_paths\": [\"src/?.{ext}\", \"src/?/init.{ext}\"],")
    };

    format!(
        r#"{{
    "lua": "{lua_value}",
    "luau": "{luau_value}",
    "entry": "src/main.{ext}",
    "output_dir": "dist",{search_paths_line}
    "minify": false,
    "profiles": {{
        "dev": {{
            "minify": false
        }},
        "release": {{
            "minify": true
        }}
    }}
}}
"#
    )
}

fn run_init(target: Option<&str>) -> ExitCode {
    let target: LuaTarget = match target {
        Some(target) => match target.parse() {
            Ok(target) => target,
            Err(e) => {
                eprintln!("Error: {e}");
                return ExitCode::from(EXIT_USAGE);
            }
        },
        None => LuaTarget::Lua54,
    };

    let config_path = PathBuf::from("luck.json");
    if config_path.exists() {
        eprintln!("Error: luck.json already exists");
        return ExitCode::from(EXIT_FAILURE);
    }

    // The scaffolded entry file and search paths follow the chosen target's
    // family; `.luau` for any Luau dialect, `.lua` otherwise.
    let ext = if target.is_luau() { "luau" } else { "lua" };
    let content = init_config_content(target);

    if let Err(e) = std::fs::write(&config_path, &content) {
        eprintln!("Error writing luck.json: {e}");
        return ExitCode::from(EXIT_FAILURE);
    }

    let src_dir = PathBuf::from("src");
    if !src_dir.exists()
        && let Err(e) = std::fs::create_dir_all(&src_dir)
    {
        eprintln!("Error creating src directory: {e}");
        return ExitCode::from(EXIT_FAILURE);
    }

    let entry_file = src_dir.join(format!("main.{ext}"));
    if !entry_file.exists()
        && let Err(e) = std::fs::write(&entry_file, "print(\"hello world\")\n")
    {
        eprintln!("Error writing {}: {e}", entry_file.display());
        return ExitCode::from(EXIT_FAILURE);
    }

    eprintln!("Created luck.json and src/main.{ext}");
    ExitCode::from(EXIT_SUCCESS)
}

fn run_build(
    config_path: Option<&Path>,
    dry_run: bool,
    profile: Option<&str>,
    verbosity: Verbosity,
) -> ExitCode {
    let total_start = Instant::now();
    let configs = match resolve_build_config(config_path, profile) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    if verbosity != Verbosity::Quiet {
        print_build_header(&configs[0]);
    }

    for config in &configs {
        if let Err(msg) = build_one(config, dry_run, verbosity) {
            eprintln!("\n  \x1b[31m✗\x1b[0m {msg}");
            return ExitCode::from(EXIT_FAILURE);
        }
    }

    if verbosity != Verbosity::Quiet {
        print_build_footer(total_start.elapsed());
    }
    ExitCode::from(EXIT_SUCCESS)
}

/// Returns source file paths on success.
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
                write_output_file(&config.output, &output);
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
    let config_path_owned = config_path.map(|p| p.to_path_buf());

    let mut watched_paths =
        run_build_collect_paths(config_path_owned.as_deref(), profile, verbosity);

    loop {
        let mut dirs: HashSet<PathBuf> = HashSet::new();
        for file in &watched_paths {
            let p = PathBuf::from(file.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = p.parent() {
                dirs.insert(parent.to_path_buf());
            }
        }
        if let Some(cp) = &config_path_owned
            && let Some(parent) = cp.parent()
        {
            dirs.insert(parent.to_path_buf());
        }
        // The DISCOVERED config (no -c) must be watched too, or config
        // edits never retrigger a rebuild.
        if config_path_owned.is_none()
            && let Ok(cwd) = std::env::current_dir()
            && let Ok(Some((discovered_path, _))) = luck_core::config::discover_config(&cwd)
            && let Some(parent) = discovered_path.parent()
        {
            dirs.insert(parent.to_path_buf());
        }
        // A failed first build used to leave NOTHING watched: the session
        // sat on `[watching for changes...]` forever with no way to
        // recover. Fall back to watching the working directory.
        if dirs.is_empty()
            && let Ok(cwd) = std::env::current_dir()
        {
            dirs.insert(cwd);
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = match new_debouncer(Duration::from_millis(200), tx) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error: failed to create file watcher: {e}");
                return ExitCode::from(EXIT_FAILURE);
            }
        };

        for dir in &dirs {
            if dir.exists()
                && let Err(e) = debouncer
                    .watcher()
                    .watch(dir, notify::RecursiveMode::Recursive)
            {
                eprintln!("Warning: failed to watch {}: {e}", dir.display());
            }
        }

        eprintln!("\n[watching for changes...]");

        // Only source/config changes trigger a rebuild - editor temp
        // files and .git churn used to retrigger constantly.
        let is_relevant = |path: &Path| -> bool {
            match path.extension().and_then(|ext| ext.to_str()) {
                Some("lua" | "luau" | "json") => true,
                _ => path
                    .file_name()
                    .is_some_and(|name| name == ".luaurc" || name == "luck.json"),
            }
        };

        loop {
            match rx.recv() {
                Ok(Ok(events)) => {
                    if events.iter().any(|event| is_relevant(&event.path)) {
                        break;
                    }
                }
                Ok(Err(errs)) => {
                    eprintln!("Watch error: {errs}");
                }
                Err(_) => return ExitCode::from(EXIT_SUCCESS), // channel closed
            }
        }

        while rx.try_recv().is_ok() {}

        eprintln!();

        watched_paths = run_build_collect_paths(config_path_owned.as_deref(), profile, verbosity);
    }
    // Unreachable: the loop only exits via the channel-closed return
    // above, which is treated as a clean shutdown.
}

fn run_build_collect_paths(
    config_path: Option<&Path>,
    profile: Option<&str>,
    verbosity: Verbosity,
) -> Vec<String> {
    // Watch mode rebuilds on the same worker thread; without this,
    // .luaurc alias edits are invisible until process restart.
    luck_resolver::clear_luaurc_cache();

    let configs = match resolve_build_config(config_path, profile) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return Vec::new();
        }
    };

    let mut all_source_files = Vec::new();
    for config in &configs {
        if let Ok(files) = build_one(config, false, verbosity) {
            all_source_files.extend(files);
        }
    }

    if let Some(cp) = config_path {
        all_source_files.push(cp.display().to_string());
    }

    all_source_files
}

/// CLI flags that disable individual minifier transforms.
#[derive(Args, Clone)]
struct MinifyFlags {
    #[arg(long)]
    no_remove_dead_code: bool,
    #[arg(long)]
    no_simplify_statements: bool,
    #[arg(long)]
    no_fold_constants: bool,
    #[arg(long)]
    no_inline_locals: bool,
    #[arg(long)]
    no_merge_locals: bool,
    #[arg(long)]
    no_simplify_indexes: bool,
    #[arg(long)]
    no_shorten_strings: bool,
    #[arg(long)]
    no_shorten_numbers: bool,
    #[arg(long)]
    no_simplify_parens: bool,
    #[arg(long)]
    no_rename_locals: bool,
    #[arg(long)]
    no_lift_locals: bool,
    /// Rename globals defined in this file (breaks cross-chunk consumers
    /// that expect the original _G keys; off unless the script is fully
    /// self-contained).
    #[arg(long)]
    rename_globals: bool,
}

fn build_transform_config(flags: &MinifyFlags) -> TransformConfig {
    TransformConfig {
        remove_dead_code: !flags.no_remove_dead_code,
        simplify_statements: !flags.no_simplify_statements,
        fold_constants: !flags.no_fold_constants,
        inline_locals: !flags.no_inline_locals,
        merge_locals: !flags.no_merge_locals,
        simplify_indexes: !flags.no_simplify_indexes,
        shorten_strings: !flags.no_shorten_strings,
        shorten_numbers: !flags.no_shorten_numbers,
        simplify_parens: !flags.no_simplify_parens,
        rename_locals: !flags.no_rename_locals,
        lift_locals: !flags.no_lift_locals,
        rename_globals: flags.rename_globals,
    }
}

fn fail_with_diagnostics(errors: &[Diagnostic], source: Option<(&str, &str)>) -> ! {
    let mut cache = build_file_cache(errors);
    if let Some((path, src)) = source {
        cache.add_file(path.to_string(), src.to_string());
    }
    render_diagnostics(errors, &mut cache);
    process::exit(EXIT_FAILURE as i32);
}

/// Resolve the target for the one-shot `bundle`/`minify`/`graph` commands.
///
/// An explicit `-t/--target` is parsed via the alias-rich `FromStr`; a bad
/// value exits with code 2. When omitted, the target is inferred from the
/// primary input file's extension: `.luau` maps to Luau, everything else to
/// Lua 5.4.
fn resolve_explicit_target(target: Option<&str>, input_path: &str) -> LuaTarget {
    if let Some(target_str) = target {
        return target_str.parse::<LuaTarget>().unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(EXIT_USAGE as i32);
        });
    }

    if Path::new(input_path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("luau"))
    {
        LuaTarget::Luau
    } else {
        LuaTarget::Lua54
    }
}

/// Parsed arguments for the top-level `luck lint` command.
struct LintArgs {
    paths: Vec<String>,
    config: Option<PathBuf>,
    allow: Vec<String>,
    warn: Vec<String>,
    deny: Vec<String>,
    globals: Vec<String>,
    fix: bool,
    format: LintFormat,
    max_warnings: Option<usize>,
    deny_warnings: bool,
    silent: bool,
    rules: bool,
    print_config: bool,
    stdin_filepath: Option<String>,
    verbosity: Verbosity,
}

/// Map an `-A`/`-W`/`-D` name to a lint category, if it names one.
fn category_from_name(name: &str) -> Option<luck_linter::diagnostic::Category> {
    use luck_linter::diagnostic::Category;
    match name.to_ascii_lowercase().as_str() {
        "correctness" => Some(Category::Correctness),
        "suspicious" => Some(Category::Suspicious),
        "style" => Some(Category::Style),
        "performance" | "perf" => Some(Category::Performance),
        _ => None,
    }
}

fn category_label(category: luck_linter::diagnostic::Category) -> &'static str {
    use luck_linter::diagnostic::Category;
    match category {
        Category::Correctness => "correctness",
        Category::Suspicious => "suspicious",
        Category::Style => "style",
        Category::Performance => "performance",
    }
}

/// Resolve the project config for the path-oriented subcommands: an explicit
/// `-c` (via extends) or upward discovery from cwd, else defaults. Exits with
/// EXIT_USAGE on error. Returns the config and the directory that roots
/// include/exclude globs.
fn resolve_project_config(config: Option<&Path>) -> (luck_core::config::LuckConfig, PathBuf) {
    use luck_core::config::{LuckConfig, discover_config, load_with_extends};

    let cwd = current_dir_or_exit();

    if let Some(path) = config {
        match load_with_extends(path) {
            Ok(config) => {
                let dir = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| cwd.clone());
                (config, dir)
            }
            Err(message) => {
                eprintln!("Error: {message}");
                process::exit(EXIT_USAGE as i32);
            }
        }
    } else {
        match discover_config(&cwd) {
            Ok(Some((path, config))) => {
                let dir = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| cwd.clone());
                (config, dir)
            }
            Ok(None) => (LuckConfig::default(), cwd.clone()),
            Err(message) => {
                eprintln!("Error: {message}");
                process::exit(EXIT_USAGE as i32);
            }
        }
    }
}

/// Expand the user's path arguments into the set of files to process.
/// Empty args default to the current directory. Directories are walked and
/// gated by the project filter; explicit file args are included
/// unconditionally. Exits EXIT_USAGE if a path does not exist.
fn collect_target_files(
    paths: &[String],
    filter: &luck_core::config::ProjectFilter,
) -> Vec<PathBuf> {
    let paths: Vec<String> = if paths.is_empty() {
        vec![".".to_string()]
    } else {
        paths.to_vec()
    };

    let mut files: Vec<PathBuf> = Vec::new();
    for raw in &paths {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            files.extend(collect_lua_files(&path, filter));
        } else if path.is_file() {
            files.push(path);
        } else {
            eprintln!("Error: path not found: {raw}");
            process::exit(EXIT_USAGE as i32);
        }
    }
    files
}

/// Resolve the effective `LintConfig` from the project config plus CLI
/// overrides, and return it alongside the resolved `LuckConfig` (for target
/// selection and include/exclude globs) and the config directory.
fn resolve_lint_config(
    args: &LintArgs,
) -> (
    luck_linter::LintConfig,
    luck_core::config::LuckConfig,
    PathBuf,
) {
    use luck_linter::RuleSetting;
    use luck_linter::diagnostic::Severity;

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

/// Implements the top-level, config-driven `luck lint` command.
fn run_lint(args: LintArgs) -> ExitCode {
    use luck_linter::diagnostic::Severity;

    let (lint_config, luck_config, config_dir) = resolve_lint_config(&args);

    if args.rules {
        print_lint_rules();
        return ExitCode::from(EXIT_SUCCESS);
    }

    if args.print_config {
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
    if let Some(stdin_path) = args.stdin_filepath.as_deref() {
        return run_lint_stdin(stdin_path, &args, &lint_config, target_for);
    }

    // Enumerate files. Explicit file args lint unconditionally; directory
    // walks honor the project's include/exclude globs.
    let filter = luck_core::config::ProjectFilter::new(
        &config_dir,
        &luck_config.include,
        &luck_config.exclude,
    )
    .unwrap_or_else(|error| {
        eprintln!("Error: {error}");
        std::process::exit(2);
    });
    let files = collect_target_files(&args.paths, &filter);

    // Files lint in parallel; each worker renders its diagnostics into
    // buffers which are then flushed IN INPUT ORDER, so output is
    // byte-identical to the sequential loop.
    struct LintOutcome {
        errors: u32,
        warnings: u32,
        fixed: bool,
        stderr_notes: Vec<String>,
        stderr_render: Vec<u8>,
        stdout_lines: String,
    }

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
                            // A skipped file must fail the run - CI going green while
                            // files silently went unlinted is worse than a hard stop.
                            outcome.errors += 1;
                            return outcome;
                        }
                    };

                    let target = target_for(file_path);
                    let mut diagnostics = luck_linter::lint_target(&source, target, &lint_config);
                    let mut current_source = source;

                    if args.fix && diagnostics.iter().any(|diag| diag.fix.is_some()) {
                        let fixed_source = luck_linter::apply_fixes(
                            &current_source,
                            &diagnostics,
                            target.lua_version(),
                        );
                        if fixed_source != current_source {
                            if let Err(error) = std::fs::write(file_path, &fixed_source) {
                                outcome.stderr_notes.push(format!(
                                    "Error: cannot write {}: {error}",
                                    file_path.display()
                                ));
                            } else {
                                outcome.fixed = true;
                                diagnostics =
                                    luck_linter::lint_target(&fixed_source, target, &lint_config);
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

                    if !diagnostics.is_empty() && !args.silent {
                        let file_label = file_path.to_string_lossy().to_string();
                        let (render, lines) = render_lint_diagnostics(
                            args.format,
                            &file_label,
                            current_source,
                            &diagnostics,
                        );
                        outcome.stderr_render = render;
                        outcome.stdout_lines = lines;
                    }
                    outcome
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

    if !args.silent && args.fix && total_fixed > 0 && args.verbosity == Verbosity::Normal {
        eprintln!("Fixed {total_fixed} file(s)");
    }

    if !args.silent
        && args.verbosity == Verbosity::Normal
        && (total_errors > 0 || total_warnings > 0)
    {
        eprintln!(
            "{total_errors} error(s), {total_warnings} warning(s) in {} file(s)",
            files.len()
        );
    }

    lint_exit_code(&args, total_errors, total_warnings)
}

/// Shared lint exit-code policy: any error fails; warnings fail only under
/// `--deny-warnings` or when they exceed `--max-warnings`.
fn lint_exit_code(args: &LintArgs, total_errors: u32, total_warnings: u32) -> ExitCode {
    let warnings_exceed = args
        .max_warnings
        .is_some_and(|max| total_warnings as usize > max);
    if total_errors > 0 || (args.deny_warnings && total_warnings > 0) || warnings_exceed {
        ExitCode::from(EXIT_FAILURE)
    } else {
        ExitCode::from(EXIT_SUCCESS)
    }
}

/// Lint a single document read from stdin, labeled and targeted by the
/// `--stdin-filepath` value. `--fix` writes the fixed source to stdout (never
/// to disk); diagnostics, formats, and the exit-code policy match the file
/// path.
fn run_lint_stdin(
    stdin_path: &str,
    args: &LintArgs,
    lint_config: &luck_linter::LintConfig,
    target_for: impl Fn(&Path) -> LuaTarget,
) -> ExitCode {
    use luck_linter::diagnostic::Severity;

    let source = match read_stdin_source() {
        Ok(text) => text,
        Err(code) => return code,
    };

    let target = target_for(&PathBuf::from(stdin_path));
    let mut diagnostics = luck_linter::lint_target(&source, target, lint_config);

    if args.fix {
        // `--fix` over stdin must ALWAYS echo the resulting buffer to stdout:
        // it is the editor's / pipeline's new file content. Emitting nothing on
        // a no-op would truncate `luck lint --fix --stdin-filepath f < f > f`.
        // Apply fixes when any exist, otherwise echo the original unchanged.
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

    if !diagnostics.is_empty() && !args.silent {
        let (render, lines) =
            render_lint_diagnostics(args.format, stdin_path, source, &diagnostics);
        if !render.is_empty() {
            use std::io::Write;
            let _ = std::io::stderr().write_all(&render);
        }
        if !lines.is_empty() {
            print!("{lines}");
        }
    }

    lint_exit_code(args, total_errors, total_warnings)
}

/// Renders one file's diagnostics: pretty output as stderr bytes,
/// JSON as stdout lines. Callers flush the buffers in input order.
fn render_lint_diagnostics(
    format: LintFormat,
    file_label: &str,
    source: String,
    diagnostics: &[luck_linter::diagnostic::LintDiagnostic],
) -> (Vec<u8>, String) {
    use luck_linter::diagnostic::Severity;
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

/// Parsed arguments for the top-level `luck fmt` command.
struct FmtArgs {
    paths: Vec<String>,
    write: bool,
    check: bool,
    list_different: bool,
    config: Option<PathBuf>,
    no_editorconfig: bool,
    stdin_filepath: Option<String>,
    range_start: Option<usize>,
    range_end: Option<usize>,
    verbosity: Verbosity,
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

/// Outcome of formatting one document, shared by the file-walk and stdin paths.
enum FormatOutcome {
    /// The source parsed and the formatted result is identical to the input.
    Unchanged,
    /// The source parsed and formatting produced different output.
    Changed(String),
    /// The source did not parse; carries the rendered diagnostics
    /// (buffered so parallel workers never interleave stderr).
    ParseError(Vec<u8>),
}

/// Format one in-memory document, rendering parse-error diagnostics under
/// `path_label` when the source does not parse. Does no IO of its own; callers
/// decide whether to write the result to disk or stdout.
fn format_document(
    source: &str,
    path_label: &str,
    file_path: &Path,
    target: LuaTarget,
    luck_config: &luck_core::config::LuckConfig,
    use_editorconfig: bool,
    range: Option<std::ops::Range<usize>>,
) -> FormatOutcome {
    // `.editorconfig` provides formatting defaults below luck.json; the
    // luck.json `format` section always wins.
    let resolved_format = luck_core::editorconfig::resolved_format_config(
        luck_config.format.as_ref(),
        file_path,
        use_editorconfig,
    );
    let options = luck_formatter::FormatOptions::from(&resolved_format);
    let result = match range {
        Some(range) => luck_formatter::format_range(source, target.lua_version(), &options, range),
        None => luck_formatter::format(source, target.lua_version(), &options),
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

/// Read the whole of stdin as UTF-8 text, mapping IO failure to an error
/// message plus `EXIT_FAILURE` (mirrors `run_minify`'s `-` handling).
fn read_stdin_source() -> Result<String, ExitCode> {
    use std::io::Read;
    let mut source = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut source) {
        eprintln!("Error: failed to read stdin: {error}");
        return Err(ExitCode::from(EXIT_FAILURE));
    }
    Ok(source)
}

/// Resolve the `LuckConfig` for `fmt` from `-c` (via extends) or upward
/// discovery from cwd, returning it alongside the config directory used to
/// root include/exclude globs.
/// Implements the top-level, config-driven `luck fmt` command (oxfmt-style).
fn run_fmt(args: FmtArgs) -> ExitCode {
    let (luck_config, config_dir) = resolve_project_config(args.config.as_deref());

    // Resolve the parse target lazily per file from the config dialects.
    let target_for = |path: &Path| -> LuaTarget {
        luck_config.target_for_path(path).unwrap_or_else(|message| {
            eprintln!("Error: {message}");
            process::exit(EXIT_USAGE as i32);
        })
    };

    // Stdin mode formats a single virtual document and writes the result to
    // stdout, ignoring positional paths (the editor "format stdin" contract).
    if let Some(stdin_path) = args.stdin_filepath.as_deref() {
        return run_fmt_stdin(stdin_path, &args, &luck_config, target_for);
    }

    // Range formatting is per-document: byte offsets are meaningless across
    // a directory walk or multiple files.
    let has_range = args.range_start.is_some() || args.range_end.is_some();
    if has_range && (args.paths.len() != 1 || !Path::new(&args.paths[0]).is_file()) {
        eprintln!(
            "Error: --range-start/--range-end require exactly one input file or --stdin-filepath"
        );
        return ExitCode::from(EXIT_USAGE);
    }

    // Explicit files format unconditionally; directory walks honor the
    // project's include/exclude globs.
    let filter = luck_core::config::ProjectFilter::new(
        &config_dir,
        &luck_config.include,
        &luck_config.exclude,
    )
    .unwrap_or_else(|error| {
        eprintln!("Error: {error}");
        std::process::exit(2);
    });
    let files = collect_target_files(&args.paths, &filter);

    // --check and --list-different only report; --write (or no mode flag at
    // all) writes in place. --write is the implicit default, so its presence
    // does not change behavior beyond documenting intent.
    let _ = args.write;
    let report_only = args.check || args.list_different;

    // Files format in parallel; per-file output is buffered and flushed
    // in input order so results match the sequential loop byte-for-byte.
    enum FmtOutcome {
        ReadError(String),
        RangeError(String),
        ParseError(Vec<u8>),
        Unchanged,
        Changed(String),
        WriteError(String),
        Written(String),
    }

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
            let outcomes: Vec<FmtOutcome> = chunk
                .par_iter()
                .map(|file_path| {
                    let source = match luck_core::source_io::read_source_file(file_path) {
                        Ok(text) => text,
                        Err(error) => {
                            return FmtOutcome::ReadError(format!(
                                "Error: cannot read {}: {error}",
                                file_path.display()
                            ));
                        }
                    };

                    let range = match resolve_format_range(
                        args.range_start,
                        args.range_end,
                        source.len(),
                    ) {
                        Ok(range) => range,
                        Err(message) => return FmtOutcome::RangeError(message),
                    };

                    let target = target_for(file_path);
                    let file_label = file_path.to_string_lossy().to_string();
                    let formatted = match format_document(
                        &source,
                        &file_label,
                        file_path,
                        target,
                        &luck_config,
                        !args.no_editorconfig,
                        range,
                    ) {
                        FormatOutcome::ParseError(rendered) => {
                            return FmtOutcome::ParseError(rendered);
                        }
                        FormatOutcome::Unchanged => return FmtOutcome::Unchanged,
                        FormatOutcome::Changed(output) => output,
                    };

                    let path_str = file_path.display().to_string();
                    if report_only {
                        FmtOutcome::Changed(path_str)
                    } else if let Err(error) = std::fs::write(file_path, &formatted) {
                        FmtOutcome::WriteError(format!("Error: cannot write {path_str}: {error}"))
                    } else {
                        FmtOutcome::Written(path_str)
                    }
                })
                .collect();

            for outcome in outcomes {
                match outcome {
                    FmtOutcome::ReadError(message) => {
                        let _ = writeln!(stderr, "{message}");
                        // A skipped file must fail the run, same as a parse error.
                        had_parse_error = true;
                    }
                    FmtOutcome::RangeError(message) => {
                        let _ = writeln!(stderr, "{message}");
                        return ExitCode::from(EXIT_USAGE);
                    }
                    FmtOutcome::ParseError(rendered) => {
                        let _ = stderr.write_all(&rendered);
                        had_parse_error = true;
                    }
                    FmtOutcome::Unchanged => {}
                    FmtOutcome::Changed(path_str) => changed.push(path_str),
                    FmtOutcome::WriteError(message) => {
                        let _ = writeln!(stderr, "{message}");
                        return ExitCode::from(EXIT_FAILURE);
                    }
                    FmtOutcome::Written(path_str) => {
                        if args.verbosity != Verbosity::Quiet {
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
        if args.check && !changed.is_empty() {
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
/// `--stdin-filepath` value. The standard mode writes the formatted result to
/// stdout; `--check`/`--list-different` only report (and `--check` sets a
/// non-zero exit when the document would change).
fn run_fmt_stdin(
    stdin_path: &str,
    args: &FmtArgs,
    luck_config: &luck_core::config::LuckConfig,
    target_for: impl Fn(&Path) -> LuaTarget,
) -> ExitCode {
    let source = match read_stdin_source() {
        Ok(text) => text,
        Err(code) => return code,
    };

    let range = match resolve_format_range(args.range_start, args.range_end, source.len()) {
        Ok(range) => range,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    let file_path = PathBuf::from(stdin_path);
    let target = target_for(&file_path);
    let report_only = args.check || args.list_different;

    match format_document(
        &source,
        stdin_path,
        &file_path,
        target,
        luck_config,
        !args.no_editorconfig,
        range,
    ) {
        FormatOutcome::ParseError(rendered) => {
            use std::io::Write;
            let _ = std::io::stderr().write_all(&rendered);
            ExitCode::from(EXIT_FAILURE)
        }
        FormatOutcome::Unchanged => {
            // Editors expect the (already-formatted) source echoed back; report
            // modes stay silent on an unchanged document.
            if !report_only {
                print!("{source}");
            }
            ExitCode::from(EXIT_SUCCESS)
        }
        FormatOutcome::Changed(output) => {
            if args.list_different {
                println!("{stdin_path}");
                ExitCode::from(EXIT_SUCCESS)
            } else if args.check {
                // --check prints nothing to stdout and signals the difference
                // through the exit code alone.
                ExitCode::from(EXIT_FAILURE)
            } else {
                print!("{output}");
                ExitCode::from(EXIT_SUCCESS)
            }
        }
    }
}

/// Parsed arguments for the top-level `luck check` command.
struct CheckArgs {
    paths: Vec<String>,
    config: Option<PathBuf>,
    verbosity: Verbosity,
}

/// Resolve the `LuckConfig` for `check` from `-c` (via extends) or upward
/// discovery from cwd, returning it alongside the config directory used to
/// root include/exclude globs.
/// Implements the top-level, config-driven `luck check` command: parse every
/// target file and report parse errors. No output is written; the exit code is
/// 1 if any file has parse errors, else 0.
fn run_check(args: CheckArgs) -> ExitCode {
    let (luck_config, config_dir) = resolve_project_config(args.config.as_deref());

    // Resolve the parse target lazily per file from the config dialects.
    let target_for = |path: &Path| -> LuaTarget {
        luck_config.target_for_path(path).unwrap_or_else(|message| {
            eprintln!("Error: {message}");
            process::exit(EXIT_USAGE as i32);
        })
    };

    // Explicit files are checked unconditionally; directory walks honor the
    // project's include/exclude globs.
    let filter = luck_core::config::ProjectFilter::new(
        &config_dir,
        &luck_config.include,
        &luck_config.exclude,
    )
    .unwrap_or_else(|error| {
        eprintln!("Error: {error}");
        std::process::exit(2);
    });
    let files = collect_target_files(&args.paths, &filter);

    // Files parse in parallel; rendered errors flush in input order so
    // output matches the sequential loop byte-for-byte.
    use rayon::prelude::*;
    let mut files_with_errors = 0u32;
    {
        // One lock and one flush for the whole run; bounded sorted groups
        // keep peak outcome memory flat on huge projects.
        use std::io::Write;
        let mut stderr = std::io::BufWriter::new(std::io::stderr().lock());
        for chunk in files.chunks(parallel_chunk_size()) {
            let outcomes: Vec<Option<Vec<u8>>> = chunk
                .par_iter()
                .map(|file_path| {
                    let source = match luck_core::source_io::read_source_file(file_path) {
                        Ok(text) => text,
                        Err(error) => {
                            return Some(
                                format!("Error: cannot read {}: {error}\n", file_path.display())
                                    .into_bytes(),
                            );
                        }
                    };

                    let target = target_for(file_path);
                    let file_label = file_path.to_string_lossy().to_string();
                    let mut result = luck_parser::parse(source, target.lua_version());
                    if result.errors.is_empty() {
                        // Compile-time checks real Lua performs beyond the
                        // grammar (const writes, goto resolution). Opt-in
                        // here only - transform pipelines skip the cost.
                        result.errors = luck_parser::validate(&result.block, target.lua_version());
                    }
                    if result.errors.is_empty() {
                        return None;
                    }

                    let diagnostics: Vec<Diagnostic> = result
                        .errors
                        .iter()
                        .map(|err| {
                            luck_core::diagnostics::errors::e008(
                                &file_label,
                                err.span.into(),
                                &err.message,
                            )
                        })
                        .collect();
                    let mut cache = FileCache::new();
                    cache.add_file(file_label.clone(), result.source);
                    Some(render_diagnostics_to_buffer(&diagnostics, &mut cache))
                })
                .collect();

            for rendered in outcomes.into_iter().flatten() {
                files_with_errors += 1;
                let _ = stderr.write_all(&rendered);
            }
        }
    }

    if files_with_errors > 0 {
        return ExitCode::from(EXIT_FAILURE);
    }

    if args.verbosity != Verbosity::Quiet {
        eprintln!("ok: {} file(s) checked", files.len());
    }

    ExitCode::from(EXIT_SUCCESS)
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
fn print_lint_config(lint_config: &luck_linter::LintConfig) {
    let mut overrides = serde_json::Map::new();
    for (name, setting) in &lint_config.rule_overrides {
        let severity = setting.severity.map(|severity| match severity {
            luck_linter::diagnostic::Severity::Error => "error",
            luck_linter::diagnostic::Severity::Warning => "warning",
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

fn run_bundle(
    target: LuaTarget,
    entry: &str,
    output: Option<&str>,
    search_paths: &[String],
    // `Some(config)` minifies with that transform config; `None` skips.
    minify: Option<&TransformConfig>,
    line_map_path: Option<&Path>,
    verbosity: Verbosity,
) -> ExitCode {
    let entry_path = PathBuf::from(entry);
    if !entry_path.is_file() {
        eprintln!("Error: entry file not found: {entry}");
        return ExitCode::from(EXIT_USAGE);
    }

    // Root Lua search paths at the ENTRY's directory: `luck bundle
    // src/main.lua` should find src/'s siblings without a hand-crafted
    // `-s`, matching how `require` resolves relative to the script.
    let search_root = entry_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(current_dir_or_exit);

    match bundle(&entry_path, target, search_paths, &search_root) {
        Ok(result) => {
            if !result.warnings.is_empty() && verbosity != Verbosity::Quiet {
                let mut cache = build_file_cache(&result.warnings);
                render_diagnostics(&result.warnings, &mut cache);
            }

            let mut code = result.output;

            if let Some(config) = minify {
                match luck_minifier::minify(&code, target, config, entry) {
                    Ok(minified) => code = minified,
                    Err(errors) => fail_with_diagnostics(&errors, Some((entry, &code))),
                }
            }

            if let Some(map_path) = line_map_path {
                let entries: Vec<serde_json::Value> = result
                    .line_map
                    .iter()
                    .map(|entry| {
                        serde_json::json!({
                            "bundleStartLine": entry.bundle_start_line,
                            "bundleEndLine": entry.bundle_end_line,
                            "path": entry.path,
                        })
                    })
                    .collect();
                let map_json = serde_json::json!({ "version": 1, "entries": entries });
                if let Err(error) = std::fs::write(map_path, format!("{map_json:#}\n")) {
                    eprintln!("Error: cannot write {}: {error}", map_path.display());
                    return ExitCode::from(EXIT_FAILURE);
                }
            }

            write_output(output, &code);
            ExitCode::from(EXIT_SUCCESS)
        }
        Err(errors) => fail_with_diagnostics(&errors, None),
    }
}

fn run_minify(
    target: LuaTarget,
    input: &str,
    output: Option<&str>,
    config: &TransformConfig,
    stats: bool,
    // Minify emits no advisory banner; `--stats`, the result, and fatal
    // errors are all essential, so there is nothing for `--quiet` to silence.
    _verbosity: Verbosity,
) -> ExitCode {
    let (source, file_path) = if input == "-" {
        use std::io::Read;
        let mut buf = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
            eprintln!("Error: failed to read stdin: {e}");
            return ExitCode::from(EXIT_FAILURE);
        }
        (buf, "<stdin>".to_string())
    } else {
        let s = match luck_core::source_io::read_source_file(input) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: failed to read {input}: {e}");
                return ExitCode::from(EXIT_FAILURE);
            }
        };
        (s, input.to_string())
    };

    let original_size = source.len();

    match luck_minifier::minify(&source, target, config, &file_path) {
        Ok(minified) => {
            if stats {
                let minified_size = minified.len();
                let ratio = if original_size > 0 {
                    (minified_size as f64 / original_size as f64) * 100.0
                } else {
                    0.0
                };
                eprintln!(
                    "{} → {} ({:.1}%)",
                    format_size(original_size),
                    format_size(minified_size),
                    ratio
                );
            }
            write_output(output, &minified);
            ExitCode::from(EXIT_SUCCESS)
        }
        Err(errors) => fail_with_diagnostics(&errors, Some((&file_path, &source))),
    }
}

fn run_graph(
    target: LuaTarget,
    entry: &str,
    search_paths: &[String],
    format: GraphFormat,
    verbosity: Verbosity,
) -> ExitCode {
    use luck_bundler::graph::build_graph;

    let entry_path = PathBuf::from(entry);
    if !entry_path.is_file() {
        eprintln!("Error: entry file not found: {entry}");
        return ExitCode::from(EXIT_USAGE);
    }

    let cwd = current_dir_or_exit();

    match build_graph(&entry_path, target, search_paths, &cwd) {
        Ok(dep_graph) => {
            if !dep_graph.warnings.is_empty() && verbosity != Verbosity::Quiet {
                let mut cache = build_file_cache(&dep_graph.warnings);
                render_diagnostics(&dep_graph.warnings, &mut cache);
            }

            match format {
                GraphFormat::Json => print_graph_json(&dep_graph),
                GraphFormat::Dot => print_graph_dot(&dep_graph),
            }
            ExitCode::from(EXIT_SUCCESS)
        }
        Err(errors) => fail_with_diagnostics(&errors, None),
    }
}

fn collect_lua_files(dir: &Path, filter: &luck_core::config::ProjectFilter) -> Vec<PathBuf> {
    use ignore::WalkBuilder;

    let mut files = Vec::new();
    let walker = WalkBuilder::new(dir)
        .add_custom_ignore_filename(".luckignore")
        .build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "lua" || ext == "luau" {
                    // include/exclude globs from luck.json gate the walk; the
                    // filter compares against canonical paths so strip_prefix
                    // against the (canonical) rc_dir lines up.
                    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                    if filter.is_included(&abs) {
                        files.push(path.to_path_buf());
                    }
                }
            }
        }
    }
    files.sort();
    files
}

/// Group size for the parallel drivers: a few batches per thread keeps
/// the pool saturated while bounding how many rendered outcomes are held
/// in memory at once.
fn parallel_chunk_size() -> usize {
    rayon::current_num_threads() * 4
}

fn current_dir_or_exit() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: failed to get current directory: {e}");
        process::exit(EXIT_USAGE as i32);
    })
}

fn write_output(output: Option<&str>, content: &str) {
    if let Some(path) = output {
        write_output_file(&PathBuf::from(path), content);
    } else {
        print!("{content}");
    }
}

fn write_output_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent()
        && !parent.exists()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("Error: failed to create output directory: {e}");
        process::exit(EXIT_FAILURE as i32);
    }
    if let Err(e) = std::fs::write(path, content) {
        eprintln!("Error: failed to write output: {e}");
        process::exit(EXIT_FAILURE as i32);
    }
}

fn print_graph_json(dep_graph: &luck_bundler::graph::DependencyGraph) {
    use serde_json::{Map, Value, json};

    let entry_path = &dep_graph.modules[dep_graph.entry_id.0].path;

    let mut modules_map = Map::new();
    for module in &dep_graph.modules {
        let requires: Vec<&str> = module
            .dependencies
            .iter()
            .map(|(_, req_str, _, _)| req_str.as_str())
            .collect();
        let resolved_deps: Vec<&str> = module
            .dependencies
            .iter()
            .map(|(_, _, path, _)| path.as_str())
            .collect();

        modules_map.insert(
            module.path.clone(),
            json!({
                "requires": requires,
                "resolved_deps": resolved_deps,
            }),
        );
    }

    let order: Vec<&str> = dep_graph
        .topo_order
        .iter()
        .map(|id| dep_graph.modules[id.0].path.as_str())
        .collect();

    let output = json!({
        "entry": entry_path,
        "modules": Value::Object(modules_map),
        "order": order,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&output).expect("failed to serialize dependency graph")
    );
}

fn print_graph_dot(dep_graph: &luck_bundler::graph::DependencyGraph) {
    println!("digraph dependencies {{");
    for module in &dep_graph.modules {
        for (_, _, dep_path, _) in &module.dependencies {
            println!("  \"{}\" -> \"{}\";", module.path, dep_path);
        }
    }
    println!("}}");
}

fn build_file_cache(diagnostics: &[Diagnostic]) -> FileCache {
    let mut cache = FileCache::new();
    for diag in diagnostics {
        let os_path = diag.file_path.replace('/', std::path::MAIN_SEPARATOR_STR);
        if let Ok(source) = luck_core::source_io::read_source_file(&os_path) {
            cache.add_file(diag.file_path.clone(), source);
        }
    }
    cache
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_is_top_level_command_with_target_flag() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["luck", "bundle", "entry.lua", "-t", "54"])
            .expect("bundle parses");
        match cli.command {
            Command::Bundle { entry, target, .. } => {
                assert_eq!(entry, "entry.lua");
                assert_eq!(target, Some("54".to_string()));
            }
            _ => panic!("expected Command::Bundle"),
        }
    }

    #[test]
    fn minify_target_is_optional() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["luck", "minify", "x.luau"])
            .expect("minify parses without target");
        match cli.command {
            Command::Minify { input, target, .. } => {
                assert_eq!(input, "x.luau");
                assert_eq!(target, None);
            }
            _ => panic!("expected Command::Minify"),
        }
    }

    #[test]
    fn graph_accepts_target_and_format() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "luck",
            "graph",
            "src/main.lua",
            "-t",
            "54",
            "--format",
            "dot",
        ])
        .expect("graph parses");
        match cli.command {
            Command::Graph {
                entry,
                target,
                format,
                ..
            } => {
                assert_eq!(entry, "src/main.lua");
                assert_eq!(target, Some("54".to_string()));
                assert!(matches!(format, GraphFormat::Dot));
            }
            _ => panic!("expected Command::Graph"),
        }
    }

    #[test]
    fn resolve_explicit_target_infers_from_extension() {
        // Omitted target infers from the input extension.
        assert_eq!(resolve_explicit_target(None, "main.luau"), LuaTarget::Luau);
        assert_eq!(resolve_explicit_target(None, "main.lua"), LuaTarget::Lua54);
        // No extension falls back to Lua54.
        assert_eq!(resolve_explicit_target(None, "main"), LuaTarget::Lua54);
    }

    #[test]
    fn resolve_explicit_target_parses_when_provided() {
        assert_eq!(
            resolve_explicit_target(Some("54"), "main.luau"),
            LuaTarget::Lua54
        );
        assert_eq!(
            resolve_explicit_target(Some("roblox"), "main.lua"),
            LuaTarget::LuauRoblox
        );
    }

    #[test]
    fn lsp_subcommand_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["luck", "lsp"]).expect("lsp parses");
        assert!(matches!(cli.command, Command::Lsp { socket: None }));
        let with_socket = Cli::try_parse_from(["luck", "lsp", "--socket", "9000"]).unwrap();
        assert!(matches!(
            with_socket.command,
            Command::Lsp { socket: Some(9000) }
        ));
    }

    /// Build a `LintArgs` with everything defaulted, overriding via a closure.
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
            verbosity: Verbosity::Quiet,
        }
    }

    #[test]
    fn lint_is_top_level_command_with_oxlint_flags() {
        use clap::Parser;
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
            Command::Lint {
                paths,
                deny,
                allow,
                warn,
                fix,
                config,
                ..
            } => {
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
        use clap::Parser;
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
            Command::Lint {
                max_warnings,
                deny_warnings,
                silent,
                rules,
                print_config,
                format,
                ..
            } => {
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
    fn category_name_classification() {
        use luck_linter::diagnostic::Category;
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
        assert_eq!(
            setting.severity,
            Some(luck_linter::diagnostic::Severity::Error)
        );
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
        assert!(
            lint_config
                .categories
                .contains(&luck_linter::diagnostic::Category::Style)
        );
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

    #[test]
    fn collect_lua_files_honors_exclude_glob() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // luck.json with an exclude glob over a generated directory.
        std::fs::write(
            root.join("luck.json"),
            r#"{"lua":"lua54","entry":"src/main.lua","exclude":["gen/**"]}"#,
        )
        .expect("write luck.json");

        let src = root.join("src");
        let generated = root.join("gen");
        std::fs::create_dir_all(&src).expect("mkdir src");
        std::fs::create_dir_all(&generated).expect("mkdir gen");
        std::fs::write(src.join("keep.lua"), "return 1\n").expect("write keep");
        std::fs::write(generated.join("skip.lua"), "return 2\n").expect("write skip");

        // Passing the RAW (non-canonical) root proves the production
        // path: ProjectFilter canonicalizes internally. The old version
        // of this test canonicalized by hand, hiding a bug where every
        // include/exclude glob silently matched nothing on Windows.
        let filter =
            luck_core::config::ProjectFilter::new(root, &None, &Some(vec!["gen/**".to_string()]))
                .expect("valid globs");
        let files = collect_lua_files(root, &filter);

        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"keep.lua".to_string()), "kept file present");
        assert!(
            !names.contains(&"skip.lua".to_string()),
            "excluded file skipped"
        );
    }

    #[test]
    fn init_config_default_sets_both_axes() {
        let content = init_config_content(LuaTarget::Lua54);
        let config = luck_core::config::parse_luck_config(&content)
            .expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert!(content.contains("\"entry\": \"src/main.lua\""));
        assert!(content.contains("search_paths"));
    }

    #[test]
    fn init_config_lua_dialect_sets_lua_axis_only() {
        let content = init_config_content(LuaTarget::Lua53);
        let config = luck_core::config::parse_luck_config(&content)
            .expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua53"));
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert!(content.contains("\"entry\": \"src/main.lua\""));
    }

    #[test]
    fn init_config_roblox_sets_luau_axis_and_luau_entry() {
        let content = init_config_content(LuaTarget::LuauRoblox);
        let config = luck_core::config::parse_luck_config(&content)
            .expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.luau.as_deref(), Some("roblox"));
        assert!(content.contains("\"entry\": \"src/main.luau\""));
        // Luau projects have no Lua-style search paths.
        assert!(!content.contains("search_paths"));
    }

    #[test]
    fn collect_lua_files_default_filter_takes_all_lua() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("a.lua"), "return 1\n").expect("write a");
        std::fs::write(root.join("b.luau"), "return 2\n").expect("write b");
        std::fs::write(root.join("c.txt"), "nope\n").expect("write c");

        // Default include globs (*.lua, *.luau) when no overrides are given.
        let filter =
            luck_core::config::ProjectFilter::new(root, &None, &None).expect("valid globs");
        let files = collect_lua_files(root, &filter);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"a.lua".to_string()));
        assert!(names.contains(&"b.luau".to_string()));
        assert!(!names.contains(&"c.txt".to_string()));
    }

    #[test]
    fn fmt_is_top_level_command_with_oxfmt_flags() {
        use clap::Parser;
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
            Command::Fmt {
                paths,
                check,
                list_different,
                write,
                config,
                ..
            } => {
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
        use clap::Parser;
        let listed = Cli::try_parse_from(["luck", "fmt", "--list-different"])
            .expect("fmt --list-different parses");
        assert!(matches!(
            listed.command,
            Command::Fmt {
                list_different: true,
                ..
            }
        ));
        let written = Cli::try_parse_from(["luck", "fmt", "--write"]).expect("fmt --write parses");
        assert!(matches!(written.command, Command::Fmt { write: true, .. }));
        // --check and --write are mutually exclusive.
        assert!(Cli::try_parse_from(["luck", "fmt", "--check", "--write"]).is_err());
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
    fn check_is_top_level_command_with_paths_and_config() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["luck", "check", "src", "main.lua", "-c", "luck.json"])
            .expect("check parses");
        match cli.command {
            Command::Check { paths, config } => {
                assert_eq!(paths, vec!["src".to_string(), "main.lua".to_string()]);
                assert_eq!(config, Some(PathBuf::from("luck.json")));
            }
            _ => panic!("expected Command::Check"),
        }
    }

    #[test]
    fn check_resolves_target_per_file_from_config() {
        // A `.luau` file under a roblox config must resolve to the roblox
        // target, mirroring how `run_check` selects the parse dialect.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(&config_path, r#"{"luau":"roblox"}"#).expect("write luck.json");

        let args = CheckArgs {
            paths: Vec::new(),
            config: Some(config_path),
            verbosity: Verbosity::Quiet,
        };
        let (luck_config, _) = resolve_project_config(args.config.as_deref());
        let target = luck_config
            .target_for_path(&dir.path().join("main.luau"))
            .expect("luau target resolves");
        assert_eq!(target, LuaTarget::LuauRoblox);
    }

    #[test]
    fn check_reports_parse_errors_via_formatter_path() {
        // The formatter's parse path surfaces syntax errors that `run_check`
        // turns into a nonzero exit; a syntactically broken file must produce
        // at least one error, while a clean file produces none.
        let broken = luck_formatter::format(
            "local x =",
            LuaTarget::Lua54.lua_version(),
            &luck_formatter::FormatOptions::default(),
        );
        assert!(!broken.errors.is_empty(), "syntax error should be reported");

        let clean = luck_formatter::format(
            "local x = 1\n",
            LuaTarget::Lua54.lua_version(),
            &luck_formatter::FormatOptions::default(),
        );
        assert!(clean.errors.is_empty(), "valid source has no parse errors");
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

        run_fmt(FmtArgs {
            paths: vec![source_path.to_string_lossy().into_owned()],
            write: true,
            check: false,
            list_different: false,
            config: Some(config_path),
            no_editorconfig: false,
            stdin_filepath: None,
            range_start: None,
            range_end: None,
            verbosity: Verbosity::Quiet,
        });

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

        run_fmt(FmtArgs {
            paths: vec![source_path.to_string_lossy().into_owned()],
            write: true,
            check: false,
            list_different: false,
            config: Some(config_path),
            no_editorconfig: false,
            stdin_filepath: None,
            range_start: None,
            range_end: None,
            verbosity: Verbosity::Quiet,
        });

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
        let luck_config = luck_core::config::LuckConfig::default();
        let path = PathBuf::from("stdin.lua");

        let unformatted = format_document(
            "local  x=1\n",
            "stdin.lua",
            &path,
            LuaTarget::Lua54,
            &luck_config,
            false,
            None,
        );
        let formatted = match unformatted {
            FormatOutcome::Changed(output) => output,
            _ => panic!("messy source should reformat to Changed"),
        };

        // Re-running over already-formatted text yields Unchanged (idempotency).
        assert!(matches!(
            format_document(
                &formatted,
                "stdin.lua",
                &path,
                LuaTarget::Lua54,
                &luck_config,
                false,
                None,
            ),
            FormatOutcome::Unchanged
        ));

        assert!(matches!(
            format_document(
                "local x =",
                "stdin.lua",
                &path,
                LuaTarget::Lua54,
                &luck_config,
                false,
                None,
            ),
            FormatOutcome::ParseError(_)
        ));
    }

    #[test]
    fn fmt_stdin_filepath_parses_on_both_commands() {
        use clap::Parser;
        let fmt = Cli::try_parse_from(["luck", "fmt", "--stdin-filepath", "buf.lua"])
            .expect("fmt --stdin-filepath parses");
        assert!(matches!(
            fmt.command,
            Command::Fmt { stdin_filepath: Some(ref p), .. } if p == "buf.lua"
        ));
        let lint = Cli::try_parse_from(["luck", "lint", "--stdin-filepath", "buf.luau"])
            .expect("lint --stdin-filepath parses");
        assert!(matches!(
            lint.command,
            Command::Lint { stdin_filepath: Some(ref p), .. } if p == "buf.luau"
        ));
    }

    #[test]
    fn fmt_format_options_default_without_format_section() {
        let config = luck_core::config::FormatConfig::default();
        let options = luck_formatter::FormatOptions::from(&config);
        let defaults = luck_formatter::FormatOptions::default();
        assert_eq!(options.indent_width, defaults.indent_width);
        assert_eq!(options.line_width, defaults.line_width);
    }
}
