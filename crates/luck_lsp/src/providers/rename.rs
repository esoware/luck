//! A member is renamed across the workspace, and a local in its document.
//!
//! A member of a module's table is renamed in every file of the workspace
//! that names it. [`prepare`] refuses globals, stdlib names, a member of a
//! module outside the workspace, whose readers the server can't know, and a
//! local whose name something outside its declaration depends on, a method's
//! implicit `self` or a local `_ENV`.
//!
//! A rename is also refused whenever the new name could change meaning: an
//! invalid identifier or a reserved word in any file it lands in, another
//! member of the same table, or for a local, an exported one included, a
//! known global or a name already used anywhere in its document. Refusing is
//! safer than silently capturing a shadowed name.

use alloc::collections::BTreeMap;
use std::path::Path;

use ls_types::{Position, PrepareRenameResponse, RenameParams, TextEdit, Uri, WorkspaceEdit};
use luck_ast::is_valid_identifier_in;
use luck_semantic::SymbolId;
use luck_token::Span;

use crate::document::{DocumentState, span_text};
use crate::module_record::Table;
use crate::providers::cursor::local_at;
use crate::workspace::{Source, WorkspaceView};

pub(crate) fn prepare(
    workspace: &WorkspaceView<'_>,
    uri: &Uri,
    document: &DocumentState,
    position: Position,
) -> Option<PrepareRenameResponse> {
    if let Some((member, module)) =
        Source::of_document(uri, document).and_then(|source| source.member_at(position))
    {
        return workspace.is_workspace_file(&module).then(|| {
            PrepareRenameResponse::RangeWithPlaceholder {
                range: member.range,
                placeholder: member.name.to_string(),
            }
        });
    }
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
    let Some((symbol_id, span)) = local_at(document, offset) else {
        return Ok(None);
    };
    if tree.symbol(symbol_id).kind.is_implicit() {
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
    Ok(Some((symbol_id, span)))
}

/// # Errors
///
/// Returns the refusal to show the user when `params.new_name` could change
/// what the code means.
pub(crate) fn rename(
    workspace: &WorkspaceView<'_>,
    uri: &Uri,
    document: &DocumentState,
    params: &RenameParams,
) -> Result<Option<WorkspaceEdit>, String> {
    let position = params.text_document_position.position;
    let new_name = params.new_name.as_str();
    if let Some((member, module)) =
        Source::of_document(uri, document).and_then(|source| source.member_at(position))
    {
        return rename_member(workspace, &member.name, &module, new_name);
    }
    let Some((symbol_id, _)) = rename_target(document, document.offset(position))? else {
        return Ok(None);
    };
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
    let changes = [(uri.clone(), edits)].into();
    Ok(Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }))
}

/// Renames the member `name` of the table of the module at `module` to
/// `new_name` in every file that names it, or `None` once the request is
/// cancelled.
///
/// # Errors
///
/// Returns the refusal to show the user when the module is outside the
/// workspace, or `new_name` could change what a file means.
fn rename_member(
    workspace: &WorkspaceView<'_>,
    name: &str,
    module: &Path,
    new_name: &str,
) -> Result<Option<WorkspaceEdit>, String> {
    if !workspace.is_workspace_file(module) {
        return Err(format!(
            "`{name}` is a member of a module outside the workspace, so renaming it could miss \
             a file that reads it"
        ));
    }
    let mut changes: BTreeMap<Uri, Vec<TextEdit>> = BTreeMap::new();
    let mut refusal = None;
    let mut is_exported = false;
    let finished = workspace.for_each_source(|source| {
        for member in source.record.members() {
            let is_renamed = &*member.name == name;
            if (!is_renamed && &*member.name != new_name)
                || !source.is_table_of(member.table, module)
            {
                continue;
            }
            if !is_renamed {
                refusal.get_or_insert_with(|| {
                    format!(
                        "`{new_name}` is already a member of the module, so renaming would \
                         merge the two"
                    )
                });
                continue;
            }
            if !is_valid_identifier_in(new_name, source.version) {
                refusal.get_or_insert_with(|| format!("`{new_name}` isn't a valid identifier"));
            }
            is_exported |= member.table == Table::Exported;
            changes
                .entry(source.uri.clone())
                .or_default()
                .push(TextEdit { range: member.range, new_text: new_name.to_owned() });
        }
    });
    if finished.is_none() {
        return Ok(None);
    }
    if let Some(refused) = refusal {
        return Err(refused);
    }
    // An exported member is a local of its module, which a new name can
    // capture there as it could any local's.
    if is_exported {
        workspace
            .with_document(module, |document| validate_new_name(document, new_name))
            .unwrap_or_else(|| Err(format!("the module that exports `{name}` can't be read")))?;
    }
    Ok(Some(WorkspaceEdit { changes: Some(changes.into_iter().collect()), ..Default::default() }))
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
    let extra_globals = &document.settings().linter.config().extra_globals;
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
    use std::fs;

    use ls_types::{
        Range, TextDocumentIdentifier, TextDocumentPositionParams, WorkDoneProgressParams,
    };
    use luck_config::Project;
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::{document, document_at, session};

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
        let session = session();
        let workspace = WorkspaceView::new(&session, &[], &|| false);
        rename(
            &workspace,
            &uri(),
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

    fn prepare_at(document: &DocumentState, position: Position) -> Option<PrepareRenameResponse> {
        let session = session();
        prepare(&WorkspaceView::new(&session, &[], &|| false), &uri(), document, position)
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
        let prepared = prepare_at(&document, Position::new(2, 3));
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
        let prepared = prepare_at(&document, Position::new(0, 4));
        assert!(prepared.is_none(), "`=` is no name to rename: {prepared:?}");
    }

    #[test]
    fn rename_and_prepare_refuse_a_local_env() {
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
        assert!(prepare_at(&document, Position::new(0, 8)).is_none());
    }

    /// An exported name is a field of the module's table, per the Luau
    /// export RFC, so a module outside the workspace keeps it.
    #[test]
    fn an_export_of_a_module_outside_the_workspace_is_not_renamed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("version.luau");
        let text = "\
export local version = 1
print(version)
";
        fs::write(&path, text).expect("write version.luau");
        let project = Project::defaults(directory.path().to_path_buf());
        let exported = document_at(text, LuaTarget::Luau, project, &path);
        assert!(exported.parse().errors.is_empty(), "{:?}", exported.parse().errors);
        let result = rename_at(&exported, 0, 15, "release");
        assert!(result.is_err(), "renaming the export could miss its readers, got {result:?}");
        assert_eq!(prepare_at(&exported, Position::new(0, 15)), None);
    }

    /// No file can require a document with no file, so its exports are its
    /// own.
    #[test]
    fn an_untitled_document_renames_its_export_as_a_local() {
        let document = document(
            "\
export local version = 1
print(version)
",
            LuaTarget::Luau,
        );
        let edit = rename_at(&document, 0, 15, "release")
            .expect("rename succeeds")
            .expect("the export is renameable");
        assert_eq!(edits_of(edit).len(), 2);
    }
}
