//! `luck init` - scaffold a fresh `luck.json` and `src/main.{lua,luau}`.

use std::fs;
use std::path::Path;

use clap::Args;
use luck_core::LuaTarget;

use crate::exit::Exit;
use crate::output::{operation_error, usage_error};

#[derive(Args)]
pub(crate) struct InitArgs {
    /// Lua target [default: Lua54].
    #[arg(short = 't', long = "target", value_name = "TARGET")]
    target: Option<String>,
}

impl InitArgs {
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] on a bad target, and
    /// [`Exit::Failure`] when `luck.json` exists or a scaffold file can't be
    /// written.
    pub(crate) fn run(self) -> Result<(), Exit> {
        let target: LuaTarget = match self.target.as_deref() {
            Some(name) => name.parse().map_err(usage_error)?,
            None => LuaTarget::Lua54,
        };

        let config_path = Path::new("luck.json");
        if config_path.exists() {
            return Err(operation_error("`luck.json` already exists"));
        }

        // Any Luau dialect gets a `.luau` entry, everything else `.lua`.
        let is_luau = target.lua_version().is_luau();
        let extension = if is_luau { "luau" } else { "lua" };
        let content = init_config_content(target);

        if let Err(error) = fs::write(config_path, &content) {
            eprintln!("Error: cannot write `luck.json`: {error}");
            return Err(Exit::Failure);
        }

        let source_directory = Path::new("src");
        if !source_directory.exists()
            && let Err(error) = fs::create_dir_all(source_directory)
        {
            eprintln!("Error: cannot create `src` directory: {error}");
            return Err(Exit::Failure);
        }

        let entry_file = source_directory.join(format!("main.{extension}"));
        if !entry_file.exists()
            && let Err(error) = fs::write(&entry_file, "print(\"hello world\")\n")
        {
            eprintln!("Error: cannot write `{}`: {error}", entry_file.display());
            return Err(Exit::Failure);
        }

        eprintln!("Created luck.json and src/main.{extension}");
        Ok(())
    }
}

/// Build the scaffolded `luck.json` contents for a fresh project.
///
/// Both the `lua` and `luau` keys are set. The chosen target fills its own key
/// (lowercased, e.g. `lua53` or `roblox`) and the other keeps its default
/// (`lua54` or `luau`). A Luau target gets a `.luau` entry and no
/// `search_paths`.
fn init_config_content(target: LuaTarget) -> String {
    let target_name = target.to_string().to_lowercase();
    let is_luau = target.lua_version().is_luau();
    let (axis_lua, axis_luau) =
        if is_luau { ("lua54", target_name.as_str()) } else { (target_name.as_str(), "luau") };

    let extension = if is_luau { "luau" } else { "lua" };
    let search_paths_line = if is_luau {
        String::new()
    } else {
        format!("\n    \"search_paths\": [\"src/?.{extension}\", \"src/?/init.{extension}\"],")
    };

    format!(
        r#"{{
    "lua": "{axis_lua}",
    "luau": "{axis_luau}",
    "entry": "src/main.{extension}",
    "output_dir": "dist",{search_paths_line}
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
    use luck_core::parse_luck_config;

    use super::*;

    #[test]
    fn init_config_default_sets_both_axes() {
        let content = init_config_content(LuaTarget::Lua54);
        let config = parse_luck_config(&content).expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert!(content.contains("\"entry\": \"src/main.lua\""));
        assert!(content.contains("search_paths"));
    }

    #[test]
    fn init_config_lua_dialect_sets_lua_axis_only() {
        let content = init_config_content(LuaTarget::Lua53);
        let config = parse_luck_config(&content).expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua53"));
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert!(content.contains("\"entry\": \"src/main.lua\""));
    }

    #[test]
    fn init_config_roblox_sets_luau_axis_and_luau_entry() {
        let content = init_config_content(LuaTarget::LuauRoblox);
        let config = parse_luck_config(&content).expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.luau.as_deref(), Some("roblox"));
        assert!(content.contains("\"entry\": \"src/main.luau\""));
        // Luau projects have no Lua-style search paths.
        assert!(!content.contains("search_paths"));
    }
}
