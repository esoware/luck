use std::fs;
use std::path::Path;

use clap::Args;
use luck_config::CONFIG_FILE_NAME;

use crate::exit::{Exit, operation_error};
use crate::verbosity::Verbosity;

#[derive(Args)]
pub(crate) struct InitArgs {
    /// Load Roblox's API, as `@roblox` in `definitions`.
    #[arg(long)]
    roblox: bool,
}

impl InitArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let config_path = Path::new(CONFIG_FILE_NAME);
        if config_path.exists() {
            return Err(operation_error("`luck.json` already exists"));
        }

        let source_directory = Path::new("src");
        fs::create_dir_all(source_directory)
            .map_err(|e| operation_error(format!("cannot create `src` directory: {e}")))?;

        let entry_file = source_directory.join("main.luau");
        if !entry_file.exists() {
            fs::write(&entry_file, "print(\"hello world\")\n").map_err(|e| {
                operation_error(format!("cannot write `{}`: {e}", entry_file.display()))
            })?;
        }

        // Last, because an existing `luck.json` is what blocks another `init`.
        fs::write(config_path, init_config_content(self.roblox))
            .map_err(|e| operation_error(format!("cannot write `luck.json`: {e}")))?;

        if verbosity != Verbosity::Quiet {
            eprintln!("Created luck.json and src/main.luau");
        }
        Ok(())
    }
}

/// A `luck.json` that loads Roblox's API when `has_roblox` and builds
/// `src/main.luau`, with a `dev` and a `release` profile.
fn init_config_content(has_roblox: bool) -> String {
    let definitions = if has_roblox { "\n    \"definitions\": [\"@roblox\"]," } else { "" };
    format!(
        r#"{{{definitions}
    "entry": "src/main.luau",
    "output_dir": "dist",
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

#[cfg(test)]
mod tests {
    use luck_config::Project;
    use luck_resolver::FileSystemOs;

    use super::*;

    /// The project the generated `luck.json` loads into.
    fn initialized(has_roblox: bool) -> (String, Project) {
        let content = init_config_content(has_roblox);
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, &content).expect("write luck.json");
        let project =
            Project::load(&config_path, FileSystemOs).expect("generated luck.json should load");
        (content, project)
    }

    #[test]
    fn init_config_loads_roblox_s_api_when_asked_and_builds_a_luau_entry() {
        for has_roblox in [false, true] {
            let (content, project) = initialized(has_roblox);
            assert_eq!(project.stdlib().library().has_roblox, has_roblox, "{content}");
            assert!(content.contains("\"entry\": \"src/main.luau\""), "{content}");
        }
    }
}
