use std::borrow::Cow;

use crate::{SecurityLevel, Variant};

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

/// The definitions `variant` adds to Luau's built-in ones for a script at
/// `security`, as Luau's `loadDefinitionFile` reads them, or `None` for
/// standalone Luau, which adds none.
pub fn definitions_for(variant: Variant, security: SecurityLevel) -> Option<Cow<'static, str>> {
    if !variant.is_roblox() {
        return None;
    }
    if security == SecurityLevel::RobloxScriptSecurity {
        return Some(Cow::Borrowed(ROBLOX));
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
    Some(Cow::Owned(text))
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
