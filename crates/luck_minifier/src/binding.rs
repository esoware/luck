use luck_ast::{FunctionBody, LocalAssignment, LocalAssignmentKind};
use luck_token::LuaVersion;

pub(crate) const METHOD_RECEIVER_NAME: &str = "self";

pub(crate) const IMPLICIT_VARARG_TABLE_NAME: &str = "arg";

pub(crate) const fn has_implicit_arg(body: &FunctionBody<'_>, version: LuaVersion) -> bool {
    body.vararg.is_some() && version.has_implicit_vararg_table()
}

/// Whether this name is the `_ENV` upvalue every global read and write
/// goes through. Outside 5.2+ the name binds an ordinary local, so
/// pinning it would cost bytes for nothing.
pub(crate) fn is_env_binding(name: &str, version: LuaVersion) -> bool {
    version.has_env_upvalue() && name == "_ENV"
}

/// Whether a `local` declaration binds a name with an attribute (`<close>`
/// runs `__close` at scope exit and `<const>` affects validity) or a 5.2+
/// `_ENV`, which redirects global lookup for everything after it.
pub(crate) fn has_attributed_or_env_name(local: &LocalAssignment<'_>, version: LuaVersion) -> bool {
    local.names.iter().any(|attributed| {
        attributed.attribute.is_some() || is_env_binding(attributed.name.text, version)
    })
}

/// Whether no transform may reshape, merge away, or move a `local`
/// declaration: an export, a `const`, or one that
/// [`has_attributed_or_env_name`].
pub(crate) fn has_fixed_binding(local: &LocalAssignment<'_>, version: LuaVersion) -> bool {
    local.kind != LocalAssignmentKind::Local || has_attributed_or_env_name(local, version)
}
