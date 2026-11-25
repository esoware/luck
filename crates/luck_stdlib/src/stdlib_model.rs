use crate::stdlib_map::StdlibMap;
use crate::variant::Variant;

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
    pub const fn deprecation(&self) -> Option<&'lib StdlibDeprecation<'lib>> {
        match *self {
            Self::Function(function) => function.deprecated.as_ref(),
            Self::Constant(value) | Self::Property(value) => value.deprecated.as_ref(),
            Self::Namespace(namespace) => namespace.deprecated.as_ref(),
        }
    }

    /// A namespace is always read-only, even when some members (`package.path`) aren't.
    pub const fn is_read_only(&self) -> bool {
        match self {
            Self::Function(function) => function.is_read_only,
            Self::Constant(value) | Self::Property(value) => value.is_read_only,
            Self::Namespace(_) => true,
        }
    }

    /// The shape of the value this entry names. A function's value has
    /// none; `returns_shape` is the shape of its call's result.
    pub const fn shape(&self) -> Option<&'lib str> {
        match self {
            Self::Constant(value) | Self::Property(value) => value.shape,
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
    pub is_pure: bool,
    pub is_must_use: bool,
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
    ) -> Option<impl Iterator<Item = &'lib [StdlibConstant<'lib>]> + Clone> {
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
    pub const fn new(params: &'lib [StdlibParam<'lib>]) -> Self {
        let mut min_args = 0;
        let mut max_args = Some(params.len());
        let mut remaining = params;
        while let [param, rest @ ..] = remaining {
            let is_vararg = matches!(param.kind, StdlibArgKind::Vararg);
            if is_vararg {
                max_args = None;
            }
            if param.is_required && !is_vararg {
                min_args += 1;
            }
            remaining = rest;
        }
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
/// can be deprecated, as Roblox's deprecated classes are.
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
    /// [`expand_replace_template`](crate::expand_replace_template) reads them.
    pub template: &'lib str,
    /// Whether the new call returns what the old one did for every argument
    /// the old one accepts. Only then can a fix make it without asking.
    pub is_exact: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct StdlibValue<'lib> {
    pub is_read_only: bool,
    pub deprecated: Option<StdlibDeprecation<'lib>>,
    /// The value's shape, if it has one (e.g. `game` is a `DataModel`).
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

/// A variant's standard library, built in or with std definitions layered
/// over it.
#[derive(Debug, Clone, Copy)]
pub struct StdlibLibrary<'lib> {
    pub variant: Variant,
    pub globals: StdlibMap<'lib, StdlibEntry<'lib>>,
    pub shapes: StdlibMap<'lib, StdlibShape<'lib>>,
    /// The lists of constants a std definition's params can name, such as
    /// Roblox's service names.
    pub constant_sets: StdlibMap<'lib, &'lib [StdlibConstant<'lib>]>,
}

impl<'lib> StdlibLibrary<'lib> {
    /// What the generated catalogs layer their files over.
    #[cfg(test)]
    pub(crate) const fn empty(variant: Variant) -> Self {
        Self {
            variant,
            globals: StdlibMap::EMPTY,
            shapes: StdlibMap::EMPTY,
            constant_sets: StdlibMap::EMPTY,
        }
    }

    /// Each step goes through a namespace's members or the shape of a
    /// property or constant, so `["game", "GetService"]` resolves through
    /// `game`'s shape.
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
    use crate::stdlib_catalog::{FUNCTION, optional, required};

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
}
