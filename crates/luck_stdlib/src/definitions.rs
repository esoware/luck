use std::borrow::Cow;

use crate::SecurityLevel;

/// Luau's standard library as a definition file, as `just stdlib` generates
/// it.
pub(crate) const LUAU: &str = include_str!("../stdlib_data/luau.luau");

/// Roblox's API as a Luau definition file at `RobloxScriptSecurity`. A member
/// a weaker level can't reach ends with a comment naming the level it needs,
/// as `-- PluginSecurity`, and a property a weaker level can only read names
/// the level that writes it, as `-- write: PluginSecurity`. The line before a
/// member is its summary when it's a comment.
#[expect(
    clippy::large_include_file,
    reason = "Roblox's whole API is one definition file, which a checker loads whole"
)]
const ROBLOX: &str = include_str!("../stdlib_data/roblox.luau");

/// Roblox's API for a script at `security`, the definitions a project's
/// `@roblox` loads over Luau's built-in ones, as Luau's `loadDefinitionFile`
/// reads them.
pub fn roblox_definitions(security: SecurityLevel) -> Cow<'static, str> {
    if security == SecurityLevel::RobloxScriptSecurity {
        return Cow::Borrowed(ROBLOX);
    }
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
    Cow::Owned(text)
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
