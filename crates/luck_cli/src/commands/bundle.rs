//! `luck bundle` - bundle a multi-file project into a single file.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Args;
use luck_bundler::{BundleOptions, LineMapEntry, bundle};
use luck_core::{DynamicRequire, LuaTarget};

use crate::exit::{Exit, fail_with_diags, fail_with_source_errors, operation_error, usage_error};
use crate::minify_flags::MinifyFlags;
use crate::output::write_output;
use crate::project::{
    config_for_one_shot, current_directory, resolve_configured_target, resolve_dynamic_require,
};
use crate::render::print_diags;
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

    /// Search path template (repeatable).
    #[arg(short = 's', long = "search-path", value_name = "PATTERN")]
    search_paths: Vec<String>,

    /// Minify the output.
    #[arg(long)]
    minify: bool,

    /// How to treat require(expr): error, warn, or allow.
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
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config or entry path, and
    /// [`Exit::Failure`] when bundling, minifying, or writing fails.
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let config = config_for_one_shot(self.config.as_deref(), &self.entry)?;
        let target = resolve_configured_target(&config, self.target, &self.entry)?;
        let version = target.lua_version();
        let dynamic_require = resolve_dynamic_require(&config, self.dynamic_require);
        let transforms = self.minify_flags.apply_to(config.transforms.unwrap_or_default());

        let entry_path = Path::new(&self.entry);
        if !entry_path.is_file() {
            return Err(usage_error(format!("entry file not found: `{}`", self.entry)));
        }

        // Root search paths at the entry's directory, so `luck bundle
        // src/main.lua` finds the modules next to it without `-s`.
        let search_root = entry_path
            .parent()
            .map_or_else(current_directory, |parent| Ok(parent.to_path_buf()))?;

        let result = bundle(
            entry_path,
            &BundleOptions {
                version,
                search_paths: &self.search_paths,
                project_root: &search_root,
                dynamic_require,
            },
        )
        .map_err(|errors| fail_with_diags(&errors, version))?;

        if verbosity != Verbosity::Quiet {
            print_diags(&result.warnings, version);
        }

        let mut code = result.output;

        if self.minify {
            code = luck_minifier::minify(&code, version, &transforms)
                .map_err(|errors| fail_with_source_errors(self.entry, &code, &errors, version))?;
        }

        if let Some(map_path) = self.line_map.as_deref() {
            write_line_map(map_path, &result.line_map)?;
        }

        write_output(self.output.as_deref(), &code)
    }
}

/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when `map_path` can't be
/// written.
fn write_line_map(map_path: &Path, line_map: &[LineMapEntry]) -> Result<(), Exit> {
    let entries: Vec<serde_json::Value> = line_map
        .iter()
        .map(|entry| {
            serde_json::json!({
                "bundleStartLine": entry.bundle_start_line,
                "bundleEndLine": entry.bundle_end_line,
                "path": entry.path,
            })
        })
        .collect();
    let map_json = serde_json::json!({ "version": 1u8, "entries": entries });
    fs::write(map_path, format!("{map_json:#}\n"))
        .map_err(|error| operation_error(format!("cannot write `{}`: {error}", map_path.display())))
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use luck_core::LuaTarget;

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
