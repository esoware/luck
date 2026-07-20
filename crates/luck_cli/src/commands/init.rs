//! `luck init` - scaffold a fresh `luck.json` and `src/main.{lua,luau}`.

use crate::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE};
use clap::Args;
use luck_core::types::LuaTarget;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Args)]
pub(crate) struct InitArgs {
    /// Lua target [default: Lua54]
    #[arg(short = 't', long = "target", value_name = "TARGET")]
    target: Option<String>,
}

impl InitArgs {
    pub(crate) fn run(self) -> ExitCode {
        let target: LuaTarget = match self.target.as_deref() {
            Some(target) => match target.parse() {
                Ok(target) => target,
                Err(error) => {
                    eprintln!("Error: {error}");
                    return ExitCode::from(EXIT_USAGE);
                }
            },
            None => LuaTarget::Lua54,
        };

        let config_path = PathBuf::from("luck.json");
        if config_path.exists() {
            eprintln!("Error: luck.json already exists");
            return ExitCode::from(EXIT_FAILURE);
        }

        // The scaffolded entry file and search paths follow the chosen target's
        // family; `.luau` for any Luau dialect, `.lua` otherwise.
        let ext = if target.is_luau() { "luau" } else { "lua" };
        let content = init_config_content(target);

        if let Err(error) = std::fs::write(&config_path, &content) {
            eprintln!("Error writing luck.json: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }

        let src_dir = PathBuf::from("src");
        if !src_dir.exists()
            && let Err(error) = std::fs::create_dir_all(&src_dir)
        {
            eprintln!("Error creating src directory: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }

        let entry_file = src_dir.join(format!("main.{ext}"));
        if !entry_file.exists()
            && let Err(error) = std::fs::write(&entry_file, "print(\"hello world\")\n")
        {
            eprintln!("Error writing {}: {error}", entry_file.display());
            return ExitCode::from(EXIT_FAILURE);
        }

        eprintln!("Created luck.json and src/main.{ext}");
        ExitCode::from(EXIT_SUCCESS)
    }
}

/// Build the scaffolded `luck.json` contents for a fresh project.
///
/// The config model is per-extension: `.lua` files use the `lua` axis,
/// `.luau` files use the `luau` axis. A complete scaffold sets both. The
/// chosen target overrides its own axis (lowercased canonical, e.g. `lua53`
/// or `roblox`); the other axis keeps its default (`lua54` / `luau`). The
/// `entry` and `search_paths` follow the chosen target's family.
fn init_config_content(target: LuaTarget) -> String {
    let mut lua_value = "lua54".to_string();
    let mut luau_value = "luau".to_string();
    if target.is_luau() {
        luau_value = target.to_string().to_lowercase();
    } else {
        lua_value = target.to_string().to_lowercase();
    }

    let ext = if target.is_luau() { "luau" } else { "lua" };
    let search_paths_line = if target.is_luau() {
        String::new()
    } else {
        format!("\n    \"search_paths\": [\"src/?.{ext}\", \"src/?/init.{ext}\"],")
    };

    format!(
        r#"{{
    "lua": "{lua_value}",
    "luau": "{luau_value}",
    "entry": "src/main.{ext}",
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
    use super::*;

    #[test]
    fn init_config_default_sets_both_axes() {
        let content = init_config_content(LuaTarget::Lua54);
        let config = luck_core::config::parse_luck_config(&content)
            .expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert!(content.contains("\"entry\": \"src/main.lua\""));
        assert!(content.contains("search_paths"));
    }

    #[test]
    fn init_config_lua_dialect_sets_lua_axis_only() {
        let content = init_config_content(LuaTarget::Lua53);
        let config = luck_core::config::parse_luck_config(&content)
            .expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua53"));
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert!(content.contains("\"entry\": \"src/main.lua\""));
    }

    #[test]
    fn init_config_roblox_sets_luau_axis_and_luau_entry() {
        let content = init_config_content(LuaTarget::LuauRoblox);
        let config = luck_core::config::parse_luck_config(&content)
            .expect("generated luck.json should parse");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.luau.as_deref(), Some("roblox"));
        assert!(content.contains("\"entry\": \"src/main.luau\""));
        // Luau projects have no Lua-style search paths.
        assert!(!content.contains("search_paths"));
    }
}
