//! `luck minify` - minify a single source file (or stdin).

use crate::minify_flags::MinifyFlags;
use crate::output::{fail_with_diagnostics, format_size, write_output};
use crate::project::resolve_explicit_target;
use crate::{EXIT_FAILURE, EXIT_SUCCESS, Verbosity};
use clap::Args;
use std::process::ExitCode;

#[derive(Args)]
pub(crate) struct MinifyArgs {
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
}

impl MinifyArgs {
    // Minify emits no advisory banner; `--stats`, the result, and fatal errors
    // are all essential, so there is nothing for `--quiet` to silence.
    pub(crate) fn run(self, _verbosity: Verbosity) -> ExitCode {
        let target = resolve_explicit_target(self.target.as_deref(), &self.input);
        let config = self.minify_flags.to_transform_config();

        let (source, file_path) = if self.input == "-" {
            use std::io::Read;
            let mut source = String::new();
            if let Err(error) = std::io::stdin().read_to_string(&mut source) {
                eprintln!("Error: failed to read stdin: {error}");
                return ExitCode::from(EXIT_FAILURE);
            }
            (source, "<stdin>".to_string())
        } else {
            match luck_core::source_io::read_source_file(&self.input) {
                Ok(source) => (source, self.input.clone()),
                Err(error) => {
                    eprintln!("Error: failed to read {}: {error}", self.input);
                    return ExitCode::from(EXIT_FAILURE);
                }
            }
        };

        let original_size = source.len();

        match luck_minifier::minify(&source, target, &config, &file_path) {
            Ok(minified) => {
                if self.stats {
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
                write_output(self.output.as_deref(), &minified);
                ExitCode::from(EXIT_SUCCESS)
            }
            Err(errors) => fail_with_diagnostics(&errors, Some((&file_path, &source))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MinifyArgs;
    use crate::args::{Cli, Command};
    use clap::Parser;

    #[test]
    fn minify_target_is_optional() {
        let cli = Cli::try_parse_from(["luck", "minify", "x.luau"])
            .expect("minify parses without target");
        match cli.command {
            Command::Minify(MinifyArgs { input, target, .. }) => {
                assert_eq!(input, "x.luau");
                assert_eq!(target, None);
            }
            _ => panic!("expected Command::Minify"),
        }
    }
}
