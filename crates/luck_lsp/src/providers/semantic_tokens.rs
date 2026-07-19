//! Semantic tokens. The lexer is the cheapest source for "every name and
//! keyword in the file" - we re-run it and classify each identifier
//! against the stdlib model so deprecated/Roblox names get distinct
//! modifiers.

use luck_semantic::stdlib_model::{EntryKind, StdlibEntry, library_for};
use luck_token::TokenKind;
use tower_lsp::lsp_types::{
    Position, Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensLegend, SemanticTokensRangeResult, SemanticTokensResult,
};

use crate::backend::DocumentState;

pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::KEYWORD,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::COMMENT,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::TYPE,
];

const TY_NAMESPACE: u32 = 0;
const TY_FUNCTION: u32 = 1;
const TY_VARIABLE: u32 = 3;
const TY_PROPERTY: u32 = 4;
const TY_KEYWORD: u32 = 5;
const TY_STRING: u32 = 6;
const TY_NUMBER: u32 = 7;
const TY_OPERATOR: u32 = 9;

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

fn encode_tokens(doc: &DocumentState, byte_range: Option<(u32, u32)>) -> SemanticTokens {
    let result = luck_lexer::lex(&doc.text, doc.target.lua_version());
    let lib = library_for(doc.target.lua_version());
    let environment = doc.target.stdlib_environment();

    let mut raw: Vec<(u32, u32, u32, u32, u32)> = Vec::with_capacity(result.tokens.len());

    for token in &result.tokens {
        if let Some((start, end)) = byte_range {
            if token.span.end < start || token.span.start > end {
                continue;
            }
        }
        let pos = doc.line_index.position(&doc.text, token.span.start);
        let length = utf16_len(&doc.text, token.span.start, token.span.end);
        let (ty, modifiers) = classify(token, lib, environment);
        let Some(ty) = ty else { continue };
        raw.push((pos.line, pos.character, length, ty, modifiers));
    }

    raw.sort_by_key(|(line, character, _, _, _)| (*line, *character));

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

fn classify(
    token: &luck_token::Token,
    lib: &'static luck_semantic::stdlib_model::StdlibLibrary,
    environment: luck_token::StdlibEnvironment,
) -> (Option<u32>, u32) {
    use TokenKind::*;
    match &token.kind {
        Identifier(name) => {
            if let Some(entry) = lib
                .globals
                .get(name.as_str())
                .filter(|entry| entry.available_in_luau(environment))
            {
                let (ty, modifiers) = stdlib_classify(entry, MOD_DEFAULT_LIBRARY);
                (Some(ty), modifiers)
            } else {
                (Some(TY_VARIABLE), 0)
            }
        }
        Number(_) => (Some(TY_NUMBER), 0),
        StringLiteral(_) => (Some(TY_STRING), 0),
        And | Break | Do | Else | ElseIf | End | False | For | Function | If | In | Local | Nil
        | Not | Or | Repeat | Return | Then | True | Until | While | Goto | Global => {
            (Some(TY_KEYWORD), 0)
        }
        Plus | Minus | Star | Slash | FloorDiv | Percent | Caret | Hash | Equal | EqualEqual
        | TildeEqual | Less | Greater | LessEqual | GreaterEqual | DotDot | DotDotDot
        | Ampersand | Pipe | Tilde | ShiftLeft | ShiftRight | Arrow | Question | At | PlusEqual
        | MinusEqual | StarEqual | SlashEqual | FloorDivEqual | PercentEqual | CaretEqual
        | DotDotEqual => (Some(TY_OPERATOR), 0),
        _ => (None, 0),
    }
}

fn stdlib_classify(entry: &StdlibEntry, base_modifiers: u32) -> (u32, u32) {
    let mut modifiers = base_modifiers;
    let ty = match &entry.kind {
        EntryKind::Function(f) => {
            if f.deprecated.is_some() {
                modifiers |= MOD_DEPRECATED;
            }
            if f.read_only {
                modifiers |= MOD_READONLY;
            }
            TY_FUNCTION
        }
        EntryKind::Namespace(_) => {
            modifiers |= MOD_READONLY;
            TY_NAMESPACE
        }
        EntryKind::Constant(v) | EntryKind::Property(v) => {
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

fn utf16_len(source: &str, start: u32, end: u32) -> u32 {
    let s = start as usize;
    let e = (end as usize).min(source.len());
    if s >= e {
        return 0;
    }
    source[s..e].chars().map(|c| c.len_utf16() as u32).sum()
}
