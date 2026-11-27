use std::sync::OnceLock;

use crate::SecurityLevel;

/// Luau's standard library as a definition file, as `just stdlib` generates
/// it: Luau's built-in definitions, with a declaration for each function they
/// leave to Luau's C++, and the library reference's description above each
/// declaration it documents.
pub const fn luau_definitions() -> &'static str {
    include_str!("../stdlib_data/luau.d.luau")
}

/// Roblox's API as a Luau definition file at `RobloxScriptSecurity`. A member
/// a weaker level can't reach ends with a comment naming the level it needs,
/// as `-- PluginSecurity`, and a property a weaker level can only read names
/// the level that writes it, as `-- write: PluginSecurity`. The line before a
/// member is its summary when it's a comment.
#[expect(
    clippy::large_include_file,
    reason = "Roblox's whole API is one definition file, which a checker loads whole"
)]
const ROBLOX: &str = include_str!("../stdlib_data/roblox.d.luau");

/// Roblox's API at each level of [`SecurityLevel::ALL`] below
/// `RobloxScriptSecurity`.
static FILTERED: [OnceLock<String>; SecurityLevel::ALL.len() - 1] =
    [const { OnceLock::new() }; SecurityLevel::ALL.len() - 1];

/// Roblox's API for a script at `security`, the definitions a project's
/// `@roblox` loads over Luau's built-in ones, as Luau's `loadDefinitionFile`
/// reads them, filtered the first time a process asks for its level.
pub fn roblox_definitions(security: SecurityLevel) -> &'static str {
    if security == SecurityLevel::RobloxScriptSecurity {
        return ROBLOX;
    }
    FILTERED[security as usize].get_or_init(|| filter(security))
}

fn filter(security: SecurityLevel) -> String {
    let mut text = String::with_capacity(ROBLOX.len());
    let mut last_line_start = 0;
    for line in ROBLOX.split_inclusive('\n') {
        let indented = line.trim_start();
        let levels = line
            .rsplit_once(" -- ")
            .filter(|_| !indented.starts_with("--"))
            .and_then(|(_, comment)| Levels::parse(comment.trim_end()));
        match levels {
            Some(levels) if levels.access > security => {
                if text[last_line_start..].trim_start().starts_with("--") {
                    text.truncate(last_line_start);
                }
            }
            Some(levels) if levels.write.is_some_and(|write| write > security) => {
                last_line_start = text.len();
                text.push_str(&line[..line.len() - indented.len()]);
                text.push_str("read ");
                text.push_str(indented);
            }
            _ => {
                last_line_start = text.len();
                text.push_str(line);
            }
        }
    }
    text
}

/// What a member's trailing comment says it needs.
struct Levels {
    access: SecurityLevel,
    write: Option<SecurityLevel>,
}

impl Levels {
    /// `PluginSecurity`, `write: PluginSecurity`, or both, comma separated.
    fn parse(comment: &str) -> Option<Self> {
        let mut levels = Self { access: SecurityLevel::None, write: None };
        for part in comment.split(", ") {
            match part.strip_prefix("write: ") {
                Some(write) => levels.write = Some(SecurityLevel::from_name(write)?),
                None => levels.access = SecurityLevel::from_name(part)?,
            }
        }
        Some(levels)
    }
}
