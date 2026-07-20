//! `luck bundle` - bundle a multi-file project into a single file.

use crate::minify_flags::MinifyFlags;
use crate::output::{build_file_cache, current_dir_or_exit, fail_with_diagnostics, write_output};
use crate::project::resolve_explicit_target;
use crate::render::render_diagnostics;
use crate::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE, Verbosity};
use clap::Args;
use luck_bundler::bundle;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Args)]
pub(crate) struct BundleArgs {
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
}

impl BundleArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> ExitCode {
        let target = resolve_explicit_target(self.target.as_deref(), &self.entry);
        let transforms = self.minify_flags.to_transform_config();

        let entry_path = PathBuf::from(&self.entry);
        if !entry_path.is_file() {
            eprintln!("Error: entry file not found: {}", self.entry);
            return ExitCode::from(EXIT_USAGE);
        }

        // Root Lua search paths at the ENTRY's directory: `luck bundle
        // src/main.lua` should find src/'s siblings without a hand-crafted
        // `-s`, matching how `require` resolves relative to the script.
        let search_root = entry_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(current_dir_or_exit);

        match bundle(&entry_path, target, &self.search_path, &search_root) {
            Ok(result) => {
                if !result.warnings.is_empty() && verbosity != Verbosity::Quiet {
                    let mut cache = build_file_cache(&result.warnings);
                    render_diagnostics(&result.warnings, &mut cache);
                }

                let mut code = result.output;

                if self.minify {
                    match luck_minifier::minify(&code, target, &transforms, &self.entry) {
                        Ok(minified) => code = minified,
                        Err(errors) => fail_with_diagnostics(&errors, Some((&self.entry, &code))),
                    }
                }

                if let Some(map_path) = self.line_map.as_deref() {
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

                write_output(self.output.as_deref(), &code);
                ExitCode::from(EXIT_SUCCESS)
            }
            Err(errors) => fail_with_diagnostics(&errors, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BundleArgs;
    use crate::args::{Cli, Command};
    use clap::Parser;

    #[test]
    fn bundle_is_top_level_command_with_target_flag() {
        let cli = Cli::try_parse_from(["luck", "bundle", "entry.lua", "-t", "54"])
            .expect("bundle parses");
        match cli.command {
            Command::Bundle(BundleArgs { entry, target, .. }) => {
                assert_eq!(entry, "entry.lua");
                assert_eq!(target, Some("54".to_string()));
            }
            _ => panic!("expected Command::Bundle"),
        }
    }
}
