use core::fmt;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::de::{Deserialize, Deserializer, Error, Visitor};

/// How deeply a glob's `{` groups may nest. `globset` recurses once per level
/// while it parses, and no real pattern comes close.
const MAX_BRACE_DEPTH: usize = 64;

/// A `luck.json` glob, matched against a path relative to the config
/// directory. `*` stops at `/` and `**/` crosses directories, and a leading
/// `./` names that directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Glob(globset::Glob);

impl Glob {
    pub(crate) fn pattern(&self) -> &str {
        self.0.glob()
    }
}

// The error comes from inside the string, so a format that tracks positions
// places it on the string.
impl<'de> Deserialize<'de> for Glob {
    fn deserialize<Source>(deserializer: Source) -> Result<Self, Source::Error>
    where
        Source: Deserializer<'de>,
    {
        deserializer.deserialize_str(GlobVisitor)
    }
}

struct GlobVisitor;

impl Visitor<'_> for GlobVisitor {
    type Value = Glob;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a glob")
    }

    fn visit_str<Failure>(self, v: &str) -> Result<Self::Value, Failure>
    where
        Failure: Error,
    {
        parse(v.strip_prefix("./").unwrap_or(v)).map(Glob).map_err(Failure::custom)
    }
}

/// `*` stops at `/`, as in `include` and `exclude`.
///
/// # Errors
///
/// Returns a message naming the first invalid glob pattern.
pub fn glob_set(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(parse(pattern)?);
    }
    builder.build().map_err(|e| format!("invalid glob set: {e}"))
}

/// # Errors
///
/// Returns a message naming the first pattern in `globs` that doesn't build.
pub(crate) fn compiled<'glob>(
    globs: impl IntoIterator<Item = &'glob Glob>,
) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        builder.add(glob.0.clone());
    }
    builder.build().map_err(|e| format!("invalid glob set: {e}"))
}

/// Invalid patterns are hard errors, like unknown config keys, so a typo'd
/// exclude can't quietly let generated code through.
///
/// # Errors
///
/// Returns a message naming `pattern` when it isn't a valid glob.
fn parse(pattern: &str) -> Result<globset::Glob, String> {
    if brace_depth(pattern) > MAX_BRACE_DEPTH {
        return Err(format!(
            "invalid glob pattern `{pattern}`: `{{` groups nest more than {MAX_BRACE_DEPTH} levels \
             deep"
        ));
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| format!("invalid glob pattern `{pattern}`: {e}"))
}

fn brace_depth(pattern: &str) -> usize {
    let mut depth: usize = 0;
    let mut deepest = 0;
    for b in pattern.bytes() {
        match b {
            b'{' => {
                depth = depth.saturating_add(1);
                deepest = deepest.max(depth);
            }
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

#[cfg(test)]
mod tests {
    use serde::de::IntoDeserializer as _;
    use serde::de::value::Error;

    use super::*;

    /// # Errors
    ///
    /// Returns the message `pattern` fails to deserialize with.
    fn glob(pattern: &str) -> Result<Glob, String> {
        Glob::deserialize(pattern.into_deserializer()).map_err(|e: Error| e.to_string())
    }

    #[test]
    fn a_leading_dot_slash_names_the_config_directory() {
        let set = compiled([&glob("./src/*.lua").unwrap()]).unwrap();
        assert!(set.is_match("src/a.lua"));
        assert!(!set.is_match("src/nested/a.lua"));
    }

    #[test]
    fn an_invalid_pattern_is_an_error_that_names_it() {
        let error = glob("src/{a,b").unwrap_err();
        assert!(error.starts_with("invalid glob pattern `src/{a,b`"), "{error}");
        let nested = format!("{}a{}", "{".repeat(100), "}".repeat(100));
        assert!(glob(&nested).unwrap_err().contains("nest more than 64 levels"));
    }
}
