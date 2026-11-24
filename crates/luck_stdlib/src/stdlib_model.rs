use core::iter::Map;
use core::slice;

use luck_allocator::Allocator;
use luck_token::{LuaTarget, offset_to_index};

/// Library entries by name, in name order, with a hash table over them,
/// built at compile time for a built-in library and in its arena for a
/// layered one.
#[derive(Debug, Clone, Copy)]
pub struct StdlibMap<'lib, V> {
    entries: &'lib [(&'lib str, V)],
    /// Open addressing by [`name_hash`]: each slot holds an entry's index plus
    /// one, or zero when it's empty, and at least half of them are.
    slots: &'lib [u16],
}

type Entries<'lib, V> =
    Map<slice::Iter<'lib, (&'lib str, V)>, fn(&'lib (&'lib str, V)) -> (&'lib str, &'lib V)>;

/// A static [`StdlibMap`] of `$value`s from `(name, value)` entries sorted by
/// name, with no name twice.
///
/// The entries are a `static`, since a value may point to another `static`,
/// as a constant set does.
macro_rules! stdlib_map {
    ($value:ty, [$($entry:expr),* $(,)?]) => {{
        const COUNT: usize = <[()]>::len(&[$($crate::stdlib_model::stdlib_map!(@unit $entry)),*]);
        static ENTRIES: [(&str, $value); COUNT] = [$($entry),*];
        static SLOTS: [u16; $crate::stdlib_model::slot_count(COUNT)] =
            $crate::stdlib_model::hash_slots(&ENTRIES);
        $crate::stdlib_model::StdlibMap::new(&ENTRIES, &SLOTS)
    }};
    (@unit $entry:expr) => {
        ()
    };
}

pub(crate) use stdlib_map;

/// The most entries a [`StdlibMap`] holds, since a `u16` slot holds an
/// entry's index plus one.
pub(crate) const MAX_MAP_ENTRIES: usize = 0xFFFF;

/// FNV-1a, which both compile-time table building and lookups run.
#[expect(clippy::as_conversions, reason = "`u32::from` isn't const")]
const fn name_hash(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    let mut remaining = name.as_bytes();
    while let [b, rest @ ..] = remaining {
        hash = (hash ^ *b as u32).wrapping_mul(0x0100_0193);
        remaining = rest;
    }
    hash
}

pub(crate) const fn slot_count(entry_count: usize) -> usize {
    entry_count.saturating_mul(2).next_power_of_two()
}

pub(crate) const fn hash_slots<V, const SLOTS: usize>(entries: &[(&str, V)]) -> [u16; SLOTS] {
    let mut slots = [0; SLOTS];
    fill_slots(entries, &mut slots);
    slots
}

/// Fills `slots`, [`slot_count`] of them, all zero, with the hash table over
/// `entries`.
///
/// # Panics
///
/// When `entries` holds more than [`MAX_MAP_ENTRIES`], which fails the build
/// for a static map, and which a layered map is checked against first.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing,
    reason = "the checked conversions aren't const, and `slot_count` makes every masked slot an \
              index"
)]
const fn fill_slots<V>(entries: &[(&str, V)], slots: &mut [u16]) {
    assert!(entries.len() <= MAX_MAP_ENTRIES, "a stdlib table has too many entries for its slots");
    let mask = slots.len().wrapping_sub(1);
    let mut index = 0usize;
    let mut remaining = entries;
    while let [(name, _), rest @ ..] = remaining {
        index = index.wrapping_add(1);
        let mut slot = name_hash(name) as usize & mask;
        while slots[slot] != 0 {
            slot = slot.wrapping_add(1) & mask;
        }
        slots[slot] = index as u16;
        remaining = rest;
    }
}

impl<'lib, V> StdlibMap<'lib, V> {
    pub(crate) const EMPTY: Self = Self { entries: &[], slots: &[] };

    /// `slots` is [`hash_slots`] of `entries`, which [`stdlib_map`] builds.
    pub(crate) const fn new(entries: &'lib [(&'lib str, V)], slots: &'lib [u16]) -> Self {
        Self { entries, slots }
    }

    /// The map of `entries`, sorted by name with no name twice and at most
    /// [`MAX_MAP_ENTRIES`] of them, in `allocator`.
    pub(crate) fn new_in(allocator: &'lib Allocator, sorted: &[(&'lib str, V)]) -> Self
    where
        V: Copy,
    {
        let entries = allocator.alloc_slice_copy(sorted);
        let mut slots = vec![0; slot_count(entries.len())];
        fill_slots(entries, &mut slots);
        Self { entries, slots: allocator.alloc_slice_copy(&slots) }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'lib V> {
        let mask = self.slots.len().wrapping_sub(1);
        let mut slot = offset_to_index(name_hash(name)) & mask;
        // An empty slot ends every probe, since at least half are empty.
        loop {
            let entry = self.slots.get(slot)?.checked_sub(1)?;
            if let Some((entry_name, value)) = self.entries.get(usize::from(entry))
                && *entry_name == name
            {
                return Some(value);
            }
            slot = slot.wrapping_add(1) & mask;
        }
    }

    #[must_use]
    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn iter(&self) -> Entries<'lib, V> {
        let pair: fn(&'lib (&'lib str, V)) -> (&'lib str, &'lib V) = |(name, value)| (*name, value);
        self.entries.iter().map(pair)
    }

    pub fn keys(&self) -> impl Iterator<Item = &'lib str> {
        self.entries.iter().map(|(name, _)| *name)
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<'lib, V> IntoIterator for &StdlibMap<'lib, V> {
    type Item = (&'lib str, &'lib V);
    type IntoIter = Entries<'lib, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Each variant holds a reference, so an entry is two words, and a table of
/// them stays dense for the lookups that scan it.
#[derive(Debug, Clone, Copy)]
pub enum StdlibEntry<'lib> {
    Function(&'lib StdlibFunction<'lib>),
    Constant(&'lib StdlibValue<'lib>),
    Property(&'lib StdlibValue<'lib>),
    Namespace(&'lib StdlibNamespace<'lib>),
}

impl<'lib> StdlibEntry<'lib> {
    #[must_use]
    pub const fn deprecation(&self) -> Option<&'lib StdlibDeprecation<'lib>> {
        match *self {
            Self::Function(function) => function.deprecated.as_ref(),
            Self::Constant(value) | Self::Property(value) => value.deprecated.as_ref(),
            Self::Namespace(namespace) => namespace.deprecated.as_ref(),
        }
    }

    /// A namespace is always read-only, even when some members (`package.path`) aren't.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        match self {
            Self::Function(function) => function.is_read_only,
            Self::Constant(value) | Self::Property(value) => value.is_read_only,
            Self::Namespace(_) => true,
        }
    }

    /// The shape of the value this entry names. A function's value has
    /// none; `returns_shape` is the shape of its call's result.
    #[must_use]
    pub const fn shape(&self) -> Option<&'lib str> {
        match self {
            Self::Constant(value) | Self::Property(value) => value.shape,
            Self::Namespace(namespace) => namespace.shape,
            Self::Function(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent flags, and all sixteen combinations are legal"
)]
pub struct StdlibFunction<'lib> {
    /// At least one, in definition order. The first is the primary form,
    /// shown where only one fits.
    pub signatures: &'lib [StdlibSignature<'lib>],
    pub is_pure: bool,
    pub is_must_use: bool,
    pub deprecated: Option<StdlibDeprecation<'lib>>,
    pub is_read_only: bool,
    /// Colon-callable on its shape. No signature counts `self` among its
    /// params.
    pub is_method: bool,
    /// Shape of the call's first return value, if it has one (e.g.
    /// `io.open` returns `file`).
    pub returns_shape: Option<&'lib str>,
}

impl<'lib> StdlibFunction<'lib> {
    #[must_use]
    pub fn min_args(&self) -> usize {
        self.signatures.iter().map(|signature| signature.min_args).min().unwrap_or(0)
    }

    /// Maximum arity across all signatures; `None` if any is unbounded.
    #[must_use]
    pub fn max_args(&self) -> Option<usize> {
        let mut max = 0;
        for signature in self.signatures {
            max = max.max(signature.max_args?);
        }
        Some(max)
    }

    #[must_use]
    pub fn accepts_arg_count(&self, count: usize) -> bool {
        self.signatures.iter().any(|signature| signature.accepts(count))
    }

    pub fn matching_signatures(
        &self,
        count: usize,
    ) -> impl Iterator<Item = &'lib StdlibSignature<'lib>> {
        self.signatures.iter().filter(move |signature| signature.accepts(count))
    }

    /// The constants that parameter `index` allows in each signature that
    /// accepts a call with `count` arguments, one list per signature in
    /// definition order.
    ///
    /// Returns `None` when no signature accepts `count`, or when one of them
    /// takes anything but a constant there, which leaves the argument
    /// unconstrained.
    #[must_use]
    pub fn allowed_constants(
        &self,
        index: usize,
        count: usize,
    ) -> Option<impl Iterator<Item = &'lib [StdlibConstant<'lib>]> + Clone> {
        let signatures: &'lib [StdlibSignature<'lib>] = self.signatures;
        let sets = signatures.iter().filter(move |signature| signature.accepts(count)).map(
            move |signature| {
                let StdlibArgKind::Constant(values) = signature.params.get(index)?.kind else {
                    return None;
                };
                Some(values)
            },
        );
        let mut checked = sets.clone();
        let is_constrained =
            checked.next().is_some_and(|set| set.is_some()) && checked.all(|set| set.is_some());
        is_constrained.then(|| sets.flatten())
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "a std definition gives every function at least one signature"
    )]
    pub const fn primary_signature(&self) -> &'lib StdlibSignature<'lib> {
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

/// One form of a function's call: its required params, then its optional
/// ones, then at most one vararg, and the arities [`Self::new`] counts from
/// them.
#[derive(Debug, Clone, Copy)]
pub struct StdlibSignature<'lib> {
    pub params: &'lib [StdlibParam<'lib>],
    pub min_args: usize,
    /// `None` when a trailing vararg leaves it unbounded.
    pub max_args: Option<usize>,
}

impl<'lib> StdlibSignature<'lib> {
    #[must_use]
    pub const fn new(params: &'lib [StdlibParam<'lib>]) -> Self {
        let mut min_args = 0usize;
        let mut max_args = Some(params.len());
        let mut remaining = params;
        while let [param, rest @ ..] = remaining {
            let is_vararg = matches!(param.kind, StdlibArgKind::Vararg);
            if is_vararg {
                max_args = None;
            }
            if param.is_required && !is_vararg {
                min_args = min_args.saturating_add(1);
            }
            remaining = rest;
        }
        Self { params, min_args, max_args }
    }

    #[must_use]
    pub fn accepts(&self, count: usize) -> bool {
        count >= self.min_args && self.max_args.is_none_or(|max| count <= max)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StdlibParam<'lib> {
    pub kind: StdlibArgKind<'lib>,
    /// Never set on a vararg.
    pub is_required: bool,
    /// Set when passing this parameter at all is deprecated (e.g. the
    /// `parent` argument of `Instance.new`).
    pub deprecated: Option<StdlibDeprecation<'lib>>,
}

#[derive(Debug, Clone, Copy)]
pub enum StdlibArgKind<'lib> {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    /// A type with no variant of its own, e.g. "file" or "buffer". The
    /// string is only a display label.
    Display(&'lib str),
    /// A fixed set of string literals, e.g. the options of
    /// `collectgarbage`.
    Constant(&'lib [StdlibConstant<'lib>]),
    /// `...`, matching zero or more values of any type.
    Vararg,
}

/// One allowed value of a `Constant`-kind parameter. Individual values
/// can be deprecated (e.g. `collectgarbage("setpause")` in 5.4).
#[derive(Debug, Clone, Copy)]
pub struct StdlibConstant<'lib> {
    pub value: &'lib str,
    pub deprecated: Option<StdlibDeprecation<'lib>>,
    /// A curated common value, which completion lists first (e.g.
    /// `Players` among 300+ Roblox services).
    pub is_common: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct StdlibDeprecation<'lib> {
    pub message: &'lib str,
    pub replacement: Option<StdlibReplacement<'lib>>,
}

/// The call that replaces a call to a deprecated function.
#[derive(Debug, Clone, Copy)]
pub struct StdlibReplacement<'lib> {
    /// The new call, with placeholders for the old call's arguments, as
    /// [`expand_replace_template`] reads them.
    pub template: &'lib str,
    /// Whether the new call returns what the old one did for every argument
    /// the old one accepts. Only then can a fix make it without asking.
    pub is_exact: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct StdlibValue<'lib> {
    pub is_read_only: bool,
    pub deprecated: Option<StdlibDeprecation<'lib>>,
    /// The value's shape, if it has one (e.g. `game` is a `DataModel`,
    /// `io.stdout` a `file`).
    pub shape: Option<&'lib str>,
}

/// A dotted-member container. `shape` is set when the namespace is also a
/// value with methods, as each `Enum.<Type>` is an `Enum` with
/// `GetEnumItems`.
#[derive(Debug, Clone, Copy)]
pub struct StdlibNamespace<'lib> {
    pub members: StdlibMap<'lib, StdlibEntry<'lib>>,
    pub shape: Option<&'lib str>,
    pub deprecated: Option<StdlibDeprecation<'lib>>,
}

/// The members of a kind of value, such as a file handle, a string, or a
/// Roblox instance. Members with `is_method` are colon-callable; the rest
/// are dot accesses.
#[derive(Debug, Clone, Copy)]
pub struct StdlibShape<'lib> {
    pub members: StdlibMap<'lib, StdlibEntry<'lib>>,
    /// The shape whose members this one has too, under its own, as each
    /// Roblox service has `Instance`'s.
    pub extends: Option<&'lib str>,
}

/// A target's standard library, built in or with std definitions layered
/// over it.
#[derive(Debug, Clone, Copy)]
pub struct StdlibLibrary<'lib> {
    pub target: LuaTarget,
    pub globals: StdlibMap<'lib, StdlibEntry<'lib>>,
    pub shapes: StdlibMap<'lib, StdlibShape<'lib>>,
    /// The lists of constants a std definition's params can name, such as
    /// Roblox's service names.
    pub constant_sets: StdlibMap<'lib, &'lib [StdlibConstant<'lib>]>,
}

impl<'lib> StdlibLibrary<'lib> {
    /// What the generated catalogs layer their files over.
    #[cfg(test)]
    pub(crate) const fn empty(target: LuaTarget) -> Self {
        Self {
            target,
            globals: StdlibMap::EMPTY,
            shapes: StdlibMap::EMPTY,
            constant_sets: StdlibMap::EMPTY,
        }
    }

    /// Each step goes through a namespace's members or the shape of a
    /// property or constant, so `["game", "GetService"]` resolves through
    /// `game`'s shape.
    #[must_use]
    pub fn lookup<Path>(&self, path: Path) -> Option<&'lib StdlibEntry<'lib>>
    where
        Path: IntoIterator,
        Path::Item: AsRef<str>,
    {
        let mut segments = path.into_iter();
        let mut current = self.globals.get(segments.next()?.as_ref())?;
        for segment in segments {
            current = self.child(current, segment.as_ref())?;
        }
        Some(current)
    }

    /// The member reachable from `entry` under `segment`: a namespace
    /// member, or a member of the entry's declared value shape.
    #[must_use]
    pub fn child(
        &self,
        entry: &StdlibEntry<'lib>,
        segment: &str,
    ) -> Option<&'lib StdlibEntry<'lib>> {
        if let StdlibEntry::Namespace(namespace) = entry
            && let Some(member) = namespace.members.get(segment)
        {
            return Some(member);
        }
        // A shaped namespace is also a value, so `Enum.Material` has the
        // `Enum` shape's methods as well as its items.
        self.shape_member(entry.shape()?, segment)
    }

    /// `name` among the members of `shape` or of a shape it extends, the
    /// nearest first.
    #[must_use]
    pub fn shape_member(&self, shape: &str, name: &str) -> Option<&'lib StdlibEntry<'lib>> {
        let mut current = self.shapes.get(shape)?;
        // A chain longer than the shape count is an `extends` cycle, which
        // has nothing left to find.
        for _ in 0..self.shapes.len() {
            if let Some(member) = current.members.get(name) {
                return Some(member);
            }
            current = self.shapes.get(current.extends?)?;
        }
        None
    }

    /// Every member [`Self::shape_member`] finds in `shape`, each name once.
    #[must_use]
    pub fn shape_members(&self, shape: &str) -> Vec<(&'lib str, &'lib StdlibEntry<'lib>)> {
        let mut members = Vec::new();
        let mut nearer: Vec<&'lib StdlibShape<'lib>> = Vec::new();
        let mut current = self.shapes.get(shape);
        while let Some(found) = current
            && nearer.len() < self.shapes.len()
        {
            for (name, member) in &found.members {
                if !nearer.iter().any(|shadowing| shadowing.members.contains_key(name)) {
                    members.push((name, member));
                }
            }
            nearer.push(found);
            current = found.extends.and_then(|base| self.shapes.get(base));
        }
        members
    }
}

/// `%1` through `%9` stand for `args[0]` through `args[8]`, and `%%` is a
/// literal percent.
///
/// A placeholder in a call's argument list expands verbatim,
/// and one before that list's `)` also forwards every later argument, so a
/// fix keeps the arguments the template doesn't spell out. Any other
/// placeholder is an operand, which gets parentheses unless it's a bare
/// operand, so the template's operator applies to the whole argument.
///
/// A missing argument expands to nothing; the caller decides whether the fix
/// is still useful.
///
/// Each argument is source as written, bytes that need not be UTF-8, and so
/// is the expansion.
#[must_use]
pub fn expand_replace_template(template: &str, args: &[&[u8]]) -> Vec<u8> {
    let mut output = Vec::with_capacity(template.len());
    let mut remaining = template.as_bytes();
    while let Some(percent) = remaining.iter().position(|&b| b == b'%') {
        let (literal, from_percent) = remaining.split_at_checked(percent).unwrap_or_default();
        output.extend_from_slice(literal);
        let after_percent = from_percent.get(1..).unwrap_or_default();
        match after_percent.split_first() {
            Some((b'%', after_escape)) => {
                output.push(b'%');
                remaining = after_escape;
            }
            Some((&digit, after_digit)) if digit.is_ascii_digit() => {
                remaining = after_digit;
                let Some(from_argument) = usize::from(digit.wrapping_sub(b'0'))
                    .checked_sub(1)
                    .and_then(|i| args.get(i..))
                else {
                    continue;
                };
                let Some(&arg) = from_argument.first() else {
                    continue;
                };
                let after_placeholder = remaining.trim_ascii_start().first();
                let is_in_argument_list =
                    matches!(output.trim_ascii_end().last(), Some(b'(' | b','))
                        && matches!(after_placeholder, Some(b')' | b','));
                if is_in_argument_list && after_placeholder == Some(&b')') {
                    output.extend_from_slice(&from_argument.join(&b", "[..]));
                } else if is_in_argument_list || is_bare_operand(arg) {
                    output.extend_from_slice(arg);
                } else {
                    output.push(b'(');
                    output.extend_from_slice(arg);
                    output.push(b')');
                }
            }
            _ => {
                output.push(b'%');
                remaining = after_percent;
            }
        }
    }
    output.extend_from_slice(remaining);
    output
}

/// No operator can split one. Anything else gets grouped, which is harmless:
/// a parenthesized call only keeps its first value, and an operand takes one
/// value anyway.
fn is_bare_operand(arg: &[u8]) -> bool {
    !arg.windows(2).any(|pair| pair == b"..")
        && arg.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib_catalog::{FUNCTION, optional, required};

    #[test]
    fn a_map_finds_exactly_its_names() {
        static MAP: StdlibMap<'static, u8> = stdlib_map!(u8, [("b", 1), ("d", 2), ("f", 3)]);
        assert_eq!(MAP.get("d"), Some(&2));
        for absent in ["", "a", "c", "e", "g"] {
            assert_eq!(MAP.get(absent), None, "{absent}");
        }
        assert_eq!(MAP.iter().collect::<Vec<_>>(), [("b", &1), ("d", &2), ("f", &3)]);
    }

    #[test]
    fn a_map_built_in_an_arena_finds_exactly_its_names() {
        let allocator = Allocator::new();
        let map = StdlibMap::new_in(&allocator, &[("b", 1u8), ("d", 2), ("f", 3)]);
        assert_eq!(map.get("f"), Some(&3));
        assert_eq!(map.get("c"), None);
        let empty: StdlibMap<'_, u8> = StdlibMap::EMPTY;
        assert_eq!(empty.get("b"), None);
    }

    #[test]
    fn a_signature_counts_its_required_params_and_a_vararg() {
        const BOUNDED: StdlibSignature<'static> = StdlibSignature::new(&[
            required(StdlibArgKind::Number),
            required(StdlibArgKind::Number),
            optional(StdlibArgKind::Number),
        ]);
        const UNBOUNDED: StdlibSignature<'static> = StdlibSignature::new(&[
            required(StdlibArgKind::String),
            optional(StdlibArgKind::Vararg),
        ]);
        assert_eq!((BOUNDED.min_args, BOUNDED.max_args), (2, Some(3)));
        assert_eq!((UNBOUNDED.min_args, UNBOUNDED.max_args), (1, None));
    }

    #[test]
    fn overloads_answer_arity_queries() {
        static OVERLOADED: StdlibFunction<'static> = StdlibFunction {
            signatures: &[
                StdlibSignature::new(&[]),
                StdlibSignature::new(&[
                    required(StdlibArgKind::Number),
                    required(StdlibArgKind::Number),
                    optional(StdlibArgKind::Number),
                ]),
            ],
            ..FUNCTION
        };
        assert_eq!(OVERLOADED.min_args(), 0);
        assert_eq!(OVERLOADED.max_args(), Some(3));
        assert!(OVERLOADED.accepts_arg_count(0));
        assert!(!OVERLOADED.accepts_arg_count(1));
        assert!(OVERLOADED.accepts_arg_count(2));
        assert!(OVERLOADED.accepts_arg_count(3));
        assert!(!OVERLOADED.accepts_arg_count(4));
        assert_eq!(OVERLOADED.matching_signatures(2).count(), 1);
        assert_eq!(OVERLOADED.signature_index_for_active_param(1), 1);
    }

    /// `template` expanded with `args`, which the tests spell as text.
    fn expand(template: &str, args: &[&str]) -> String {
        let arg_bytes: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
        String::from_utf8(expand_replace_template(template, &arg_bytes)).unwrap()
    }

    #[test]
    fn replace_template_substitutes_numbered_args() {
        assert_eq!(expand("#%1", &["t"]), "#t");
        assert_eq!(expand("%1 ^ %2", &["a", "b"]), "a ^ b");
        assert_eq!(expand("load(%1)", &["x"]), "load(x)");
    }

    #[test]
    fn replace_template_unescapes_double_percent() {
        assert_eq!(expand("100%% %1", &["x"]), "100% x");
    }

    #[test]
    fn replace_template_expands_missing_arg_to_nothing() {
        assert_eq!(expand("%1 + %2", &["a"]), "a + ");
    }

    #[test]
    fn replace_template_groups_compound_operands() {
        assert_eq!(expand("(%1 ^ %2)", &["a + 1", "b"]), "((a + 1) ^ b)");
        assert_eq!(expand("#%1", &["a or b"]), "#(a or b)");
        assert_eq!(expand("#%1", &["..."]), "#(...)");
        assert_eq!(expand("#%1", &["a.b.c"]), "#a.b.c");
        assert_eq!(expand("(%1 ^ %2)", &["a", "-1"]), "(a ^ (-1))");
    }

    #[test]
    fn replace_template_keeps_call_arguments_verbatim() {
        assert_eq!(expand("math.log(%1, 10)", &["a + 1"]), "math.log(a + 1, 10)");
        assert_eq!(expand("string.gmatch(%1, %2)", &["s", "p .. q"]), "string.gmatch(s, p .. q)");
    }

    #[test]
    fn replace_template_forwards_trailing_arguments() {
        assert_eq!(expand("table.unpack(%1)", &["t", "2", "#t"]), "table.unpack(t, 2, #t)");
        assert_eq!(expand("task.wait(%1)", &[]), "task.wait()");
    }
}
