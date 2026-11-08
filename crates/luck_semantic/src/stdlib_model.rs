//! The stdlib catalog model: overloaded signatures, typed parameters, colon
//! methods on named shapes, and deprecation on entries, parameters, and
//! constant values.

use alloc::collections::BTreeMap;

use luck_token::{CompactString, LuaVersion, StdlibEnvironment, offset_to_index};

/// A global, namespace member, or shape member.
#[derive(Debug, Clone)]
pub enum StdlibEntry {
    Function(Box<StdlibFunction>),
    Constant(StdlibValue),
    Property(StdlibValue),
    Namespace(StdlibNamespace),
}

impl StdlibEntry {
    /// Entry-level deprecation, regardless of entry kind.
    #[must_use]
    pub fn deprecation(&self) -> Option<&StdlibDeprecation> {
        match self {
            Self::Function(function) => function.deprecated.as_ref(),
            Self::Constant(value) | Self::Property(value) => value.deprecated.as_ref(),
            Self::Namespace(namespace) => namespace.deprecated.as_ref(),
        }
    }

    /// Whether assigning to this entry overwrites a builtin. A namespace is
    /// always read-only, even when some members (`package.path`) aren't.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        match self {
            Self::Function(function) => function.is_read_only,
            Self::Constant(value) | Self::Property(value) => value.is_read_only,
            Self::Namespace(_) => true,
        }
    }

    /// The shape of the value this entry names. A function's value has
    /// none; `returns_shape` is the shape of its call's result.
    #[must_use]
    pub const fn shape(&self) -> Option<&CompactString> {
        match self {
            Self::Constant(value) | Self::Property(value) => value.shape.as_ref(),
            Self::Namespace(namespace) => namespace.shape.as_ref(),
            Self::Function(_) => None,
        }
    }
}

/// A stdlib function: one or more signatures plus call-site metadata.
/// Boxed in `StdlibEntry` to keep the enum small.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent flags, and all sixteen combinations are legal"
)]
pub struct StdlibFunction {
    /// At least one, in catalog order. The first is the primary form,
    /// shown where only one fits.
    pub signatures: Vec<StdlibSignature>,
    pub is_pure: bool,
    pub is_must_use: bool,
    pub deprecated: Option<StdlibDeprecation>,
    pub is_read_only: bool,
    /// Colon-callable on its shape. No signature counts `self` among its
    /// params.
    pub is_method: bool,
    /// Shape of the call's first return value, if it has one (e.g.
    /// `io.open` returns `file`).
    pub returns_shape: Option<CompactString>,
}

impl StdlibFunction {
    /// Minimum arity across all signatures.
    #[must_use]
    pub fn min_args(&self) -> usize {
        self.signatures.iter().map(|signature| signature.min_args).min().unwrap_or(0)
    }

    /// Maximum arity across all signatures; `None` if any is unbounded.
    #[must_use]
    pub fn max_args(&self) -> Option<usize> {
        let mut max = 0;
        for signature in &self.signatures {
            max = max.max(signature.max_args?);
        }
        Some(max)
    }

    /// Whether any signature accepts a call with `count` arguments.
    #[must_use]
    pub fn accepts_arg_count(&self, count: usize) -> bool {
        self.signatures.iter().any(|signature| signature.accepts(count))
    }

    /// Signatures that accept a call with `count` arguments.
    pub fn matching_signatures(&self, count: usize) -> impl Iterator<Item = &StdlibSignature> {
        self.signatures.iter().filter(move |signature| signature.accepts(count))
    }

    /// The constants that parameter `index` allows in each signature that
    /// accepts a call with `count` arguments, one list per signature in
    /// catalog order.
    ///
    /// Returns `None` when no signature accepts `count`, or when one of them
    /// takes anything but a constant there, which leaves the argument
    /// unconstrained.
    #[must_use]
    pub fn allowed_constants(&self, index: usize, count: usize) -> Option<Vec<&[StdlibConstant]>> {
        let sets: Vec<&[StdlibConstant]> = self
            .matching_signatures(count)
            .map(|signature| {
                let StdlibArgKind::Constant(values) = &signature.params.get(index)?.kind else {
                    return None;
                };
                Some(values.as_slice())
            })
            .collect::<Option<_>>()?;
        (!sets.is_empty()).then_some(sets)
    }

    /// The primary (first) signature.
    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "every catalog function is loaded with at least one signature"
    )]
    pub fn primary_signature(&self) -> &StdlibSignature {
        &self.signatures[0]
    }

    /// Index of the signature to highlight when the cursor sits on
    /// parameter `active_param`: the first signature with a slot at
    /// that position, else the last.
    #[must_use]
    pub fn signature_index_for_active_param(&self, active_param: usize) -> usize {
        self.signatures
            .iter()
            .position(|signature| {
                active_param < signature.params.len()
                    || signature
                        .params
                        .last()
                        .is_some_and(|param| matches!(param.kind, StdlibArgKind::Vararg))
            })
            .unwrap_or_else(|| self.signatures.len().saturating_sub(1))
    }
}

#[derive(Debug, Clone)]
pub struct StdlibSignature {
    pub params: Vec<StdlibParam>,
    pub min_args: usize,
    /// `None` = unbounded (trailing vararg).
    pub max_args: Option<usize>,
}

impl StdlibSignature {
    #[must_use]
    pub fn accepts(&self, count: usize) -> bool {
        count >= self.min_args && self.max_args.is_none_or(|max| count <= max)
    }
}

#[derive(Debug, Clone)]
pub struct StdlibParam {
    pub kind: StdlibArgKind,
    pub is_required: bool,
    /// Set when passing this parameter at all is deprecated (e.g. the
    /// `parent` argument of `Instance.new`).
    pub deprecated: Option<StdlibDeprecation>,
}

#[derive(Debug, Clone)]
pub enum StdlibArgKind {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    /// A type with no variant of its own, e.g. "file" or "buffer". The
    /// string is only a display label.
    Display(CompactString),
    /// A fixed set of string literals, e.g. the options of
    /// `collectgarbage`.
    Constant(Vec<StdlibConstant>),
    /// `...`, matching zero or more values of any type.
    Vararg,
}

/// One allowed value of a `Constant`-kind parameter. Individual values
/// can be deprecated (e.g. `collectgarbage("setpause")` in 5.4).
#[derive(Debug, Clone)]
pub struct StdlibConstant {
    pub value: CompactString,
    pub deprecated: Option<StdlibDeprecation>,
    /// A curated common value, which completion lists first (e.g.
    /// `Players` among 300+ Roblox services).
    pub is_common: bool,
}

#[derive(Debug, Clone)]
pub struct StdlibDeprecation {
    pub message: CompactString,
    /// Auto-fix replacement; see [`expand_replace_template`] for its
    /// placeholders.
    pub replace_template: Option<CompactString>,
}

#[derive(Debug, Clone)]
pub struct StdlibValue {
    pub is_read_only: bool,
    pub deprecated: Option<StdlibDeprecation>,
    /// The value's shape, if it has one (e.g. `game` is a `DataModel`,
    /// `io.stdout` a `file`).
    pub shape: Option<CompactString>,
}

/// A dotted-member container. `shape` is set when the namespace is also a
/// value with methods, as each `Enum.<Type>` is an `Enum` with
/// `GetEnumItems`.
#[derive(Debug, Clone)]
pub struct StdlibNamespace {
    pub members: BTreeMap<CompactString, StdlibEntry>,
    pub shape: Option<CompactString>,
    pub deprecated: Option<StdlibDeprecation>,
}

/// The members of a kind of value, such as a file handle, a string, or a
/// Roblox instance. Members with `is_method` are colon-callable; the rest
/// are dot accesses.
#[derive(Debug, Clone)]
pub struct StdlibShape {
    pub members: BTreeMap<CompactString, StdlibEntry>,
}

/// Top-level library: every global and shape of one environment.
#[derive(Debug, Clone)]
pub struct StdlibLibrary {
    pub version: LuaVersion,
    pub environment: StdlibEnvironment,
    pub globals: BTreeMap<CompactString, StdlibEntry>,
    pub shapes: BTreeMap<CompactString, StdlibShape>,
}

impl StdlibLibrary {
    /// Resolve a dotted path like `["table", "concat"]` to its entry.
    ///
    /// Each step goes through a namespace's members or the shape of a
    /// property or constant, so `["game", "GetService"]` resolves through
    /// `game`'s shape.
    #[must_use]
    pub fn lookup(&self, path: &[&str]) -> Option<&StdlibEntry> {
        let (root, members) = path.split_first()?;
        let mut current = self.globals.get(*root)?;
        for segment in members {
            current = self.child(current, segment)?;
        }
        Some(current)
    }

    /// The member reachable from `entry` under `segment`: a namespace
    /// member, or a member of the entry's declared value shape.
    #[must_use]
    pub fn child<'library>(
        &'library self,
        entry: &'library StdlibEntry,
        segment: &str,
    ) -> Option<&'library StdlibEntry> {
        if let StdlibEntry::Namespace(namespace) = entry
            && let Some(member) = namespace.members.get(segment)
        {
            return Some(member);
        }
        // A shaped namespace is also a value, so `Enum.Material` has the
        // `Enum` shape's methods as well as its items.
        self.shape_member(entry.shape()?, segment)
    }

    #[must_use]
    pub fn shape_member(&self, shape: &str, name: &str) -> Option<&StdlibEntry> {
        self.shapes.get(shape)?.members.get(name)
    }
}

/// Expand a deprecation's `replace_template` with the call's argument
/// source text.
///
/// `%1` through `%9` stand for `args[0]` through `args[8]`, and `%%` is a
/// literal percent. A missing argument expands to nothing; the caller
/// decides whether the fix is still useful.
#[must_use]
pub fn expand_replace_template(template: &str, args: &[&str]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                output.push('%');
                chars.next();
            }
            Some(digit) if digit.is_ascii_digit() => {
                chars.next();
                if let Some(argument) = digit
                    .to_digit(10)
                    .and_then(|position| position.checked_sub(1))
                    .and_then(|index| args.get(offset_to_index(index)))
                {
                    output.push_str(argument);
                }
            }
            _ => output.push('%'),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_template_substitutes_numbered_args() {
        assert_eq!(expand_replace_template("#%1", &["t"]), "#t");
        assert_eq!(expand_replace_template("%1 ^ %2", &["a", "b"]), "a ^ b");
        assert_eq!(expand_replace_template("load(%1)", &["x"]), "load(x)");
    }

    #[test]
    fn replace_template_unescapes_double_percent() {
        assert_eq!(expand_replace_template("100%% %1", &["x"]), "100% x");
    }

    #[test]
    fn replace_template_expands_missing_arg_to_nothing() {
        assert_eq!(expand_replace_template("%1 + %2", &["a"]), "a + ");
    }
}
