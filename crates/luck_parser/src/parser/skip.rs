//! A construct the depth cap refuses is skipped by its tokens, not parsed.
//!
//! The skips loop over a heap stack, so they take no more host stack however
//! deep the construct nests. Each stops where parsing the construct would
//! have stopped, so the parser goes on from there as if it had.

use luck_token::{TokenKind, UnOp};

use super::Parser;

/// An open construct whose closing token the skip is looking for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// A bracket or an interpolated string. Everything inside is balanced, so
    /// only brackets count there.
    Group,
    /// A type's `<`, closed by `>`.
    Angle,
    /// `do`, `function`, a statement `if`, or a loop past its `do`, closed by
    /// `end`.
    End,
    /// `while` or `for` before its `do`.
    LoopHeader,
    /// `repeat`, closed by `until`.
    Until,
    /// A Luau if-expression, which has no closing token and ends at its
    /// `else`.
    IfExpr,
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
/// statement, outside an if-expression's `then` or `else`.
///
/// A Luau type ends in `>` more often than an expression continues with one,
/// so `>` reads as ending an operand, and `a > if b then c else d` inside a
/// skipped block throws its `end`s off by one.
const fn precedes_expression(kind: &TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Identifier(_)
            | TokenKind::Number(_)
            | TokenKind::StringLiteral(_)
            | TokenKind::Nil
            | TokenKind::True
            | TokenKind::False
            | TokenKind::DotDotDot
            | TokenKind::RightParen
            | TokenKind::RightBracket
            | TokenKind::RightBrace
            | TokenKind::InterpolatedStringEnd(_)
            | TokenKind::Greater
            | TokenKind::Question
            | TokenKind::End
            | TokenKind::Do
            | TokenKind::Repeat
            | TokenKind::Break
            | TokenKind::Semicolon
    )
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
            TokenKind::Less => Frame::Angle,
            TokenKind::Do | TokenKind::Function | TokenKind::If => Frame::End,
            TokenKind::While | TokenKind::For => Frame::LoopHeader,
            TokenKind::Repeat => Frame::Until,
            _ => Frame::Group,
        };
        let opener = self.advance();
        let mut frames = vec![frame];
        let mut is_expr_next = precedes_expression(&opener.kind);
        while let Some(&innermost) = frames.last() {
            match innermost {
                Frame::Group => self.skip_in_group(&mut frames, &mut is_expr_next),
                Frame::Angle => self.skip_in_angle(&mut frames),
                Frame::End | Frame::LoopHeader | Frame::Until | Frame::IfExpr => {
                    self.skip_in_block(&mut frames, &mut is_expr_next);
                }
            }
            if self.is_at_eof() {
                return;
            }
        }
    }

    /// Inside a bracket, keywords are balanced, so only brackets open or
    /// close anything.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token is inside the group")]
    fn skip_in_group(&mut self, frames: &mut Vec<Frame>, is_expr_next: &mut bool) {
        match self.peek() {
            TokenKind::LeftParen
            | TokenKind::LeftBracket
            | TokenKind::LeftBrace
            | TokenKind::InterpolatedStringBegin(_) => frames.push(Frame::Group),
            TokenKind::RightParen
            | TokenKind::RightBracket
            | TokenKind::RightBrace
            | TokenKind::InterpolatedStringEnd(_) => {
                frames.pop();
                *is_expr_next = false;
            }
            _ => {}
        }
        self.advance();
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token is inside the type")]
    fn skip_in_angle(&mut self, frames: &mut Vec<Frame>) {
        match self.peek() {
            TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => {
                frames.push(Frame::Group);
            }
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace => {
                return self.close_stray_bracket(frames);
            }
            TokenKind::Less => frames.push(Frame::Angle),
            TokenKind::Greater => {
                frames.pop();
            }
            // The lexer reads `>>` and `>=` greedily. A `>` this skip doesn't
            // close stays for the parser.
            TokenKind::ShiftRight if frames.len() >= 2 => {
                frames.truncate(frames.len().saturating_sub(2));
            }
            TokenKind::ShiftRight => {
                frames.pop();
                self.split_close_angle(TokenKind::Greater);
                return;
            }
            TokenKind::GreaterEqual => {
                frames.pop();
                self.split_close_angle(TokenKind::Equal);
                return;
            }
            _ => {}
        }
        self.advance();
    }

    /// Outside brackets, a block keyword opens or closes a construct, and an
    /// `if` is a statement unless it follows a token that leaves an
    /// expression to come.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token is inside the block")]
    fn skip_in_block(&mut self, frames: &mut Vec<Frame>, is_expr_next: &mut bool) {
        let kind = self.peek();
        let is_in_if_expr = frames.last() == Some(&Frame::IfExpr);
        let next_is_expr = match kind {
            TokenKind::LeftParen
            | TokenKind::LeftBracket
            | TokenKind::LeftBrace
            | TokenKind::InterpolatedStringBegin(_) => {
                frames.push(Frame::Group);
                true
            }
            TokenKind::RightParen
            | TokenKind::RightBracket
            | TokenKind::RightBrace
            | TokenKind::InterpolatedStringMiddle(_)
            | TokenKind::InterpolatedStringEnd(_) => return self.close_stray_bracket(frames),
            TokenKind::Do => {
                if let Some(frame @ Frame::LoopHeader) = frames.last_mut() {
                    *frame = Frame::End;
                } else {
                    frames.push(Frame::End);
                }
                false
            }
            TokenKind::Function => {
                frames.push(Frame::End);
                true
            }
            TokenKind::While | TokenKind::For => {
                frames.push(Frame::LoopHeader);
                true
            }
            TokenKind::Repeat => {
                frames.push(Frame::Until);
                false
            }
            TokenKind::If => {
                let is_expr = *is_expr_next && self.version.has_if_exprs();
                frames.push(if is_expr { Frame::IfExpr } else { Frame::End });
                true
            }
            // A Luau if-expression has no closer, so one still open here was
            // missing its `else`.
            TokenKind::End | TokenKind::Until => {
                while frames.last() == Some(&Frame::IfExpr) {
                    frames.pop();
                }
                frames.pop();
                matches!(kind, TokenKind::Until)
            }
            TokenKind::Then => is_in_if_expr,
            TokenKind::Else => {
                if is_in_if_expr {
                    frames.pop();
                }
                is_in_if_expr
            }
            _ => precedes_expression(kind),
        };
        *is_expr_next = next_is_expr;
        self.advance();
    }

    /// A closing bracket at block level closes the bracket open around the
    /// unclosed constructs inside it, or, with none open, ends the skip
    /// before it.
    fn close_stray_bracket(&mut self, frames: &mut Vec<Frame>) {
        let Some(group) = frames.iter().rposition(|frame| *frame == Frame::Group) else {
            frames.clear();
            return;
        };
        frames.truncate(group);
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
            | TokenKind::Number(_) => {
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
