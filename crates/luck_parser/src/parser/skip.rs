//! A construct the depth cap refuses is skipped by its tokens, not parsed.
//!
//! The skips loop over a heap stack, so they take no more host stack however
//! deep the construct nests. Each stops where parsing the construct would have
//! stopped, so the parser goes on from there as if it had.
//!
//! A closer closes the innermost open construct it can, and closes everything
//! open inside that construct with it, as parsing does when what's inside is
//! missing its own closer. As in parsing, `end` and `until` each close any
//! block. A closer that nothing open in the skip can close ends the skip
//! before it when something the parser has open can. Otherwise it's stray: a
//! closing bracket stands for the innermost open bracket, as a typo for its
//! closer, and anything else is passed over.

use luck_token::{TokenKind, UnOp};

use super::{Closer, OpenConstructs, Parser};

/// An open construct the skip is inside.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    /// `(`, `[`, or `{`, closed by its own kind of closing bracket.
    Bracket(Closer),
    /// A function's parameter list, which a Luau return type may follow.
    Params,
    /// A Luau interpolated string, closed by its `InterpolatedStringEnd`.
    InterpolatedString,
    /// A type's `<`, closed by `>`.
    Angle,
    /// `do`, `function`, or a loop past its `do`, closed by `end`.
    End,
    /// A statement `if`, closed by `end` and continued by `else` and `elseif`.
    If,
    /// `while` or `for` before its `do`.
    LoopHeader,
    /// `repeat`, closed by `until`.
    Until,
    /// A Luau if-expression, which has no closing token and ends at its
    /// `else`.
    IfExpr,
}

impl FrameKind {
    const fn closes_at(self, closer: Closer) -> bool {
        match (self, closer) {
            (Self::Bracket(Closer::Paren) | Self::Params, Closer::Paren)
            | (Self::Bracket(Closer::Bracket), Closer::Bracket)
            | (Self::Bracket(Closer::Brace), Closer::Brace)
            | (Self::InterpolatedString, Closer::StringSegment)
            | (
                Self::End | Self::If | Self::LoopHeader | Self::Until,
                Closer::End | Closer::Until,
            )
            | (Self::If | Self::IfExpr, Closer::Else) => true,
            (
                Self::Bracket(_)
                | Self::Params
                | Self::InterpolatedString
                | Self::Angle
                | Self::End
                | Self::If
                | Self::LoopHeader
                | Self::Until
                | Self::IfExpr,
                Closer::Paren
                | Closer::Bracket
                | Closer::Brace
                | Closer::StringSegment
                | Closer::End
                | Closer::Else
                | Closer::Until,
            ) => false,
        }
    }

    const fn is_bracket(self) -> bool {
        matches!(self, Self::Bracket(_) | Self::Params)
    }

    /// The closers that close or continue this frame.
    const fn closers(self) -> [Option<Closer>; 2] {
        match self {
            Self::Bracket(closer) => [Some(closer), None],
            Self::Params => [Some(Closer::Paren), None],
            Self::InterpolatedString => [Some(Closer::StringSegment), None],
            Self::Angle => [None, None],
            Self::End | Self::LoopHeader | Self::Until => [Some(Closer::End), None],
            Self::If => [Some(Closer::End), Some(Closer::Else)],
            Self::IfExpr => [Some(Closer::Else), None],
        }
    }

    /// Whether statements go here, so an `if` may start one and Luau types
    /// need following, since a type that ends in `>` ends an operand where a
    /// `>` operator would leave one to come.
    const fn holds_statements(self) -> bool {
        match self {
            Self::End | Self::If | Self::LoopHeader | Self::Until => true,
            Self::Bracket(_)
            | Self::Params
            | Self::InterpolatedString
            | Self::Angle
            | Self::IfExpr => false,
        }
    }
}

/// Where a frame that holds statements is in the Luau grammar around types.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Expression,
    /// The names after `local`, `const`, or `for`, which take annotations.
    Bindings {
        is_name_next: bool,
    },
    /// After `function`, up to its parameters.
    FunctionName,
    /// Right after a function's parameters, where a return type may follow.
    AfterParams,
    /// After a `type` declaration's keyword, up to its `=`.
    TypeAliasName,
    Type {
        is_primary_next: bool,
        is_in_bindings: bool,
    },
}

impl Context {
    /// Where the walk goes on once a type in this context ends.
    const fn after_type(is_in_bindings: bool) -> Self {
        if is_in_bindings { Self::Bindings { is_name_next: false } } else { Self::Expression }
    }
}

#[derive(Clone, Copy)]
struct Frame {
    kind: FrameKind,
    context: Context,
}

impl Frame {
    const fn new(kind: FrameKind) -> Self {
        Self { kind, context: Context::Expression }
    }
}

/// Where a skipped Luau if-expression is.
#[derive(Clone, Copy)]
enum IfPart {
    Condition,
    Branch,
    Else,
}

/// What a skipped expression takes after its operand at the top level.
#[derive(Clone, Copy)]
pub(super) enum ExprTail {
    /// Nothing, for a construct the parser reads as one simple expression.
    Nothing,
    /// A cast, then binary operators binding at least this tightly.
    Operators(u8),
}

/// How much of a type a skip takes, from narrowest to widest.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TypeExtent {
    /// One type with no `?`, `&`, or `|` after it, though a function type
    /// takes its whole return type.
    Primary,
    /// A type and its `?`s, as an intersection member is.
    IntersectionMember,
    /// That and any `&` members, as a union member is.
    UnionMember,
    /// A whole type, leading `|` or `&` included.
    Full,
}

/// Whether an `if` right after `kind` starts an expression rather than a
/// statement, where statements go.
const fn precedes_expression(kind: &TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Identifier(_)
            | TokenKind::Number(_)
            | TokenKind::Integer(_)
            | TokenKind::StringLiteral(_)
            | TokenKind::Nil
            | TokenKind::True
            | TokenKind::False
            | TokenKind::DotDotDot
            | TokenKind::RightParen
            | TokenKind::RightBracket
            | TokenKind::RightBrace
            | TokenKind::InterpolatedStringEnd(_)
            | TokenKind::Question
            | TokenKind::End
            | TokenKind::Do
            | TokenKind::Repeat
            | TokenKind::Break
            | TokenKind::Semicolon
    )
}

/// The frames a skip has open, innermost last, and whether an `if` next
/// starts an expression.
///
/// A closer looks for its frame only when one is open, and pops what it
/// looks past, so however the source mismatches them, the skip stays linear.
struct Skip {
    frames: Vec<Frame>,
    /// How many open frames each closer closes.
    open: OpenConstructs,
    /// Where each open frame other than a bracket is, innermost last.
    blocks: Vec<usize>,
    is_expr_next: bool,
}

impl Skip {
    fn new(frame: Frame, is_expr_next: bool) -> Self {
        let mut skip = Self {
            frames: Vec::new(),
            open: OpenConstructs::default(),
            blocks: Vec::new(),
            is_expr_next,
        };
        skip.push(frame.kind, frame.context);
        skip
    }

    fn innermost_kind(&self) -> Option<FrameKind> {
        self.frames.last().map(|frame| frame.kind)
    }

    fn innermost_block(&self) -> Option<(usize, FrameKind)> {
        let index = *self.blocks.last()?;
        Some((index, self.frames.get(index)?.kind))
    }

    fn push(&mut self, kind: FrameKind, context: Context) {
        for closer in kind.closers().into_iter().flatten() {
            let count = self.open.count_mut(closer);
            *count = count.saturating_add(1);
        }
        if !kind.is_bracket() {
            self.blocks.push(self.frames.len());
        }
        self.frames.push(Frame { kind, context });
    }

    /// Close every frame from `index` up, and tell the frame left innermost
    /// what closed.
    fn close_from(&mut self, index: usize) {
        let closed = self.frames.get(index).map(|frame| frame.kind);
        for frame in self.frames.drain(index..) {
            for closer in frame.kind.closers().into_iter().flatten() {
                let count = self.open.count_mut(closer);
                *count = count.saturating_sub(1);
            }
        }
        while self.blocks.last().is_some_and(|&block| block >= index) {
            self.blocks.pop();
        }
        let Some(outer) = self.frames.last_mut() else {
            return;
        };
        outer.context = match (outer.context, closed) {
            (Context::Type { is_in_bindings, .. }, Some(FrameKind::Bracket(_))) => {
                Context::Type { is_primary_next: false, is_in_bindings }
            }
            (Context::FunctionName, Some(FrameKind::Params)) => Context::AfterParams,
            (context, _) => context,
        };
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl Parser<'_> {
    /// Skip the construct the current token opens through the token that
    /// closes it: a bracket, an interpolated string, a type's `<`, `function`
    /// and its body, or a statement that holds a block. `repeat` stops after
    /// its `until`, before the condition.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token opens a bracket")]
    pub(super) fn skip_nested(&mut self) {
        let frame = match self.peek() {
            TokenKind::Less => Frame::new(FrameKind::Angle),
            TokenKind::Do => Frame::new(FrameKind::End),
            TokenKind::Function => Frame { kind: FrameKind::End, context: Context::FunctionName },
            TokenKind::If => Frame::new(FrameKind::If),
            TokenKind::While => Frame::new(FrameKind::LoopHeader),
            TokenKind::For => Frame {
                kind: FrameKind::LoopHeader,
                context: Context::Bindings { is_name_next: true },
            },
            TokenKind::Repeat => Frame::new(FrameKind::Until),
            TokenKind::InterpolatedStringBegin(_) => Frame::new(FrameKind::InterpolatedString),
            TokenKind::LeftBracket => Frame::new(FrameKind::Bracket(Closer::Bracket)),
            TokenKind::LeftBrace => Frame::new(FrameKind::Bracket(Closer::Brace)),
            _ => Frame::new(FrameKind::Bracket(Closer::Paren)),
        };
        let opener = self.advance();
        let mut skip = Skip::new(frame, precedes_expression(&opener.kind));
        while !skip.frames.is_empty() && !self.is_at_eof() {
            self.skip_token(&mut skip);
        }
    }

    fn skip_token(&mut self, skip: &mut Skip) {
        if let Some(closer) = Closer::of(self.peek()) {
            return self.skip_closer(skip, closer);
        }
        let Some(frame) = skip.frames.last().copied() else {
            return;
        };
        if frame.kind == FrameKind::Angle {
            return self.skip_in_angle(skip);
        }
        if frame.kind.holds_statements()
            && self.version.has_type_syntax()
            && self.skip_in_context(skip, frame.context)
        {
            skip.is_expr_next = false;
            return;
        }
        self.skip_plain_token(skip);
    }

    /// Close the innermost frame `closer` closes, as parsing would.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token closes its frame")]
    fn skip_closer(&mut self, skip: &mut Skip, closer: Closer) {
        let found = skip
            .open
            .expects(closer)
            .then(|| skip.frames.iter().rposition(|frame| frame.kind.closes_at(closer)))
            .flatten();
        let Some(index) = found else {
            return self.skip_unclosable(skip, closer);
        };
        let closed_kind = skip.frames.get(index).map(|frame| frame.kind);
        let (is_kept, is_expr_next) = match self.peek() {
            TokenKind::InterpolatedStringMiddle(_) | TokenKind::ElseIf => (true, true),
            TokenKind::Else => {
                let is_if_expr = closed_kind == Some(FrameKind::IfExpr);
                (!is_if_expr, is_if_expr)
            }
            TokenKind::Until => (false, true),
            _ => (false, false),
        };
        if is_kept {
            skip.close_from(index.saturating_add(1));
            if let Some(frame) = skip.frames.last_mut() {
                frame.context = Context::Expression;
            }
        } else {
            skip.close_from(index);
        }
        skip.is_expr_next = is_expr_next;
        self.advance();
    }

    /// A closer nothing open in the skip closes ends the skip before it when
    /// the parser has something open it closes, and is otherwise stray.
    #[cold]
    #[inline(never)]
    fn skip_unclosable(&mut self, skip: &mut Skip, closer: Closer) {
        if self.open.expects(closer) {
            skip.frames.clear();
            return;
        }
        if matches!(closer, Closer::Paren | Closer::Bracket | Closer::Brace)
            && skip.innermost_kind().is_some_and(FrameKind::is_bracket)
        {
            skip.close_from(skip.frames.len().saturating_sub(1));
        }
        skip.is_expr_next = false;
        self.advance();
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token is inside the type")]
    fn skip_in_angle(&mut self, skip: &mut Skip) {
        match self.peek() {
            TokenKind::Less => skip.push(FrameKind::Angle, Context::Expression),
            TokenKind::Greater => skip.close_from(skip.frames.len().saturating_sub(1)),
            // The lexer reads `>>` and `>=` greedily. A `>` this skip doesn't
            // close stays for the parser.
            TokenKind::ShiftRight
                if skip.frames.iter().rev().take(2).all(|frame| frame.kind == FrameKind::Angle)
                    && skip.frames.len() >= 2 =>
            {
                skip.close_from(skip.frames.len().saturating_sub(2));
            }
            TokenKind::ShiftRight => {
                skip.close_from(skip.frames.len().saturating_sub(1));
                self.split_close_angle(TokenKind::Greater);
                skip.is_expr_next = false;
                return;
            }
            TokenKind::GreaterEqual => {
                skip.close_from(skip.frames.len().saturating_sub(1));
                self.split_close_angle(TokenKind::Equal);
                skip.is_expr_next = false;
                return;
            }
            _ => return self.skip_plain_token(skip),
        }
        skip.is_expr_next = false;
        self.advance();
    }

    /// Follow just enough Luau grammar where statements go to tell a type's
    /// `<` and `>` from comparison operators. True when this took the token.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token leaves the context")]
    fn skip_in_context(&mut self, skip: &mut Skip, context: Context) -> bool {
        let index = skip.frames.len().saturating_sub(1);
        let kind = self.peek();
        let next = match (context, kind) {
            (Context::Expression, TokenKind::DoubleColon)
            | (Context::TypeAliasName, TokenKind::Equal) => {
                Context::Type { is_primary_next: true, is_in_bindings: false }
            }
            (Context::Expression, TokenKind::Local) => Context::Bindings { is_name_next: true },
            (Context::Expression, TokenKind::Identifier(name))
                if name == "const" && matches!(self.peek_next(), TokenKind::Identifier(_)) =>
            {
                Context::Bindings { is_name_next: true }
            }
            (Context::Expression, TokenKind::Identifier(name))
                if name == "type" && matches!(self.peek_next(), TokenKind::Identifier(_)) =>
            {
                Context::TypeAliasName
            }
            // `f<<T>>`
            (Context::Expression, TokenKind::Less)
                if matches!(self.peek_next(), TokenKind::Less) =>
            {
                skip.push(FrameKind::Angle, Context::Expression);
                Context::Expression
            }
            (Context::Bindings { is_name_next: true }, TokenKind::Identifier(_)) => {
                Context::Bindings { is_name_next: false }
            }
            (Context::Bindings { is_name_next: false }, TokenKind::Comma) => {
                Context::Bindings { is_name_next: true }
            }
            (
                Context::Bindings { is_name_next: false } | Context::AfterParams,
                TokenKind::Colon,
            ) => Context::Type {
                is_primary_next: true,
                is_in_bindings: matches!(context, Context::Bindings { .. }),
            },
            (
                Context::FunctionName,
                TokenKind::Identifier(_) | TokenKind::Dot | TokenKind::Colon,
            )
            | (Context::TypeAliasName, TokenKind::Identifier(_)) => context,
            (Context::FunctionName | Context::TypeAliasName, TokenKind::Less) => {
                skip.push(FrameKind::Angle, Context::Expression);
                context
            }
            (Context::FunctionName, TokenKind::LeftParen) => {
                skip.push(FrameKind::Params, Context::Expression);
                context
            }
            (Context::Type { is_primary_next: true, is_in_bindings }, _) => {
                return self.skip_type_primary_token(skip, is_in_bindings);
            }
            (Context::Type { is_primary_next: false, is_in_bindings }, _) => {
                let is_primary_next = match kind {
                    TokenKind::Question | TokenKind::DotDotDot => false,
                    TokenKind::Pipe | TokenKind::Ampersand | TokenKind::Arrow | TokenKind::Dot => {
                        true
                    }
                    TokenKind::Less => {
                        skip.push(FrameKind::Angle, Context::Expression);
                        false
                    }
                    _ => return self.leave_context(skip, Context::after_type(is_in_bindings)),
                };
                Context::Type { is_primary_next, is_in_bindings }
            }
            (
                Context::Expression
                | Context::Bindings { .. }
                | Context::FunctionName
                | Context::AfterParams
                | Context::TypeAliasName,
                _,
            ) => {
                return context != Context::Expression
                    && self.leave_context(skip, Context::Expression);
            }
        };
        if let Some(frame) = skip.frames.get_mut(index) {
            frame.context = next;
        }
        self.advance();
        true
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token starts no type")]
    fn skip_type_primary_token(&mut self, skip: &mut Skip, is_in_bindings: bool) -> bool {
        let index = skip.frames.len().saturating_sub(1);
        let is_primary_next = match self.peek() {
            TokenKind::Identifier(name) => {
                name == "typeof" && matches!(self.peek_next(), TokenKind::LeftParen)
            }
            TokenKind::Nil
            | TokenKind::True
            | TokenKind::False
            | TokenKind::StringLiteral(_)
            | TokenKind::Number(_)
            | TokenKind::Integer(_) => false,
            TokenKind::DotDotDot | TokenKind::Pipe | TokenKind::Ampersand => true,
            TokenKind::LeftParen | TokenKind::LeftBrace => {
                let closer = if matches!(self.peek(), TokenKind::LeftParen) {
                    Closer::Paren
                } else {
                    Closer::Brace
                };
                skip.push(FrameKind::Bracket(closer), Context::Expression);
                true
            }
            TokenKind::Less => {
                skip.push(FrameKind::Angle, Context::Expression);
                true
            }
            _ => return self.leave_context(skip, Context::after_type(is_in_bindings)),
        };
        if let Some(frame) = skip.frames.get_mut(index) {
            frame.context = Context::Type { is_primary_next, is_in_bindings };
        }
        self.advance();
        true
    }

    /// The current token ends the context it's in, and goes on in `next`,
    /// right after a type or a name, which ends an operand.
    fn leave_context(&mut self, skip: &mut Skip, next: Context) -> bool {
        if let Some(frame) = skip.frames.last_mut() {
            frame.context = next;
        }
        skip.is_expr_next = false;
        let Some(frame) = skip.frames.last().copied() else {
            return false;
        };
        self.skip_in_context(skip, frame.context)
    }

    /// Track what opens and closes constructs, and whether an `if` here
    /// would start an expression.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token is inside the construct")]
    fn skip_plain_token(&mut self, skip: &mut Skip) {
        let innermost = skip.innermost_kind();
        let kind = self.peek();
        let is_expr_next = match kind {
            TokenKind::LeftParen => {
                skip.push(FrameKind::Bracket(Closer::Paren), Context::Expression);
                true
            }
            TokenKind::LeftBracket => {
                skip.push(FrameKind::Bracket(Closer::Bracket), Context::Expression);
                true
            }
            TokenKind::LeftBrace => {
                skip.push(FrameKind::Bracket(Closer::Brace), Context::Expression);
                true
            }
            TokenKind::InterpolatedStringBegin(_) => {
                skip.push(FrameKind::InterpolatedString, Context::Expression);
                true
            }
            TokenKind::Function => {
                skip.push(FrameKind::End, Context::FunctionName);
                false
            }
            // A loop's `do` closes what its header left open.
            TokenKind::Do => {
                if let Some((index, FrameKind::LoopHeader)) = skip.innermost_block() {
                    skip.close_from(index);
                }
                skip.push(FrameKind::End, Context::Expression);
                false
            }
            TokenKind::While => {
                skip.push(FrameKind::LoopHeader, Context::Expression);
                true
            }
            TokenKind::For => {
                skip.push(FrameKind::LoopHeader, Context::Bindings { is_name_next: true });
                true
            }
            TokenKind::Repeat => {
                skip.push(FrameKind::Until, Context::Expression);
                false
            }
            TokenKind::If => {
                let is_expr = self.version.has_if_exprs()
                    && (skip.is_expr_next || !innermost.is_some_and(FrameKind::holds_statements));
                skip.push(
                    if is_expr { FrameKind::IfExpr } else { FrameKind::If },
                    Context::Expression,
                );
                true
            }
            TokenKind::Then => innermost == Some(FrameKind::IfExpr),
            _ => precedes_expression(kind),
        };
        skip.is_expr_next = is_expr_next;
        self.advance();
    }

    /// Skip an expression from its first token, with the tail `tail` allows.
    pub(super) fn skip_expr(&mut self, tail: ExprTail) {
        self.skip_expr_from(tail, false);
    }

    /// Skip the rest of an expression whose first operand the parser has read.
    pub(super) fn skip_expr_tail(&mut self, tail: ExprTail) {
        self.skip_expr_from(tail, true);
    }

    /// A Luau if-expression's condition and branches are whole expressions,
    /// so its parts take any operator, however tightly `tail` binds.
    fn skip_expr_from(&mut self, tail: ExprTail, is_after_operand: bool) {
        let mut parts = Vec::new();
        let mut is_at_operand = !is_after_operand;
        loop {
            if is_at_operand {
                while UnOp::from_token_kind(self.peek()).is_some() {
                    self.advance();
                }
                // Luau
                if self.version.has_if_exprs() && self.eat(&TokenKind::If) {
                    parts.push(IfPart::Condition);
                    continue;
                }
                if !self.skip_operand() {
                    return;
                }
                is_at_operand = false;
            }
            let min_precedence = match (parts.last(), tail) {
                (Some(_), ExprTail::Nothing | ExprTail::Operators(_)) => 0,
                (None, ExprTail::Operators(min_precedence)) => min_precedence,
                (None, ExprTail::Nothing) => return,
            };
            if self.version.has_type_syntax() && self.eat(&TokenKind::DoubleColon) {
                self.skip_type(TypeExtent::Full);
            }
            if self.peek_binary_op().is_some_and(|op| op.precedence().0 >= min_precedence) {
                self.advance();
                is_at_operand = true;
                continue;
            }
            match (parts.last_mut(), self.peek()) {
                (Some(part @ IfPart::Condition), TokenKind::Then) => *part = IfPart::Branch,
                (Some(part @ IfPart::Branch), TokenKind::ElseIf) => *part = IfPart::Condition,
                (Some(part @ IfPart::Branch), TokenKind::Else) => *part = IfPart::Else,
                (Some(IfPart::Else), _) => {
                    parts.pop();
                    continue;
                }
                (Some(IfPart::Condition | IfPart::Branch) | None, _) => return,
            }
            self.advance();
            is_at_operand = true;
        }
    }

    /// Skip a simple expression other than an if-expression. False when none
    /// starts here.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token starts no operand")]
    fn skip_operand(&mut self) -> bool {
        match self.peek() {
            TokenKind::Identifier(_) => {
                self.advance();
                self.skip_suffixes();
            }
            TokenKind::LeftParen => {
                self.skip_nested();
                self.skip_suffixes();
            }
            TokenKind::Nil
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Number(_)
            | TokenKind::Integer(_)
            | TokenKind::StringLiteral(_)
            | TokenKind::DotDotDot => {
                self.advance();
            }
            TokenKind::LeftBrace | TokenKind::InterpolatedStringBegin(_) | TokenKind::Function => {
                self.skip_nested();
            }
            // Luau
            TokenKind::At if self.version.has_function_attributes() => {
                self.skip_attributes();
                if !matches!(self.peek(), TokenKind::Function) {
                    return false;
                }
                self.skip_nested();
            }
            _ => return false,
        }
        true
    }

    /// Skip the run of suffixes at the current token.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other suffix is a string argument")]
    pub(super) fn skip_suffixes(&mut self) {
        while self.is_at_suffix() {
            match self.peek() {
                // Luau: `<<T>>`
                TokenKind::Less => {
                    self.advance();
                    self.skip_nested();
                    self.eat(&TokenKind::Greater);
                }
                TokenKind::Dot | TokenKind::Colon => {
                    self.advance();
                    if self.is_at_identifier() {
                        self.advance();
                    }
                }
                TokenKind::LeftBracket | TokenKind::LeftParen | TokenKind::LeftBrace => {
                    self.skip_nested();
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    /// Skip Luau's `@name` and `@[...]` function attributes.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token names no attribute")]
    pub(super) fn skip_attributes(&mut self) {
        while self.eat(&TokenKind::At) {
            match self.peek() {
                TokenKind::LeftBracket => self.skip_nested(),
                TokenKind::Identifier(_) => {
                    self.advance();
                }
                _ => {}
            }
        }
    }

    /// Skip a Luau type from its first token, as much of it as `extent` takes.
    pub(super) fn skip_type(&mut self, extent: TypeExtent) {
        if extent == TypeExtent::Full
            && matches!(self.peek(), TokenKind::Pipe | TokenKind::Ampersand)
        {
            self.advance();
        }
        self.skip_type_from(extent, false);
    }

    /// Skip the rest of a type whose first member the parser has read.
    pub(super) fn skip_type_tail(&mut self, extent: TypeExtent) {
        self.skip_type_from(extent, true);
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token ends the type")]
    fn skip_type_from(&mut self, mut extent: TypeExtent, is_after_primary: bool) {
        let mut is_at_primary = !is_after_primary;
        loop {
            if is_at_primary {
                if self.skip_primary_type() {
                    extent = TypeExtent::Full;
                    continue;
                }
                if extent == TypeExtent::Primary {
                    return;
                }
            }
            while self.eat(&TokenKind::Question) {}
            let is_member_next = match self.peek() {
                TokenKind::Ampersand => extent != TypeExtent::IntersectionMember,
                TokenKind::Pipe => extent == TypeExtent::Full,
                _ => false,
            };
            if !is_member_next {
                return;
            }
            self.advance();
            is_at_primary = true;
        }
    }

    /// Skip one type up to any `?`, `&`, or `|` after it. True when what it
    /// skipped leaves a whole type to come: a function type's return type or
    /// a variadic's type.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token starts no type")]
    fn skip_primary_type(&mut self) -> bool {
        match self.peek() {
            TokenKind::Identifier(name)
                if name == "typeof" && !matches!(self.peek_next(), TokenKind::Dot) =>
            {
                self.advance();
                if matches!(self.peek(), TokenKind::LeftParen) {
                    self.skip_nested();
                }
            }
            TokenKind::Identifier(_) => {
                self.advance();
                if self.eat(&TokenKind::DotDotDot) {
                    return false;
                }
                if self.eat(&TokenKind::Dot) && self.is_at_identifier() {
                    self.advance();
                }
                if matches!(self.peek(), TokenKind::Less) {
                    self.skip_nested();
                }
            }
            TokenKind::Nil
            | TokenKind::True
            | TokenKind::False
            | TokenKind::StringLiteral(_)
            | TokenKind::Number(_)
            | TokenKind::Integer(_) => {
                self.advance();
            }
            TokenKind::LeftBrace => self.skip_nested(),
            TokenKind::LeftParen | TokenKind::Less => {
                if matches!(self.peek(), TokenKind::Less) {
                    self.skip_nested();
                }
                if matches!(self.peek(), TokenKind::LeftParen) {
                    self.skip_nested();
                }
                return self.eat(&TokenKind::Arrow);
            }
            TokenKind::DotDotDot => {
                self.advance();
                return true;
            }
            _ => {}
        }
        false
    }
}
