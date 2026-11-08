//! Identifier tokens and the local declarations that bind them.

use luck_ast::LocalAssignment;
use luck_token::{CompactString, LuaVersion, Span, Token, TokenKind};

/// The name an identifier token spells, or `""` for any other token.
pub(crate) fn identifier_name(token: &Token) -> &str {
    token.identifier().unwrap_or_default()
}

/// A span-less identifier token for a node a pass builds.
pub(crate) fn identifier_token(name: CompactString) -> Token {
    Token::new(TokenKind::Identifier(name), Span::default())
}

/// Whether this name is the `_ENV` upvalue every global read and write
/// goes through. Outside 5.2+ the name binds an ordinary local, so
/// pinning it would cost bytes for nothing.
pub(crate) fn is_env_binding(name: &str, version: LuaVersion) -> bool {
    version.has_env_upvalue() && name == "_ENV"
}

/// Whether no transform may reshape, merge away, or move a `local`
/// declaration.
///
/// That holds for an export, a `const`, an attributed name (`<close>` runs
/// `__close` at scope exit and `<const>` affects validity), and a 5.2+
/// `_ENV`, which redirects global lookup for everything after it.
pub(crate) fn has_fixed_binding(local: &LocalAssignment, version: LuaVersion) -> bool {
    local.is_const
        || local.is_exported
        || local.names.iter().any(|attributed| {
            attributed.attribute.is_some()
                || is_env_binding(identifier_name(&attributed.name), version)
        })
}
