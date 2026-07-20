//! Semantic tokens. Emits a token for every identifier - variables,
//! parameters, properties, methods, functions - mirroring the coloring
//! contract of the TypeScript language server. Many themes color
//! identifiers only through semantic tokens and leave the TextMate
//! variable scopes unthemed, so a stdlib-only emission renders user
//! code plain in those themes. Types and modifiers are chosen so the
//! default semantic-to-scope fallbacks land on the same scopes the
//! grammar emits (all-caps names carry readonly to reach
//! variable.other.constant, stdlib names carry defaultLibrary to reach
//! support.function), keeping semantic and TextMate rendering aligned.

use std::collections::HashMap;

use luck_semantic::scope::SymbolKind;
use luck_semantic::stdlib_model::{StdlibEntry, StdlibLibrary, library_for};
use luck_token::{Span, Token, TokenKind};
use tower_lsp::lsp_types::{
    Position, Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensLegend, SemanticTokensRangeResult, SemanticTokensResult,
};

use crate::backend::DocumentState;

pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::METHOD,
];

const TY_NAMESPACE: u32 = 0;
const TY_FUNCTION: u32 = 1;
const TY_PROPERTY: u32 = 2;
const TY_VARIABLE: u32 = 3;
const TY_PARAMETER: u32 = 4;
const TY_METHOD: u32 = 5;

pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DEPRECATED,
    SemanticTokenModifier::READONLY,
    SemanticTokenModifier::DEFAULT_LIBRARY,
];

const MOD_DEPRECATED: u32 = 1 << 0;
const MOD_READONLY: u32 = 1 << 1;
const MOD_DEFAULT_LIBRARY: u32 = 1 << 2;

#[must_use]
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: TOKEN_MODIFIERS.to_vec(),
    }
}

#[must_use]
pub fn semantic_tokens(doc: &DocumentState) -> SemanticTokensResult {
    SemanticTokensResult::Tokens(encode_tokens(doc, None))
}

/// Range request: same classification, filtered to tokens overlapping the
/// range. Deltas are re-encoded from the filtered list, so the first token
/// is relative to the document start exactly as the protocol requires.
#[must_use]
pub fn semantic_tokens_range(doc: &DocumentState, range: &Range) -> SemanticTokensRangeResult {
    let start = doc.line_index.offset(&doc.text, range.start);
    let end = doc.line_index.offset(&doc.text, range.end);
    SemanticTokensRangeResult::Tokens(encode_tokens(doc, Some((start, end))))
}

/// How the token stream uses a name at a given position - the same
/// signals the grammar's lookaheads key on, so semantic and TextMate
/// classification agree at every site.
enum ValueShape {
    Called,
    AssignedFunction,
    AssignedStdlibFunction,
    Plain,
}

fn is_call_opener(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LeftParen
            | TokenKind::LeftBrace
            | TokenKind::StringLiteral(_)
            | TokenKind::InterpBegin(_)
    )
}

fn value_shape(tokens: &[Token], idx: usize, lib: &StdlibLibrary) -> ValueShape {
    match tokens.get(idx + 1).map(|token| &token.kind) {
        Some(kind) if is_call_opener(kind) => ValueShape::Called,
        Some(TokenKind::Equal) => match tokens.get(idx + 2).map(|token| &token.kind) {
            Some(TokenKind::Function) => ValueShape::AssignedFunction,
            Some(TokenKind::Identifier(rhs)) => {
                let is_stdlib_fn = lib
                    .globals
                    .get(rhs.as_str())
                    .is_some_and(|entry| matches!(entry, StdlibEntry::Function(_)));
                let extends = tokens.get(idx + 3).is_some_and(|token| {
                    is_call_opener(&token.kind)
                        | matches!(
                            token.kind,
                            TokenKind::Dot | TokenKind::Colon | TokenKind::LeftBracket
                        )
                });
                if is_stdlib_fn && !extends {
                    ValueShape::AssignedStdlibFunction
                } else {
                    ValueShape::Plain
                }
            }
            _ => ValueShape::Plain,
        },
        _ => ValueShape::Plain,
    }
}

/// Matches the grammar's all-caps constant pattern: `[A-Z][A-Z0-9_]*`.
fn is_screaming_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn encode_tokens(doc: &DocumentState, byte_range: Option<(u32, u32)>) -> SemanticTokens {
    let lex_result = luck_lexer::lex(&doc.text, doc.target.lua_version());
    let tokens = &lex_result.tokens;
    let lib = library_for(doc.target.lua_version(), doc.target.stdlib_environment());
    let tree = &doc.semantic.scope_tree;

    let mut ident_at_offset: HashMap<u32, usize> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        if matches!(token.kind, TokenKind::Identifier(_)) {
            ident_at_offset.insert(token.span.start, idx);
        }
    }

    let mut raw: Vec<(u32, u32, u32, u32, u32)> = Vec::new();
    let mut push = |span: Span, ty: u32, modifiers: u32| {
        if let Some((start, end)) = byte_range {
            if span.end < start || span.start > end {
                return;
            }
        }
        let pos = doc.line_index.position(&doc.text, span.start);
        let length = utf16_len(&doc.text, span.start, span.end);
        raw.push((pos.line, pos.character, length, ty, modifiers));
    };

    let classify_bare = |name: &str, span: Span, base: (u32, u32)| -> (u32, u32) {
        let (mut ty, mut modifiers) = base;
        if ty == TY_VARIABLE {
            if let Some(&idx) = ident_at_offset.get(&span.start) {
                match value_shape(tokens, idx, lib) {
                    ValueShape::Called | ValueShape::AssignedFunction => ty = TY_FUNCTION,
                    ValueShape::AssignedStdlibFunction => {
                        ty = TY_FUNCTION;
                        modifiers |= MOD_DEFAULT_LIBRARY;
                    }
                    ValueShape::Plain => {
                        if is_screaming_case(name) {
                            modifiers |= MOD_READONLY;
                        }
                    }
                }
            }
        }
        (ty, modifiers)
    };

    for symbol in &tree.symbols {
        if symbol.name == "self"
            || !symbol
                .name
                .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        {
            continue;
        }
        let base = match symbol.kind {
            SymbolKind::Parameter => (TY_PARAMETER, 0),
            SymbolKind::FunctionName => (TY_FUNCTION, 0),
            _ => (TY_VARIABLE, 0),
        };
        let (ty, modifiers) = classify_bare(&symbol.name, symbol.definition_span, base);
        push(symbol.definition_span, ty, modifiers);
    }

    for reference in &tree.references {
        if reference.name == "self" {
            continue;
        }
        let base = match reference.resolved {
            Some(symbol_id) => match tree.symbol(symbol_id).kind {
                SymbolKind::Parameter => (TY_PARAMETER, 0),
                SymbolKind::FunctionName => (TY_FUNCTION, 0),
                _ => (TY_VARIABLE, 0),
            },
            None => match lib.globals.get(reference.name.as_str()) {
                Some(entry) => stdlib_classify(entry),
                None => (TY_VARIABLE, 0),
            },
        };
        let (ty, modifiers) = classify_bare(&reference.name, reference.span, base);
        push(reference.span, ty, modifiers);
    }

    for (idx, token) in tokens.iter().enumerate() {
        let TokenKind::Identifier(name) = &token.kind else {
            continue;
        };
        let Some(prev) = idx.checked_sub(1).and_then(|i| tokens.get(i)) else {
            continue;
        };
        if !matches!(prev.kind, TokenKind::Dot | TokenKind::Colon) {
            continue;
        }
        // Walk the whole chain back to its root so nested paths classify
        // too (`Enum.Material.Grass`, `game:GetService`, `f:read` on a
        // shaped local - not just `math.floor`). The final separator may
        // be `.` or `:`; the base chain is dots only.
        let stdlib_member = {
            let mut segments: Vec<&str> = vec![name.as_str()];
            let mut sep_idx = idx - 1;
            let root_idx = loop {
                let Some(base_idx) = sep_idx.checked_sub(1) else {
                    break None;
                };
                let TokenKind::Identifier(base) = &tokens[base_idx].kind else {
                    break None;
                };
                segments.push(base.as_str());
                match base_idx.checked_sub(1).map(|i| &tokens[i].kind) {
                    Some(TokenKind::Dot) => sep_idx = base_idx - 1,
                    _ => break Some(base_idx),
                }
            };
            root_idx.and_then(|root_idx| {
                segments.reverse();
                doc.semantic
                    .resolve_stdlib_path(&segments, tokens[root_idx].span)
                    .map(member_classify)
            })
        };
        let (ty, modifiers) =
            stdlib_member.unwrap_or_else(|| match value_shape(tokens, idx, lib) {
                ValueShape::Called | ValueShape::AssignedFunction => (TY_METHOD, 0),
                ValueShape::AssignedStdlibFunction => (TY_FUNCTION, MOD_DEFAULT_LIBRARY),
                ValueShape::Plain => {
                    let modifiers = if is_screaming_case(name) {
                        MOD_READONLY
                    } else {
                        0
                    };
                    (TY_PROPERTY, modifiers)
                }
            });
        push(token.span, ty, modifiers);
    }

    raw.sort_by_key(|(line, character, _, _, _)| (*line, *character));
    raw.dedup_by_key(|(line, character, _, _, _)| (*line, *character));

    let mut data: Vec<SemanticToken> = Vec::with_capacity(raw.len());
    let mut prev = Position {
        line: 0,
        character: 0,
    };
    for (line, character, length, token_type, token_modifiers_bitset) in raw {
        let delta_line = line - prev.line;
        let delta_start = if delta_line == 0 {
            character - prev.character
        } else {
            character
        };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type,
            token_modifiers_bitset,
        });
        prev = Position { line, character };
    }

    SemanticTokens {
        result_id: None,
        data,
    }
}

fn stdlib_classify(entry: &StdlibEntry) -> (u32, u32) {
    let mut modifiers = MOD_DEFAULT_LIBRARY;
    let ty = match entry {
        StdlibEntry::Function(f) => {
            if f.deprecated.is_some() {
                modifiers |= MOD_DEPRECATED;
            }
            if f.read_only {
                modifiers |= MOD_READONLY;
            }
            TY_FUNCTION
        }
        StdlibEntry::Namespace(_) => {
            modifiers |= MOD_READONLY;
            TY_NAMESPACE
        }
        StdlibEntry::Constant(v) | StdlibEntry::Property(v) => {
            if v.deprecated.is_some() {
                modifiers |= MOD_DEPRECATED;
            }
            if v.read_only {
                modifiers |= MOD_READONLY;
            }
            TY_PROPERTY
        }
    };
    (ty, modifiers)
}

/// Stdlib namespace members: same modifiers as bare globals, but member
/// functions read as methods so call sites match the accessor grammar.
fn member_classify(entry: &StdlibEntry) -> (u32, u32) {
    let (ty, modifiers) = stdlib_classify(entry);
    if ty == TY_FUNCTION {
        (TY_METHOD, modifiers)
    } else {
        (ty, modifiers)
    }
}

fn utf16_len(source: &str, start: u32, end: u32) -> u32 {
    let s = start as usize;
    let e = (end as usize).min(source.len());
    if s >= e {
        return 0;
    }
    source[s..e].chars().map(|c| c.len_utf16() as u32).sum()
}
