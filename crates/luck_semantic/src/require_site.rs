use luck_ast::{Call, Expression, FunctionArgs, Name, StringLiteral, Suffix, SuffixedExpression};

use crate::id::ReferenceId;
use crate::scoping::Scoping;

/// A call of the host's global `require`, which loads a module.
///
/// It calls the name, as `require(path)` or `require "path"`, or the field
/// the global table holds it in, as `_G.require(path)`. A method call and a
/// call of a local or of a global the chunk writes load nothing the chunk
/// can know of, so they're no site.
#[derive(Debug, Clone, Copy)]
pub struct RequireSite<'ast> {
    /// Its callee is the name, or the global table and the field.
    pub call: Call<'ast>,
    /// The module path, when the call passes one string literal and nothing
    /// else, and `None` for a path only the running program knows.
    pub path: Option<&'ast StringLiteral<'ast>>,
    /// Made in a type, as `typeof(require(path))` makes one, which never
    /// runs.
    pub is_in_type: bool,
}

/// The non-method call `chain` opens with when it calls a name `require`, or
/// reads `require` from the global table `prefix` names and calls it, as
/// `_G.require(path)`.
pub(crate) fn require_call<'ast>(
    chain: &'ast SuffixedExpression<'ast>,
    prefix: &Name<'_>,
) -> Option<Call<'ast>> {
    let callee_suffixes = if prefix.text == "require" {
        0
    } else if prefix.text == "_G" && is_key(chain.suffixes.first(), "require") {
        1
    } else {
        return None;
    };
    let call = chain.call_at(callee_suffixes)?;
    call.method.is_none().then_some(call)
}

/// The sites among `calls`, each a [`require_call`] with the reference its
/// prefix makes, once `scoping` is finished.
pub(crate) fn require_sites<'ast>(
    calls: Vec<(Call<'ast>, ReferenceId)>,
    scoping: &Scoping,
) -> Vec<RequireSite<'ast>> {
    // `_G.require` is the global `require` only while the chunk leaves that
    // global alone too.
    let is_require_assigned =
        scoping.name_id("require").is_some_and(|require| scoping.is_assigned_global(require));
    calls
        .into_iter()
        .filter(|&(call, reference)| {
            scoping.is_host_global(scoping.reference(reference))
                && (call.callee.suffixes.is_empty() || !is_require_assigned)
        })
        .map(|(call, reference)| RequireSite {
            call,
            path: require_path(call.args),
            is_in_type: scoping.reference(reference).is_in_type,
        })
        .collect()
}

/// Whether `suffix` reads the field `key`, as `.key` or `["key"]`, however
/// the string is spelled.
fn is_key(suffix: &Suffix<'_>, key: &str) -> bool {
    match suffix {
        Suffix::Field(name) => name.text == key,
        Suffix::Index { index: Expression::StringLiteral(literal), .. } => {
            literal.value == key.as_bytes()
        }
        Suffix::Index { .. }
        | Suffix::Call(_)
        | Suffix::MethodCall(_)
        | Suffix::TypeInstantiation { .. } => false,
    }
}

/// The module path of a `require` call that passes one string literal and
/// nothing else.
fn require_path<'ast>(
    function_args: &'ast FunctionArgs<'ast>,
) -> Option<&'ast StringLiteral<'ast>> {
    match function_args {
        FunctionArgs::Parenthesized { args, .. } => {
            let [Expression::StringLiteral(literal)] = &**args else {
                return None;
            };
            Some(literal)
        }
        FunctionArgs::StringLiteral(literal) => Some(literal),
        FunctionArgs::TableConstructor(_) | FunctionArgs::Error(_) => None,
    }
}
