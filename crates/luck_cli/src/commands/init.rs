use std::fs;
use std::path::Path;

use clap::Args;
use luck_config::CONFIG_FILE_NAME;
use luck_token::Variant;

use crate::exit::{Exit, operation_error};
use crate::verbosity::Verbosity;

#[derive(Args)]
pub(crate) struct InitArgs {
    /// Luau variant, standalone or roblox [default: standalone].
    #[arg(long, value_name = "VARIANT")]
    variant: Option<Variant>,
}

impl InitArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let variant = self.variant.unwrap_or_default();

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
        fs::write(config_path, init_config_content(variant))
            .map_err(|e| operation_error(format!("cannot write `luck.json`: {e}")))?;

        if verbosity != Verbosity::Quiet {
            eprintln!("Created luck.json and src/main.luau");
        }
        Ok(())
    }
}

/// A `luck.json` that names `variant` and builds `src/main.luau`, with a `dev`
/// and a `release` profile.
fn init_config_content(variant: Variant) -> String {
    let variant_name = variant.to_string().to_lowercase();
    format!(
        r#"{{
    "variant": "{variant_name}",
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

    use super::*;

    /// The project the generated `luck.json` loads into.
    fn initialized(variant: Variant) -> (String, Project) {
        let content = init_config_content(variant);
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, &content).expect("write luck.json");
        let project = Project::load(&config_path).expect("generated luck.json should load");
        (content, project)
    }

    #[test]
    fn init_config_names_its_variant_and_a_luau_entry() {
        for variant in Variant::ALL {
            let (content, project) = initialized(variant);
            assert_eq!(project.variant(), variant, "{content}");
            assert!(content.contains("\"entry\": \"src/main.luau\""), "{content}");
        }
    }
}
