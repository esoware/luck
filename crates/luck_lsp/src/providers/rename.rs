//! Locals only: [`prepare`] refuses globals, stdlib names, and locals whose
//! name something outside its declaration depends on (a method's implicit
//! `self`, a local `_ENV`, an exported binding).
//!
//! A rename is also refused whenever the new name could change meaning: an
//! invalid identifier, a reserved word, a known global, or a name already used
//! anywhere in the document. Refusing is safer than silently capturing a
//! shadowed name.

use ls_types::{Position, PrepareRenameResponse, RenameParams, TextEdit, WorkspaceEdit};
use luck_ast::is_valid_identifier_in;
use luck_semantic::SymbolId;
use luck_token::Span;

use crate::document::{DocumentState, span_text};
use crate::providers::cursor::local_at;

pub(crate) fn prepare(
    document: &DocumentState,
    position: Position,
) -> Option<PrepareRenameResponse> {
    let (_, span) = rename_target(document, document.offset(position)).ok().flatten()?;
    Some(PrepareRenameResponse::RangeWithPlaceholder {
        range: document.range(span),
        placeholder: span_text(document.text(), span).to_owned(),
    })
}

/// # Errors
///
/// Returns the refusal to show the user when that local's name can't change
/// without changing what the document means.
fn rename_target(
    document: &DocumentState,
    offset: u32,
) -> Result<Option<(SymbolId, Span)>, String> {
    let tree = document.tree();
    let Some((symbol_id, span)) = local_at(tree, offset) else {
        return Ok(None);
    };
    let symbol = tree.symbol(symbol_id);
    if symbol.kind.is_implicit() {
        return Err(
            "`self` is the method's implicit parameter, so it has no declaration to rename"
                .to_owned(),
        );
    }
    if tree.is_env_binding(symbol_id) {
        return Err(
            "renaming `_ENV` would change what every free name in its scope means".to_owned()
        );
    }
    if symbol.is_exported {
        let name = tree.name(symbol.name);
        return Err(format!("`{name}` is exported, so its name is the module's API"));
    }
    Ok(Some((symbol_id, span)))
}

/// # Errors
///
/// Returns the refusal to show the user when `params.new_name` could change
/// what the document means.
pub(crate) fn rename(
    document: &DocumentState,
    params: &RenameParams,
) -> Result<Option<WorkspaceEdit>, String> {
    let text_document_position = &params.text_document_position;
    let Some((symbol_id, _)) =
        rename_target(document, document.offset(text_document_position.position))?
    else {
        return Ok(None);
    };
    let new_name = params.new_name.as_str();
    validate_new_name(document, new_name)?;

    let tree = document.tree();
    let mut spans: Vec<Span> = tree
        .resolved_references(symbol_id)
        .iter()
        .map(|&reference_id| tree.reference(reference_id).span)
        .collect();
    spans.push(tree.symbol(symbol_id).definition_span);
    spans.sort_by_key(|span| span.start);
    spans.dedup();

    let mut positions = document.positions();
    let edits: Vec<TextEdit> = spans
        .into_iter()
        .map(|span| TextEdit { range: positions.range(span), new_text: new_name.to_owned() })
        .collect();
    let changes = [(text_document_position.text_document.uri.clone(), edits)].into();
    Ok(Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }))
}

/// # Errors
///
/// Returns the refusal to show the user when it isn't an identifier, shadows
/// a built-in or configured global, or is already used in the document.
fn validate_new_name(document: &DocumentState, new_name: &str) -> Result<(), String> {
    // Checked under the document's Lua version, so keywords are rejected
    // version-accurately (`goto` is an identifier in 5.1).
    if !is_valid_identifier_in(new_name, document.version()) {
        return Err(format!("`{new_name}` isn't a valid identifier"));
    }
    // The analysis knows only the configured globals the document names.
    let extra_globals = &document.settings().lint.extra_globals;
    if document.semantic().is_known_global(new_name)
        || extra_globals.iter().any(|global| global == new_name)
    {
        return Err(format!("`{new_name}` would shadow a built-in global"));
    }
    // Every name a local or a reference uses is one of the chunk's names.
    if document.tree().name_id(new_name).is_some() {
        return Err(format!(
            "`{new_name}` is already used in this file, so renaming could capture it"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ls_types::{
        Range, TextDocumentIdentifier, TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    };
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::document;

    fn uri() -> Uri {
        "file:///main.lua".parse().expect("valid URI")
    }

    /// # Errors
    ///
    /// Returns the refusal [`rename`] gives.
    fn rename_at(
        document: &DocumentState,
        line: u32,
        utf16_column: u32,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, String> {
        rename(
            document,
            &RenameParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: uri() },
                    position: Position::new(line, utf16_column),
                },
                new_name: new_name.to_owned(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        )
    }

    fn edits_of(edit: WorkspaceEdit) -> Vec<TextEdit> {
        edit.changes.expect("changes map").remove(&uri()).expect("edits for the document")
    }

    #[test]
    fn rename_of_a_local_declared_in_a_method_edits_only_that_local() {
        let document = document(
            "\
local t = {}
function t:m()
  local value = 1
  return value + self.x
end
",
            LuaTarget::Lua54,
        );
        let edit = rename_at(&document, 2, 9, "total")
            .expect("rename succeeds")
            .expect("a local is renameable");
        let ranges: Vec<Range> = edits_of(edit).iter().map(|text_edit| text_edit.range).collect();
        assert_eq!(
            ranges,
            vec![
                Range::new(Position::new(2, 8), Position::new(2, 13)),
                Range::new(Position::new(3, 9), Position::new(3, 14)),
            ],
            "renaming `value` must edit its declaration and read, not the implicit `self`'s body"
        );
    }

    #[test]
    fn rename_refuses_the_implicit_self() {
        // A method's `self` has no declaration to rewrite, so no new name
        // can work.
        let document = document(
            "\
local t = {}
function t:m()
  return self.x
end
",
            LuaTarget::Lua54,
        );
        let result = rename_at(&document, 2, 10, "me");
        assert!(
            !matches!(result, Ok(Some(_))),
            "renaming the implicit `self` must be refused, got {result:?}"
        );
    }

    #[test]
    fn prepare_rename_refuses_a_keyword_inside_a_method() {
        let document = document(
            "\
local t = {}
function t:m()
  return 1
end
",
            LuaTarget::Lua54,
        );
        let prepared = prepare(&document, Position::new(2, 3));
        assert!(
            prepared.is_none(),
            "`return` is no name, but it resolves to `self` through the body span: {prepared:?}"
        );
    }

    #[test]
    fn prepare_rename_refuses_a_recovered_name() {
        // Recovery gives the loop variable the `=` token's span and an empty
        // name.
        let document = document("for = 1, 2 do end\n", LuaTarget::Lua54);
        let prepared = prepare(&document, Position::new(0, 4));
        assert!(prepared.is_none(), "`=` is no name to rename: {prepared:?}");
    }

    #[test]
    fn rename_refuses_a_local_env() {
        // In 5.2+ `_ENV` resolves every free name, so renaming the local
        // sends `x` to the real globals. `local _ENV = {x = 1}; return x`
        // returns 1 while the renamed form returns nil on lua5.4.
        let document = document(
            "\
local _ENV = {x = 1}
return x
",
            LuaTarget::Lua54,
        );
        let result = rename_at(&document, 0, 8, "scope");
        assert!(
            result.is_err(),
            "renaming `_ENV` changes what every free name means, got {result:?}"
        );
    }

    #[test]
    fn rename_refuses_an_exported_binding() {
        // An exported name is the module's public field name, per the Luau
        // export RFC.
        let document = document(
            "\
export local version = 1
print(version)
",
            LuaTarget::Luau,
        );
        assert!(document.parse().errors.is_empty(), "{:?}", document.parse().errors);
        let result = rename_at(&document, 0, 15, "release");
        assert!(
            result.is_err(),
            "renaming an exported binding changes the module's API, got {result:?}"
        );
    }

    #[test]
    fn prepare_rename_refuses_what_rename_refuses() {
        let exported = document("export local version = 1\n", LuaTarget::Luau);
        assert!(prepare(&exported, Position::new(0, 15)).is_none());
        let env = document(
            "\
local _ENV = {x = 1}
return x
",
            LuaTarget::Lua54,
        );
        assert!(prepare(&env, Position::new(0, 8)).is_none());
    }
}
