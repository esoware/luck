use std::fs;
use std::path::{Path, PathBuf};

use clap::Args;
use luck_allocator::ThreadAllocators;
use luck_bundler::{DynamicRequire, LineMapEntry, bundle, bundle_minified};
use luck_config::check_output_paths;
use luck_resolver::to_slash_lossy;
use luck_token::LuaTarget;

use crate::exit::{Exit, fail_with_diags, operation_error, usage_error};
use crate::minify_flags::MinifyFlags;
use crate::output::write_output;
use crate::project::{bundle_options, existing_file, one_shot_project};
use crate::reporter::print_files;
use crate::verbosity::Verbosity;

#[derive(Args)]
pub(crate) struct BundleArgs {
    /// Entry file.
    entry: String,

    /// Lua target [default: from the entry's extension and luck.json].
    #[arg(short = 't', long = "target", value_name = "TARGET")]
    target: Option<LuaTarget>,

    /// Output file [default: stdout].
    #[arg(short, long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Project config [default: discovered from entry directory].
    #[arg(short = 'c', long = "config", value_name = "PATH")]
    config: Option<PathBuf>,

    /// Search path template (repeatable) [default: luck.json's `search_paths`].
    #[arg(short = 's', long = "search-path", value_name = "PATTERN")]
    search_paths: Vec<String>,

    /// Minify the output.
    #[arg(long)]
    minify: bool,

    /// How to treat require(expr): error, warn, or allow [default: from luck.json].
    #[arg(long = "dynamic-require", value_name = "MODE")]
    dynamic_require: Option<DynamicRequire>,

    /// Write a JSON line map (bundle lines -> source files) to PATH.
    /// Incompatible with --minify, which rewrites line structure.
    #[arg(long = "line-map", value_name = "PATH", conflicts_with = "minify")]
    line_map: Option<PathBuf>,

    #[command(flatten)]
    minify_flags: MinifyFlags,
}

impl BundleArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let entry_path = existing_file(&self.entry)?;
        let project = one_shot_project(self.config.as_deref(), entry_path)?;
        let target = self.target.unwrap_or_else(|| project.target_for(Some(entry_path)));
        let version = target.version();
        let transforms = self.minify_flags.apply_to(project.transforms());
        let options = bundle_options(&project, version, &self.search_paths, self.dynamic_require);

        let arenas = ThreadAllocators::new();
        if self.minify {
            let minified = bundle_minified(&arenas, entry_path, &options, &transforms)
                .map_err(|errors| fail_with_diags(&errors, version))?;
            check_output_paths(&minified.source_files, self.output.iter()).map_err(usage_error)?;
            if verbosity != Verbosity::Quiet {
                print_files(&minified.warnings, version);
            }
            return write_output(self.output.as_deref(), &minified.output);
        }

        let result = bundle(&arenas, entry_path, &options)
            .map_err(|errors| fail_with_diags(&errors, version))?;

        check_output_paths(&result.source_files, self.output.iter().chain(&self.line_map))
            .map_err(usage_error)?;

        if verbosity != Verbosity::Quiet {
            print_files(&result.warnings, version);
        }

        if let Some(map_path) = self.line_map.as_deref() {
            write_line_map(map_path, &result.line_map)?;
        }

        write_output(self.output.as_deref(), &result.output)
    }
}

fn write_line_map(map_path: &Path, line_map: &[LineMapEntry]) -> Result<(), Exit> {
    let entries: Vec<serde_json::Value> = line_map
        .iter()
        .map(|entry| {
            serde_json::json!({
                "bundleStartLine": entry.bundle_start_line,
                "bundleEndLine": entry.bundle_end_line,
                "path": to_slash_lossy(&entry.path),
            })
        })
        .collect();
    let map_json = serde_json::json!({ "version": 1, "entries": entries });
    fs::write(map_path, format!("{map_json:#}\n"))
        .map_err(|e| operation_error(format!("cannot write `{}`: {e}", map_path.display())))
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use luck_token::LuaTarget;

    use super::BundleArgs;
    use crate::cli::{Cli, Command};

    #[test]
    fn bundle_is_top_level_command_with_target_flag() {
        let cli = Cli::try_parse_from(["luck", "bundle", "entry.lua", "-t", "54"])
            .expect("bundle parses");
        let Command::Bundle(BundleArgs { entry, target, .. }) = cli.command else {
            panic!("expected Command::Bundle");
        };
        assert_eq!(entry, "entry.lua");
        assert_eq!(target, Some(LuaTarget::Lua54));
    }
}
