use std::borrow::Cow;

use crate::stdlib_map::StdlibMap;

/// Each variant holds a reference, so an entry is two words, and a table of
/// them stays dense for the lookups that scan it.
#[derive(Debug, Clone, Copy)]
pub enum StdlibEntry<'lib> {
    Function(&'lib StdlibFunction<'lib>),
    Value(&'lib StdlibValue<'lib>),
    Namespace(&'lib StdlibNamespace<'lib>),
}

impl<'lib> StdlibEntry<'lib> {
    /// Only a function can be deprecated, since a definition file's
    /// attributes go only on functions.
    pub const fn deprecation(&self) -> Option<&'lib StdlibDeprecation<'lib>> {
        match *self {
            Self::Function(function) => function.deprecated.as_ref(),
            Self::Value(_) | Self::Namespace(_) => None,
        }
    }

    pub const fn is_read_only(&self) -> bool {
        match self {
            Self::Function(function) => function.is_read_only,
            Self::Value(value) => value.is_read_only,
            Self::Namespace(namespace) => namespace.is_read_only,
        }
    }

    /// The shape of the value this entry names. A function's value has
    /// none; `returns_shape` is the shape of its call's result.
    pub const fn shape(&self) -> Option<&'lib str> {
        match self {
            Self::Value(value) => value.shape,
            Self::Namespace(namespace) => namespace.shape,
            Self::Function(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StdlibFunction<'lib> {
    /// At least one, in definition order. The first is the primary form,
    /// shown where only one fits.
    pub signatures: &'lib [StdlibSignature<'lib>],
    pub deprecated: Option<StdlibDeprecation<'lib>>,
    pub is_read_only: bool,
    /// Colon-callable on its shape. No signature counts `self` among its
    /// params.
    pub is_method: bool,
    /// Shape of the call's first return value, if it has one (e.g.
    /// `string.rep` returns `string`).
    pub returns_shape: Option<&'lib str>,
}

impl<'lib> StdlibFunction<'lib> {
    pub fn min_args(&self) -> usize {
        self.signatures.iter().map(|signature| signature.min_args).min().unwrap_or(0)
    }

    /// Maximum arity across all signatures; `None` if any is unbounded.
    pub fn max_args(&self) -> Option<usize> {
        let mut max = 0;
        for signature in self.signatures {
            max = max.max(signature.max_args?);
        }
        Some(max)
    }

    pub fn accepts_arg_count(&self, count: usize) -> bool {
        self.signatures.iter().any(|signature| signature.accepts(count))
    }

    pub fn matching_signatures(
        &self,
        count: usize,
    ) -> impl Iterator<Item = &'lib StdlibSignature<'lib>> + Clone {
        self.signatures.iter().filter(move |signature| signature.accepts(count))
    }

    /// The constants that parameter `index` allows in each signature that
    /// accepts a call with `count` arguments, one list per signature in
    /// definition order.
    ///
    /// Returns `None` when no signature accepts `count`, or when one of them
    /// takes anything but a constant there, which leaves the argument
    /// unconstrained.
    pub fn allowed_constants(
        &self,
        index: usize,
        count: usize,
    ) -> Option<impl Iterator<Item = &'lib [&'lib str]> + Clone> {
        let sets = self.matching_signatures(count).map(move |signature| {
            let StdlibArgKind::Constant(values) = signature.params.get(index)?.kind else {
                return None;
            };
            Some(values)
        });
        let mut checked = sets.clone();
        let is_constrained =
            checked.next().is_some_and(|set| set.is_some()) && checked.all(|set| set.is_some());
        is_constrained.then(|| sets.flatten())
    }

    pub const fn primary_signature(&self) -> &'lib StdlibSignature<'lib> {
        &self.signatures[0]
    }

    /// Index of the signature to highlight when the cursor sits on
    /// parameter `active_param`: the first signature with a slot at
    /// that position, else the last.
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
            .unwrap_or_else(|| self.signatures.len() - 1)
    }
}

/// One form of a function's call: its params, of which only the last can be
/// a vararg, and the arities [`Self::new`] counts from them.
#[derive(Debug, Clone, Copy)]
pub struct StdlibSignature<'lib> {
    pub params: &'lib [StdlibParam<'lib>],
    /// Through the last required param, since a call can leave out only the
    /// optional ones after it.
    pub min_args: usize,
    /// `None` when a trailing vararg leaves it unbounded.
    pub max_args: Option<usize>,
}

impl<'lib> StdlibSignature<'lib> {
    pub fn new(params: &'lib [StdlibParam<'lib>]) -> Self {
        let is_vararg = |param: &StdlibParam<'_>| matches!(param.kind, StdlibArgKind::Vararg);
        let min_args = params
            .iter()
            .rposition(|param| param.is_required && !is_vararg(param))
            .map_or(0, |last| last + 1);
        let max_args = (!params.last().is_some_and(is_vararg)).then_some(params.len());
        Self { params, min_args, max_args }
    }

    pub fn accepts(&self, count: usize) -> bool {
        count >= self.min_args && self.max_args.is_none_or(|max| count <= max)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StdlibParam<'lib> {
    pub kind: StdlibArgKind<'lib>,
    /// Never set on a vararg.
    pub is_required: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum StdlibArgKind<'lib> {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    /// A type with no variant of its own, e.g. "buffer" or "thread". The
    /// string is only a display label.
    Display(&'lib str),
    /// A fixed set of string literals, e.g. the formats of `os.date`.
    Constant(&'lib [&'lib str]),
    /// `...`, matching zero or more values of any type.
    Vararg,
}

/// What a function's `@[deprecated {use = "...", reason = "..."}]` says, each
/// part as the definition gives it.
#[derive(Debug, Clone, Copy)]
pub struct StdlibDeprecation<'lib> {
    /// What to call instead, as `task.wait`, or a method's name.
    pub r#use: Option<&'lib str>,
    pub reason: Option<&'lib str>,
}

impl<'lib> StdlibDeprecation<'lib> {
    /// What a reader is told: the reason, or else what to use instead.
    pub fn advice(&self) -> Option<Cow<'lib, str>> {
        match (self.reason, self.r#use) {
            (Some(reason), _) => Some(Cow::Borrowed(reason)),
            (None, Some(replacement)) => Some(Cow::Owned(format!("use `{replacement}` instead"))),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StdlibValue<'lib> {
    pub is_read_only: bool,
    /// The value's shape, if it has one (e.g. `game` is a `DataModel`).
    pub shape: Option<&'lib str>,
}

/// A dotted-member container: a table type's fields. `shape` is set when
/// the namespace is also a value with methods, as each `Enum.<Type>` is an
/// `Enum` with `GetEnumItems`.
#[derive(Debug, Clone, Copy)]
pub struct StdlibNamespace<'lib> {
    pub members: StdlibMap<'lib, StdlibEntry<'lib>>,
    pub shape: Option<&'lib str>,
    pub is_read_only: bool,
}

/// The members of a kind of value, such as a string or a Roblox instance.
/// Members with `is_method` are colon-callable; the rest are dot accesses.
#[derive(Debug, Clone, Copy)]
pub struct StdlibShape<'lib> {
    pub members: StdlibMap<'lib, StdlibEntry<'lib>>,
    /// The shape whose members this one has too, under its own, as each
    /// Roblox service has `Instance`'s.
    pub extends: Option<&'lib str>,
}

/// A standard library: Luau's, with the definitions a project loads layered
/// over it.
#[derive(Debug, Clone, Copy)]
pub struct StdlibLibrary<'lib> {
    /// Whether Roblox's API is among its layers, which the Roblox rules run
    /// on.
    pub has_roblox: bool,
    pub globals: StdlibMap<'lib, StdlibEntry<'lib>>,
    pub shapes: StdlibMap<'lib, StdlibShape<'lib>>,
}

impl<'lib> StdlibLibrary<'lib> {
    /// Each step goes through a namespace's members or the shape of a
    /// value, so `["game", "GetService"]` resolves through `game`'s shape.
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

#[cfg(test)]
mod tests {
    use super::*;

    const fn param(kind: StdlibArgKind<'static>, is_required: bool) -> StdlibParam<'static> {
        StdlibParam { kind, is_required }
    }

    #[test]
    fn a_signature_counts_through_its_last_required_param_and_a_vararg() {
        let bounded = [
            param(StdlibArgKind::Number, true),
            param(StdlibArgKind::Number, true),
            param(StdlibArgKind::Number, false),
        ];
        let unbounded = [param(StdlibArgKind::String, true), param(StdlibArgKind::Vararg, false)];
        let optional_first =
            [param(StdlibArgKind::Number, false), param(StdlibArgKind::Function, true)];
        let [bounded, unbounded, optional_first] =
            [&bounded[..], &unbounded, &optional_first].map(StdlibSignature::new);
        assert_eq!((bounded.min_args, bounded.max_args), (2, Some(3)));
        assert_eq!((unbounded.min_args, unbounded.max_args), (1, None));
        assert_eq!((optional_first.min_args, optional_first.max_args), (2, Some(2)));
    }

    #[test]
    fn overloads_answer_arity_queries() {
        let two_or_three = [
            param(StdlibArgKind::Number, true),
            param(StdlibArgKind::Number, true),
            param(StdlibArgKind::Number, false),
        ];
        let signatures = [StdlibSignature::new(&[]), StdlibSignature::new(&two_or_three)];
        let overloaded = StdlibFunction {
            signatures: &signatures,
            deprecated: None,
            is_read_only: true,
            is_method: false,
            returns_shape: None,
        };
        assert_eq!(overloaded.min_args(), 0);
        assert_eq!(overloaded.max_args(), Some(3));
        assert!(overloaded.accepts_arg_count(0));
        assert!(!overloaded.accepts_arg_count(1));
        assert!(overloaded.accepts_arg_count(2));
        assert!(overloaded.accepts_arg_count(3));
        assert!(!overloaded.accepts_arg_count(4));
        assert_eq!(overloaded.matching_signatures(2).count(), 1);
        assert_eq!(overloaded.signature_index_for_active_param(1), 1);
    }
}
