//! A tree built in code can hold a leaf no source spells. Both printers
//! refuse such a tree with the same error, at the leaf, and print the same
//! tree with that leaf fixed into text that reads back as it.

use luck_ast::{
    Attribute, AttributedName, Block, CallSuffix, Expression, Field, FunctionArgs,
    FunctionAttribute, FunctionBody, FunctionDecl, FunctionName, FunctionSignature, FunctionType,
    FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam, GlobalFunction,
    GlobalStar, GotoStatement, InterpolatedString, InterpolatedStringPart,
    InterpolatedStringSegment, LabelStatement, LastStatement, Literal, LocalFunction, NamedType,
    NumericFor, OneOrMore, Parameter, Punctuated, Statement, Suffix, SuffixedExpression, Synth,
    TableConstructor, TableType, Type, TypeArgs, TypeDeclaration, TypeDeclarationValue, TypeField,
    UnionType, Var, VarArgParam,
};
use luck_formatter::{Comments, FormatOptions, blocks_equivalent, format_block};
use luck_token::{CompactString, LuaVersion, SourceError, Span, Token, TokenKind};

/// Far from every span a [`Synth`] starting at 0 hands out here.
const LEAF_SPAN: Span = Span::new(90_000, 90_000);

/// Both printers refuse `block` with one error, which this returns.
fn refusal(block: &Block, label: &str) -> SourceError {
    let compacted = luck_codegen::compact(block);
    let formatted = format_block(block, Comments::none(), &FormatOptions::default());
    let (Err(compact_error), Err(format_error)) = (compacted, formatted) else {
        panic!("{label}: both printers must refuse the tree");
    };
    assert_eq!(compact_error, format_error, "{label}: the printers refuse alike");
    compact_error
}

/// Both printers print `block` as text that parses clean under `version` back
/// into an equivalent tree.
fn assert_prints(block: &Block, version: LuaVersion, label: &str) {
    let compacted = luck_codegen::compact(block).unwrap_or_else(|e| panic!("{label}: {e:?}"));
    let formatted = format_block(block, Comments::none(), &FormatOptions::default())
        .unwrap_or_else(|e| panic!("{label}: {e:?}"));
    for output in [compacted, formatted] {
        let reparsed = luck_parser::parse(output.as_str(), version);
        assert!(reparsed.errors.is_empty(), "{label}: {output:?}: {:?}", reparsed.errors);
        let equivalence = blocks_equivalent(block, &reparsed.block, version);
        assert!(equivalence.is_ok(), "{label}: {output:?}: {equivalence:?}");
    }
}

fn identifier(name: &str) -> Token {
    Token::new(TokenKind::Identifier(name.into()), LEAF_SPAN)
}

fn returning(synth: &Synth, expr: Expression) -> Block {
    synth.block(Vec::new(), Some(synth.return_(vec![expr])))
}

fn stmt_block(synth: &Synth, stmt: Statement) -> Block {
    synth.block(vec![stmt], None)
}

fn empty_body(synth: &Synth) -> FunctionBody {
    FunctionBody {
        span: Span::default(),
        generics: None,
        params: Punctuated::empty(),
        vararg: None,
        return_type: None,
        block: synth.block(Vec::new(), None),
    }
}

fn chain(prefix: Expression, suffix: Suffix) -> Expression {
    Expression::Suffixed(Box::new(SuffixedExpression {
        span: Span::default(),
        prefix,
        suffixes: OneOrMore::new(suffix),
    }))
}

fn call_suffix(method: Option<Token>, args: FunctionArgs) -> Suffix {
    Suffix::Call(CallSuffix { method, explicit_type_args: None, args })
}

fn no_args() -> FunctionArgs {
    FunctionArgs::Parenthesized { span: Span::default(), args: Punctuated::empty() }
}

fn type_alias(synth: &Synth, value: Type) -> Block {
    stmt_block(synth, synth.type_declaration(false, "T", None, value))
}

fn named_type(prefix: Option<Token>, name: Token, generics: Option<Vec<Type>>) -> Type {
    Type::Named(Box::new(NamedType {
        span: Span::default(),
        prefix,
        name,
        generics: generics
            .map(|args| TypeArgs { span: Span::default(), args: Punctuated::from_items(args) }),
    }))
}

fn table_type(field: TypeField) -> Type {
    Type::Table(Box::new(TableType { span: Span::default(), fields: Punctuated::from_item(field) }))
}

type NameBuilder = fn(&Synth, Token) -> Block;

/// A label, the dialect the position parses in, an identifier valid there, and
/// the tree with a given name token in the position.
type NamePosition = (&'static str, LuaVersion, &'static str, NameBuilder);

/// Every place a tree holds a name token, with the dialect it parses in and
/// an identifier that's valid there.
fn name_positions() -> Vec<NamePosition> {
    let mut positions = statement_name_positions();
    positions.extend(declaration_name_positions());
    positions.extend(type_name_positions());
    positions
}

/// The names an expression or a statement other than a declaration holds.
fn statement_name_positions() -> Vec<NamePosition> {
    vec![
        ("name expression", LuaVersion::Lua54, "ok", |synth, name| {
            returning(synth, Expression::Name(name))
        }),
        ("assignment target", LuaVersion::Lua54, "ok", |synth, name| {
            stmt_block(synth, synth.assign(vec![Var::Name(name)], vec![synth.nil()]))
        }),
        ("field", LuaVersion::Lua54, "ok", |synth, name| {
            returning(synth, chain(synth.name_expr("t"), Suffix::Field(name)))
        }),
        ("method", LuaVersion::Lua54, "ok", |synth, name| {
            returning(synth, chain(synth.name_expr("t"), call_suffix(Some(name), no_args())))
        }),
        ("table field name", LuaVersion::Lua54, "ok", |synth, name| {
            let field = Field::Named { span: Span::default(), name, value: synth.nil() };
            let table =
                TableConstructor { span: Span::default(), fields: Punctuated::from_item(field) };
            returning(synth, Expression::TableConstructor(Box::new(table)))
        }),
        ("goto label", LuaVersion::Lua54, "ok", |synth, name| {
            let goto = Statement::Goto(Box::new(GotoStatement { span: Span::default(), name }));
            synth.block(vec![goto, synth.label("ok")], None)
        }),
        ("label", LuaVersion::Lua54, "ok", |synth, name| {
            stmt_block(
                synth,
                Statement::Label(Box::new(LabelStatement { span: Span::default(), name })),
            )
        }),
        ("numeric for name", LuaVersion::Lua54, "ok", |synth, name| {
            let numeric_for = NumericFor {
                span: Span::default(),
                name,
                type_annotation: None,
                start: synth.number("1"),
                limit: synth.number("2"),
                step: None,
                block: synth.block(Vec::new(), None),
            };
            stmt_block(synth, Statement::NumericFor(Box::new(numeric_for)))
        }),
        ("generic for name", LuaVersion::Lua54, "ok", |synth, name| {
            let binding = Parameter { span: Span::default(), name, type_annotation: None };
            let body = synth.block(Vec::new(), None);
            stmt_block(synth, synth.generic_for(vec![binding], vec![synth.name_expr("t")], body))
        }),
    ]
}

/// The names a declaration or a function holds.
fn declaration_name_positions() -> Vec<NamePosition> {
    vec![
        ("function name", LuaVersion::Lua54, "ok", |synth, name| {
            let function_name =
                FunctionName { span: Span::default(), names: vec![name], method: None };
            let decl = FunctionDecl {
                span: Span::default(),
                attributes: Vec::new(),
                name: function_name,
                body: empty_body(synth),
            };
            stmt_block(synth, Statement::FunctionDecl(Box::new(decl)))
        }),
        ("function method name", LuaVersion::Lua54, "ok", |synth, name| {
            let function_name = FunctionName {
                span: Span::default(),
                names: vec![synth.identifier("t")],
                method: Some(name),
            };
            let decl = FunctionDecl {
                span: Span::default(),
                attributes: Vec::new(),
                name: function_name,
                body: empty_body(synth),
            };
            stmt_block(synth, Statement::FunctionDecl(Box::new(decl)))
        }),
        ("local function name", LuaVersion::Lua54, "ok", |synth, name| {
            let function = LocalFunction {
                span: Span::default(),
                attributes: Vec::new(),
                name,
                body: empty_body(synth),
                is_const: false,
                is_exported: false,
            };
            stmt_block(synth, Statement::LocalFunction(Box::new(function)))
        }),
        ("global function name", LuaVersion::Lua55, "ok", |synth, name| {
            let function = GlobalFunction { span: Span::default(), name, body: empty_body(synth) };
            stmt_block(synth, Statement::GlobalFunction(Box::new(function)))
        }),
        ("local name", LuaVersion::Lua54, "ok", |synth, name| {
            let binding = AttributedName { name, type_annotation: None, attribute: None };
            stmt_block(synth, synth.local_full(vec![binding], vec![synth.nil()]))
        }),
        ("local attribute", LuaVersion::Lua54, "const", |synth, name| {
            let attribute = Some(Box::new(Attribute { span: Span::default(), name }));
            let binding =
                AttributedName { name: synth.identifier("x"), type_annotation: None, attribute };
            stmt_block(synth, synth.local_full(vec![binding], vec![synth.nil()]))
        }),
        ("global star attribute", LuaVersion::Lua55, "const", |synth, name| {
            let attribute = Some(Attribute { span: Span::default(), name });
            stmt_block(
                synth,
                Statement::GlobalStar(Box::new(GlobalStar { span: Span::default(), attribute })),
            )
        }),
        ("parameter", LuaVersion::Lua54, "ok", |synth, name| {
            let param = Parameter { span: Span::default(), name, type_annotation: None };
            let signature =
                FunctionSignature { params: vec![param], ..FunctionSignature::default() };
            returning(
                synth,
                synth.function_def_full(Vec::new(), signature, synth.block(Vec::new(), None)),
            )
        }),
        ("vararg name", LuaVersion::Lua55, "ok", |synth, name| {
            let vararg =
                VarArgParam { span: Span::default(), name: Some(name), type_annotation: None };
            let signature =
                FunctionSignature { vararg: Some(vararg), ..FunctionSignature::default() };
            returning(
                synth,
                synth.function_def_full(Vec::new(), signature, synth.block(Vec::new(), None)),
            )
        }),
        ("function attribute", LuaVersion::Luau, "native", |synth, name| {
            let attribute = FunctionAttribute { span: Span::default(), name, args: None };
            let body = synth.block(Vec::new(), None);
            stmt_block(
                synth,
                synth.local_function_full("f", vec![attribute], FunctionSignature::default(), body),
            )
        }),
    ]
}

/// The names Luau type syntax holds.
fn type_name_positions() -> Vec<NamePosition> {
    vec![
        ("type declaration name", LuaVersion::Luau, "ok", |synth, name| {
            let declaration = TypeDeclaration {
                span: Span::default(),
                is_exported: false,
                name,
                generics: None,
                type_value: TypeDeclarationValue::Alias(synth.ty_named("number")),
            };
            stmt_block(synth, Statement::TypeDeclaration(Box::new(declaration)))
        }),
        ("generic type parameter", LuaVersion::Luau, "ok", |synth, name| {
            let param =
                GenericTypeParam { span: Span::default(), name, is_pack: false, default: None };
            let generics =
                GenericTypeList { span: Span::default(), params: Punctuated::from_item(param) };
            let declaration = TypeDeclaration {
                span: Span::default(),
                is_exported: false,
                name: synth.identifier("T"),
                generics: Some(Box::new(generics)),
                type_value: TypeDeclarationValue::Alias(synth.ty_named("number")),
            };
            stmt_block(synth, Statement::TypeDeclaration(Box::new(declaration)))
        }),
        ("type module", LuaVersion::Luau, "ok", |synth, name| {
            type_alias(synth, named_type(Some(name), synth.identifier("X"), None))
        }),
        ("qualified type name", LuaVersion::Luau, "ok", |synth, name| {
            type_alias(synth, named_type(Some(synth.identifier("m")), name, None))
        }),
        ("generic type name", LuaVersion::Luau, "ok", |synth, name| {
            type_alias(synth, named_type(None, name, Some(vec![synth.ty_named("number")])))
        }),
        ("generic pack", LuaVersion::Luau, "ok", |synth, name| {
            let pack = Type::GenericPack(Box::new(GenericPackType { span: Span::default(), name }));
            type_alias(synth, synth.ty_function(Vec::new(), pack))
        }),
        ("table type field name", LuaVersion::Luau, "ok", |synth, name| {
            let field = TypeField::Named {
                span: Span::default(),
                access: None,
                name,
                value: synth.ty_named("number"),
            };
            type_alias(synth, table_type(field))
        }),
        ("function type parameter", LuaVersion::Luau, "ok", |synth, name| {
            let param = FunctionTypeParam {
                span: Span::default(),
                name: Some(name),
                type_value: synth.ty_named("number"),
            };
            let function = FunctionType {
                span: Span::default(),
                generics: None,
                params: Punctuated::from_item(param),
                return_type: synth.ty_pack(Vec::new()),
            };
            type_alias(synth, Type::Function(Box::new(function)))
        }),
    ]
}

#[test]
fn a_name_token_with_no_name_spelling_is_refused_wherever_a_name_belongs() {
    let bad_names = [
        (TokenKind::End, "`end`"),
        (TokenKind::Plus, "`+`"),
        (TokenKind::Eof, "end of file"),
        (TokenKind::Number("1".into()), "number `1`"),
        (TokenKind::StringLiteral("\"s\"".into()), "string"),
        (TokenKind::Identifier("".into()), "nothing"),
        (TokenKind::Identifier("end".into()), "`end`"),
        (TokenKind::Identifier("1x".into()), "`1x`"),
        (TokenKind::Identifier("a\nb".into()), "`a\\nb`"),
    ];
    for (label, version, valid, build) in name_positions() {
        let synth = Synth::new();
        assert_prints(&build(&synth, identifier(valid)), version, label);
        for (kind, found) in &bad_names {
            let error = refusal(&build(&synth, Token::new(kind.clone(), LEAF_SPAN)), label);
            assert_eq!(error.span, LEAF_SPAN, "{label}");
            assert_eq!(error.message, format!("expected identifier, found {found}"), "{label}");
        }
    }
}

#[test]
fn typeof_is_refused_only_where_a_name_starts_a_type() {
    let starts_a_type = ["generic type name", "generic pack"];
    for (label, version, _, build) in name_positions() {
        let synth = Synth::new();
        let block = build(&synth, identifier("typeof"));
        if starts_a_type.contains(&label) {
            let error = refusal(&block, label);
            assert_eq!(error.span, LEAF_SPAN, "{label}");
            assert_eq!(error.message, "expected type name, found `typeof`", "{label}");
            continue;
        }
        if version.has_type_syntax() && label != "function attribute" {
            assert_prints(&block, version, label);
        }
    }
    let synth = Synth::new();
    let bare = type_alias(&synth, Type::Name { name: "typeof".into(), span: LEAF_SPAN });
    assert_eq!(refusal(&bare, "bare type name").message, "expected type name, found `typeof`");
    let qualified =
        type_alias(&synth, named_type(Some(identifier("typeof")), identifier("X"), None));
    assert_prints(&qualified, LuaVersion::Luau, "`typeof.X`");
}

#[test]
fn a_type_name_with_no_name_spelling_is_refused() {
    let synth = Synth::new();
    for (text, found) in [("", "nothing"), ("end", "`end`"), ("nil", "`nil`"), ("a.b", "`a.b`")] {
        let block = type_alias(&synth, Type::Name { name: text.into(), span: LEAF_SPAN });
        let error = refusal(&block, text);
        assert_eq!(
            (error.span, error.message),
            (LEAF_SPAN, format!("expected type name, found {found}"))
        );
    }
}

type LiteralBuilder = fn(&Synth, Literal) -> Block;

/// A label, the tree with a literal in the position, texts that print with
/// the dialect each parses in, texts refused there, and how the refusal opens.
type LiteralCase = (
    &'static str,
    LiteralBuilder,
    &'static [(&'static str, LuaVersion)],
    &'static [&'static str],
    &'static str,
);

#[test]
fn a_literal_whose_text_is_not_one_token_of_its_kind_is_refused() {
    let number: LiteralBuilder = |synth, literal| returning(synth, Expression::Number(literal));
    let integer: LiteralBuilder = |synth, literal| returning(synth, Expression::Integer(literal));
    let string: LiteralBuilder =
        |synth, literal| returning(synth, Expression::StringLiteral(literal));
    let string_argument: LiteralBuilder = |synth, literal| {
        let call = call_suffix(None, FunctionArgs::StringLiteral(literal));
        stmt_block(synth, synth.call_stmt(chain(synth.name_expr("f"), call)))
    };
    let cases: [LiteralCase; 4] = [
        (
            "number",
            number,
            &[
                ("1", LuaVersion::Lua51),
                ("0x1.8p3", LuaVersion::Lua54),
                ("1_000", LuaVersion::Luau),
                (".5", LuaVersion::Lua51),
            ],
            &["", "x", "-1", "1..2", "0x", "1i", "1 "],
            "expected number literal",
        ),
        (
            "integer",
            integer,
            &[("1i", LuaVersion::Luau), ("0xffi", LuaVersion::Luau)],
            &["1", "1.5i", "i", "99999999999999999999i"],
            "expected integer literal",
        ),
        (
            "string",
            string,
            &[
                ("\"s\"", LuaVersion::Lua51),
                ("'s'", LuaVersion::Lua51),
                ("[==[a]]b]==]", LuaVersion::Lua54),
                ("\"\\u{41}\"", LuaVersion::Lua54),
            ],
            &["", "s", "\"s", "'s\"", "\"a\nb\"", "\"a\"b\"", "[[a]]]", "\"\\256\""],
            "expected string literal",
        ),
        (
            "string argument",
            string_argument,
            &[("\"s\"", LuaVersion::Lua51)],
            &["s", "\"s"],
            "expected string literal",
        ),
    ];
    for (label, build, valid_texts, bad_texts, expected) in cases {
        let synth = Synth::new();
        for &(text, version) in valid_texts {
            let literal = Literal { text: CompactString::from(text), span: LEAF_SPAN };
            assert_prints(&build(&synth, literal), version, &format!("{label} {text:?}"));
        }
        for text in bad_texts {
            let literal = Literal { text: CompactString::from(*text), span: LEAF_SPAN };
            let error = refusal(&build(&synth, literal), &format!("{label} {text:?}"));
            assert_eq!(error.span, LEAF_SPAN, "{label} {text:?}");
            assert!(error.message.starts_with(expected), "{label} {text:?}: {error:?}");
        }
    }
}

#[test]
fn a_singleton_type_prints_only_a_singleton_token() {
    let synth = Synth::new();
    let singleton =
        |kind: TokenKind| type_alias(&synth, Type::Singleton(Token::new(kind, LEAF_SPAN)));
    for kind in [
        TokenKind::True,
        TokenKind::False,
        TokenKind::Nil,
        TokenKind::StringLiteral("'s'".into()),
        TokenKind::Number("1".into()),
    ] {
        assert_prints(&singleton(kind.clone()), LuaVersion::Luau, &format!("{kind:?}"));
    }
    for (kind, message) in [
        (TokenKind::End, "expected singleton type, found `end`"),
        (TokenKind::Identifier("x".into()), "expected singleton type, found `x`"),
        (TokenKind::Eof, "expected singleton type, found end of file"),
        (TokenKind::StringLiteral("s".into()), "expected string literal, found `s`"),
        (TokenKind::Number("1x".into()), "expected number literal, found `1x`"),
    ] {
        let error = refusal(&singleton(kind.clone()), &format!("{kind:?}"));
        assert_eq!((error.span, error.message.as_str()), (LEAF_SPAN, message));
    }
}

#[test]
fn a_table_type_access_modifier_is_read_or_write() {
    let synth = Synth::new();
    let with_access = |access: Token| {
        let field = TypeField::Named {
            span: Span::default(),
            access: Some(access),
            name: synth.identifier("x"),
            value: synth.ty_named("number"),
        };
        type_alias(&synth, table_type(field))
    };
    for access in ["read", "write"] {
        assert_prints(&with_access(identifier(access)), LuaVersion::Luau, access);
    }
    for (access, found) in
        [(identifier("readonly"), "`readonly`"), (Token::new(TokenKind::End, LEAF_SPAN), "`end`")]
    {
        let error = refusal(&with_access(access), found);
        assert_eq!(error.message, format!("expected `read` or `write`, found {found}"));
    }
}

#[test]
fn an_array_type_field_cannot_open_with_a_bare_access_word() {
    let synth = Synth::new();
    let array = |access: Option<&str>, value: Type| {
        let field =
            TypeField::Array { span: Span::default(), access: access.map(identifier), value };
        type_alias(&synth, table_type(field))
    };
    let union = |has_leading_pipe: bool| {
        Type::Union(Box::new(UnionType {
            span: Span::default(),
            has_leading_pipe,
            types: Punctuated::from_items(vec![
                Type::Name { name: "write".into(), span: LEAF_SPAN },
                synth.ty_named("number"),
            ]),
        }))
    };
    for (label, block) in [
        ("bare", array(None, Type::Name { name: "read".into(), span: LEAF_SPAN })),
        ("qualified", array(None, named_type(Some(identifier("read")), identifier("X"), None))),
        (
            "optional",
            array(None, synth.ty_optional(Type::Name { name: "read".into(), span: LEAF_SPAN })),
        ),
        ("union", array(None, union(false))),
    ] {
        let error = refusal(&block, label);
        assert_eq!(error.span, LEAF_SPAN, "{label}");
        assert!(
            error.message.ends_with("opening a table type field reads back as an access modifier"),
            "{label}"
        );
    }
    assert_prints(
        &array(Some("read"), synth.ty_named("read")),
        LuaVersion::Luau,
        "after an access",
    );
    assert_prints(&array(None, union(true)), LuaVersion::Luau, "after a leading `|`");
    // `Synth` spells such an element out instead.
    let built = type_alias(&synth, synth.ty_table_array(synth.ty_named("write")));
    assert_prints(&built, LuaVersion::Luau, "built");
}

#[test]
fn an_interpolated_string_prints_only_the_segments_the_lexer_reads() {
    let synth = Synth::new();
    let segment = |kind: TokenKind, expr: Option<Expression>| InterpolatedStringSegment {
        literal: Token::new(kind, LEAF_SPAN),
        expr,
    };
    let string = |segments: Vec<InterpolatedStringSegment>| {
        returning(
            &synth,
            Expression::InterpolatedString(Box::new(InterpolatedString {
                span: LEAF_SPAN,
                segments,
            })),
        )
    };
    let begin = |text: &str| TokenKind::InterpolatedStringBegin(text.into());
    let middle = |text: &str| TokenKind::InterpolatedStringMiddle(text.into());
    let end = |text: &str| TokenKind::InterpolatedStringEnd(text.into());
    let hole = || Some(synth.name_expr("x"));

    let parts = vec![
        InterpolatedStringPart::Text("a{b}`c\\"),
        InterpolatedStringPart::Expr(synth.name_expr("x")),
        InterpolatedStringPart::Expr(synth.name_expr("y")),
    ];
    assert_prints(&returning(&synth, synth.interpolated_string(parts)), LuaVersion::Luau, "built");

    for (label, segments, message) in [
        ("one segment", vec![segment(end("a"), None)], "malformed interpolated string"),
        (
            "holeless begin text",
            vec![segment(begin("a"), None), segment(end("b"), None)],
            "malformed interpolated string",
        ),
        (
            "hole after end",
            vec![segment(begin(""), hole()), segment(end(""), hole())],
            "malformed interpolated string",
        ),
        (
            "middle first",
            vec![segment(middle(""), hole()), segment(end(""), None)],
            "malformed interpolated string",
        ),
        (
            "middle without a hole",
            vec![segment(begin(""), hole()), segment(middle(""), None), segment(end(""), None)],
            "malformed interpolated string",
        ),
        (
            "string token",
            vec![segment(TokenKind::StringLiteral("\"a\"".into()), hole()), segment(end(""), None)],
            "malformed interpolated string",
        ),
        (
            "open brace",
            vec![segment(begin("a{"), hole()), segment(end(""), None)],
            "expected interpolated string text, found `a{`",
        ),
        (
            "backtick",
            vec![segment(begin(""), hole()), segment(end("`"), None)],
            "expected interpolated string text, found ```",
        ),
        (
            "trailing backslash",
            vec![segment(begin("a\\"), hole()), segment(end(""), None)],
            "expected interpolated string text, found `a\\\\`",
        ),
    ] {
        let error = refusal(&string(segments), label);
        assert_eq!((error.span, error.message.as_str()), (LEAF_SPAN, message), "{label}");
    }
}

#[test]
fn an_error_node_is_refused_wherever_it_stands() {
    let synth = Synth::new();
    for (label, block) in [
        ("expression", returning(&synth, Expression::Error(LEAF_SPAN))),
        ("statement", stmt_block(&synth, Statement::Error(LEAF_SPAN))),
        ("last statement", synth.block(Vec::new(), Some(LastStatement::Error(LEAF_SPAN)))),
        (
            "nested statement",
            stmt_block(&synth, synth.do_block(stmt_block(&synth, Statement::Error(LEAF_SPAN)))),
        ),
        ("type", type_alias(&synth, Type::Error(LEAF_SPAN))),
    ] {
        let error = refusal(&block, label);
        assert_eq!(
            (error.span, error.message.as_str()),
            (LEAF_SPAN, "an error node has no text"),
            "{label}"
        );
    }
}
