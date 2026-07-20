//! `luck check` - parse every target file and report syntax errors.

use crate::output::parallel_chunk_size;
use crate::project::{collect_target_files, project_filter, resolve_project_config};
use crate::render::{FileCache, render_diagnostics_to_buffer};
use crate::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE, Verbosity};
use clap::Args;
use luck_core::diagnostics::Diagnostic;
use luck_core::types::LuaTarget;
use std::path::{Path, PathBuf};
use std::process;
use std::process::ExitCode;

#[derive(Args)]
pub(crate) struct CheckArgs {
    /// Files or directories to check [default: current directory]
    paths: Vec<String>,

    /// Path to config file [default: discover luck.json upward from cwd]
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,
}

impl CheckArgs {
    /// Parse every target file and report parse errors. No output is written;
    /// the exit code is 1 if any file has parse errors, else 0.
    pub(crate) fn run(self, verbosity: Verbosity) -> ExitCode {
        let (luck_config, config_dir) = resolve_project_config(self.config.as_deref());

        // Resolve the parse target lazily per file from the config dialects.
        let target_for = |path: &Path| -> LuaTarget {
            luck_config.target_for_path(path).unwrap_or_else(|message| {
                eprintln!("Error: {message}");
                process::exit(EXIT_USAGE as i32);
            })
        };

        let filter = project_filter(&config_dir, &luck_config);
        let files = collect_target_files(&self.paths, &filter);

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
                    .map(|file_path| check_file(file_path, target_for(file_path)))
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

        if verbosity != Verbosity::Quiet {
            eprintln!("ok: {} file(s) checked", files.len());
        }

        ExitCode::from(EXIT_SUCCESS)
    }
}

/// Parse and validate one file, returning rendered diagnostics when it fails
/// to parse (or cannot be read), or `None` when it is clean.
fn check_file(file_path: &Path, target: LuaTarget) -> Option<Vec<u8>> {
    let source = match luck_core::source_io::read_source_file(file_path) {
        Ok(text) => text,
        Err(error) => {
            return Some(
                format!("Error: cannot read {}: {error}\n", file_path.display()).into_bytes(),
            );
        }
    };

    let file_label = file_path.to_string_lossy().to_string();
    let mut result = luck_parser::parse(source, target.lua_version());
    if result.errors.is_empty() {
        // Compile-time checks real Lua performs beyond the grammar (const
        // writes, goto resolution). Opt-in here only - transform pipelines
        // skip the cost.
        result.errors = luck_parser::validate(&result.block, target.lua_version());
    }
    if result.errors.is_empty() {
        return None;
    }

    let diagnostics: Vec<Diagnostic> = result
        .errors
        .iter()
        .map(|err| luck_core::diagnostics::errors::e008(&file_label, err.span.into(), &err.message))
        .collect();
    let mut cache = FileCache::new();
    cache.add_file(file_label, result.source);
    Some(render_diagnostics_to_buffer(&diagnostics, &mut cache))
}

#[cfg(test)]
mod tests {
    use super::CheckArgs;
    use crate::args::{Cli, Command};
    use crate::project::resolve_project_config;
    use clap::Parser;
    use luck_core::types::LuaTarget;

    #[test]
    fn check_is_top_level_command_with_paths_and_config() {
        let cli = Cli::try_parse_from(["luck", "check", "src", "main.lua", "-c", "luck.json"])
            .expect("check parses");
        match cli.command {
            Command::Check(CheckArgs { paths, config }) => {
                assert_eq!(paths, vec!["src".to_string(), "main.lua".to_string()]);
                assert_eq!(config, Some(std::path::PathBuf::from("luck.json")));
            }
            _ => panic!("expected Command::Check"),
        }
    }

    #[test]
    fn check_resolves_target_per_file_from_config() {
        // A `.luau` file under a roblox config must resolve to the roblox
        // target, mirroring how `run` selects the parse dialect.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("luck.json");
        std::fs::write(&config_path, r#"{"luau":"roblox"}"#).expect("write luck.json");

        let (luck_config, _) = resolve_project_config(Some(config_path.as_path()));
        let target = luck_config
            .target_for_path(&dir.path().join("main.luau"))
            .expect("luau target resolves");
        assert_eq!(target, LuaTarget::LuauRoblox);
    }

    #[test]
    fn check_reports_parse_errors_via_formatter_path() {
        // A syntactically broken file must produce at least one error, while a
        // clean file produces none.
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
}
