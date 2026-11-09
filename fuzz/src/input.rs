use luck_token::LuaVersion;

/// The source text and target version a fuzz input encodes, or `None` when
/// it isn't UTF-8. The input's length picks the version.
#[must_use]
pub fn decode_input(data: &[u8]) -> Option<(&str, LuaVersion)> {
    let source = std::str::from_utf8(data).ok()?;
    Some((source, LuaVersion::ALL[data.len() % LuaVersion::ALL.len()]))
}
