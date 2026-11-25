use std::path::{Path, PathBuf};

use clap::Args;
use luck_allocator::Allocator;
use luck_resolver::{FileSystem as _, FileSystemOs};

use crate::exit::{Exit, operation_error};
use crate::minify_flags::MinifyFlags;
use crate::output::{format_size, read_stdin_source, write_output};
use crate::project::{existing_file, one_shot_project};
use crate::reporter::print_document;

#[derive(Args)]
pub(crate) struct MinifyArgs {
    /// Input file, or - for stdin.
    input: String,

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
    pub(crate) fn run(self) -> Result<(), Exit> {
        let input = Path::new(&self.input);
        let project = one_shot_project(self.config.as_deref(), input)?;
        let transforms = self.minify_flags.apply_to(project.transforms());

        let (source, label) = if self.input == "-" {
            (read_stdin_source()?, "<stdin>".to_owned())
        } else {
            let source = FileSystemOs
                .read(existing_file(&self.input)?)
                .map_err(|e| operation_error(format!("failed to read `{}`: {e}", self.input)))?;
            (source, self.input)
        };

        let original_size = source.len();

        let minified =
            luck_minifier::minify(&Allocator::new(), &source, &transforms).map_err(|errors| {
                print_document(&label, &source, &errors);
                Exit::Failure
            })?;

        if self.stats {
            let minified_size = minified.len();
            #[expect(
                clippy::cast_precision_loss,
                reason = "f64 rounds only a size past 2^53 bytes"
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
    use std::path::Path;

    use clap::Parser as _;

    use super::MinifyArgs;
    use crate::cli::Cli;
    use crate::commands::Command;

    #[test]
    fn minify_takes_an_input_and_an_output() {
        let cli = Cli::try_parse_from(["luck", "minify", "x.luau", "-o", "y.luau"])
            .expect("minify parses");
        let Command::Minify(MinifyArgs { input, output, .. }) = cli.command else {
            panic!("expected Command::Minify");
        };
        assert_eq!(input, "x.luau");
        assert_eq!(output.as_deref(), Some(Path::new("y.luau")));
    }
}
