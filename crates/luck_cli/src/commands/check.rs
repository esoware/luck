use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use clap::Args;
use luck_core::{LuaTarget, read_source_file};

use crate::exit::Exit;
use crate::project::{outcomes_in_order, project_files};
use crate::render::render_source_errors;
use crate::verbosity::Verbosity;

#[derive(Args)]
pub(crate) struct CheckArgs {
    /// Files, directories, or globs to check, and `!` globs to leave out
    /// [default: current directory].
    paths: Vec<String>,

    /// Use this config for every file [default: the nearest luck.json above
    /// each file].
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Exit 0 when no file matches.
    #[arg(long)]
    no_error_on_unmatched_pattern: bool,
}

impl CheckArgs {
    /// # Errors
    ///
    /// Returns [`Exit::Failure`] if any file can't be read or has errors, and
    /// [`Exit::Usage`] on a bad config or path, or when no file matches.
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let files = project_files(
            self.config.as_deref(),
            &self.paths,
            self.no_error_on_unmatched_pattern,
            |_| Ok(()),
        )?;

        let mut has_errors = false;
        {
            // One lock and one flush for the whole run.
            let mut stderr = BufWriter::new(io::stderr().lock());
            let outcomes = outcomes_in_order(&files, |file| check_file(&file.path, file.target));
            for rendered in outcomes.flatten() {
                has_errors = true;
                let _unused = stderr.write_all(&rendered);
            }
        }

        if has_errors {
            return Err(Exit::Failure);
        }

        if verbosity != Verbosity::Quiet {
            eprintln!("ok: {} file(s) checked", files.len());
        }

        Ok(())
    }
}

fn check_file(file_path: &Path, target: LuaTarget) -> Option<Vec<u8>> {
    let source = match read_source_file(file_path) {
        Ok(text) => text,
        Err(error) => {
            return Some(
                format!("error: cannot read `{}`: {error}\n", file_path.display()).into_bytes(),
            );
        }
    };

    let version = target.lua_version();
    let mut result = luck_parser::parse(source, version);
    if result.errors.is_empty() {
        // The compile-time errors real Lua reports beyond the grammar, such as
        // const writes and unresolved gotos. Only `check` pays for this pass.
        result.errors = luck_parser::validate(&result.block, version);
    }
    if result.errors.is_empty() {
        return None;
    }

    Some(render_source_errors(
        file_path.to_string_lossy().into_owned(),
        &result.source,
        &result.errors,
        version,
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser as _;

    use super::CheckArgs;
    use crate::cli::{Cli, Command};

    #[test]
    fn check_is_top_level_command_with_paths_and_config() {
        let cli = Cli::try_parse_from(["luck", "check", "src", "main.lua", "-c", "luck.json"])
            .expect("check parses");
        let Command::Check(CheckArgs { paths, config, no_error_on_unmatched_pattern }) =
            cli.command
        else {
            panic!("expected Command::Check");
        };
        assert_eq!(paths, vec!["src".to_owned(), "main.lua".to_owned()]);
        assert_eq!(config, Some(PathBuf::from("luck.json")));
        assert!(!no_error_on_unmatched_pattern);
    }
}
