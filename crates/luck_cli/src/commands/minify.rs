//! `luck minify` - minify a single source file (or stdin).

use std::path::PathBuf;

use clap::Args;
use luck_core::{LuaTarget, read_source_file};

use crate::exit::{Exit, fail_with_source_errors, operation_error};
use crate::minify_flags::MinifyFlags;
use crate::output::{format_size, read_stdin_source, write_output};
use crate::project::{config_for_one_shot, resolve_configured_target};

#[derive(Args)]
pub(crate) struct MinifyArgs {
    /// Input file, or - for stdin.
    input: String,

    /// Lua target [default: from the input's extension and luck.json].
    #[arg(short = 't', long = "target", value_name = "TARGET")]
    target: Option<LuaTarget>,

    /// Output file [default: stdout].
    #[arg(short, long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Project config [default: discovered from input directory, or cwd for stdin].
    #[arg(short = 'c', long = "config", value_name = "PATH")]
    config: Option<PathBuf>,

    /// Print size statistics to stderr.
    #[arg(long)]
    stats: bool,

    #[command(flatten)]
    minify_flags: MinifyFlags,
}

impl MinifyArgs {
    /// Takes no verbosity: `--quiet` has nothing to hide here, since minify
    /// prints only its result, `--stats`, and errors.
    ///
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config, and [`Exit::Failure`] when the
    /// input can't be read or minified or the result can't be written.
    pub(crate) fn run(self) -> Result<(), Exit> {
        let config = config_for_one_shot(self.config.as_deref(), &self.input)?;
        let target = resolve_configured_target(&config, self.target, &self.input)?;
        let transforms = self.minify_flags.apply_to(config.transforms.unwrap_or_default());

        let (source, file_path) = if self.input == "-" {
            (read_stdin_source()?, "<stdin>".to_owned())
        } else {
            let source = read_source_file(&self.input).map_err(|error| {
                operation_error(format!("failed to read `{}`: {error}", self.input))
            })?;
            (source, self.input)
        };

        let original_size = source.len();

        let version = target.lua_version();
        let minified = luck_minifier::minify(&source, version, &transforms)
            .map_err(|errors| fail_with_source_errors(file_path, &source, &errors, version))?;

        if self.stats {
            let minified_size = minified.len();
            #[expect(
                clippy::as_conversions,
                clippy::cast_precision_loss,
                clippy::float_arithmetic,
                reason = "display math, and its exact rounding is part of the output"
            )]
            let ratio: f64 = if original_size > 0 {
                (minified_size as f64 / original_size as f64) * 100.0
            } else {
                0.0
            };
            eprintln!(
                "{} -> {} ({ratio:.1}%)",
                format_size(original_size),
                format_size(minified_size),
            );
        }
        write_output(self.output.as_deref(), &minified)
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::MinifyArgs;
    use crate::cli::{Cli, Command};

    #[test]
    fn minify_target_is_optional() {
        let cli = Cli::try_parse_from(["luck", "minify", "x.luau"])
            .expect("minify parses without target");
        let Command::Minify(MinifyArgs { input, target, .. }) = cli.command else {
            panic!("expected Command::Minify");
        };
        assert_eq!(input, "x.luau");
        assert_eq!(target, None);
    }
}
