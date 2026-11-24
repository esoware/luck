//! PUC Lua's compile-time limits on what one function holds, counted as
//! `lparser.c` counts them while it parses: the locals active at once, and
//! the variables of enclosing functions it captures.
//!
//! A local counts from its declaration to the end of its block, with the
//! hidden locals a `for` loop or a Lua 5.5 vararg function declares, and in
//! Lua 5.5 only one that takes a register. Each function between a variable
//! and a use of it captures it, `_ENV` included in Lua 5.2+, where a global
//! is a field of it, and a compile-time constant is no upvalue, since its
//! uses read its value.

use luck_ast::{Expression, Name};
use luck_diagnostics::Diagnostic;
use luck_token::{BinOp, LuaVersion, MAX_ACTIVE_LOCALS, Span, UnOp};

use crate::checker::{too_many_locals, too_many_upvalues};
use crate::id::SymbolId;
use crate::scoping::Scoping;

/// What a function's limits count of one symbol.
#[derive(Clone, Copy)]
enum Tracked {
    /// Lua 5.4+: a compile-time constant, which no function captures.
    Constant,
    /// A variable, with the depth of the innermost open function that holds
    /// it, as a local or an upvalue. Every open function between there and
    /// the one that declares it holds it too.
    Variable(usize),
}

/// A variable a function captures.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Upvalue {
    Symbol(SymbolId),
    /// Lua 5.2+: the chunk's `_ENV`, which every global reads.
    Env,
}

struct Frame {
    /// The locals active in the function, as its compiler counts them.
    locals: usize,
    /// Each variable the function captures, once.
    upvalues: Vec<Upvalue>,
    /// What the function's diagnostics point at, `None` for the chunk.
    header: Option<Span>,
    has_too_many_locals: bool,
    has_too_many_upvalues: bool,
}

impl Frame {
    const fn new(header: Option<Span>, upvalues: Vec<Upvalue>) -> Self {
        Self {
            locals: 0,
            upvalues,
            header,
            has_too_many_locals: false,
            has_too_many_upvalues: false,
        }
    }
}

pub(super) struct Limits {
    version: LuaVersion,
    /// The open functions, the chunk's first.
    frames: Vec<Frame>,
    /// Each open block's function's active locals where the block began.
    scope_locals: Vec<usize>,
    /// By symbol index. `None` for a symbol no function counts, such as a
    /// type function's parameter.
    symbols: Vec<Option<Tracked>>,
    /// The depth of the innermost open function that holds `_ENV`, which the
    /// chunk does from the start.
    env_depth: usize,
    /// Closed functions' upvalue lists, emptied, which keep their room for
    /// the next function, so lists are made only as deep as functions nest.
    spare_upvalues: Vec<Vec<Upvalue>>,
}

impl Limits {
    /// The counts of a chunk in `version` with `declarations` symbols, `None`
    /// for a version whose compiler counts after it optimizes.
    pub(super) fn new(version: LuaVersion, declarations: usize) -> Option<Self> {
        version.has_parse_time_limits().then(|| Self {
            version,
            frames: vec![Frame::new(None, Vec::new())],
            scope_locals: Vec::new(),
            symbols: Vec::with_capacity(declarations),
            env_depth: 0,
            spare_upvalues: Vec::new(),
        })
    }

    pub(super) fn enter_function(&mut self, header: Span) {
        let upvalues = self.spare_upvalues.pop().unwrap_or_default();
        self.frames.push(Frame::new(Some(header), upvalues));
    }

    /// Each variable the function captured is held by its enclosing
    /// function, which captured it too.
    pub(super) fn leave_function(&mut self) {
        let Some(mut frame) = self.frames.pop() else {
            return;
        };
        let enclosing = self.frames.len().saturating_sub(1);
        for upvalue in frame.upvalues.drain(..) {
            match upvalue {
                Upvalue::Symbol(id) => {
                    if let Some(Some(Tracked::Variable(depth))) = self.symbols.get_mut(id.index()) {
                        *depth = enclosing;
                    }
                }
                Upvalue::Env => self.env_depth = enclosing,
            }
        }
        self.spare_upvalues.push(frame.upvalues);
    }

    pub(super) fn enter_scope(&mut self) {
        let locals = self.frames.last().map_or(0, |frame| frame.locals);
        self.scope_locals.push(locals);
    }

    pub(super) fn leave_scope(&mut self) {
        if let (Some(locals), Some(frame)) = (self.scope_locals.pop(), self.frames.last_mut()) {
            frame.locals = locals;
        }
    }

    /// Declares `symbol`, a local of the current function written at
    /// `span`. A compile-time constant takes no register in Lua 5.5, so it
    /// counts only before that.
    pub(super) fn local(
        &mut self,
        symbol: SymbolId,
        span: Span,
        is_constant: bool,
        errors: &mut Vec<Diagnostic>,
    ) {
        let depth = self.frames.len().saturating_sub(1);
        let index = symbol.index();
        if self.symbols.len() <= index {
            self.symbols.resize(index.saturating_add(1), None);
        }
        if let Some(slot) = self.symbols.get_mut(index) {
            *slot = Some(if is_constant { Tracked::Constant } else { Tracked::Variable(depth) });
        }
        if !is_constant || !self.version.has_register_local_limit() {
            self.hidden(1, span, errors);
        }
    }

    /// `count` locals no name spells, which the construct at `span`
    /// declares, as a `for` loop's state.
    pub(super) fn hidden(&mut self, count: usize, span: Span, errors: &mut Vec<Diagnostic>) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        frame.locals = frame.locals.saturating_add(count);
        if frame.locals > MAX_ACTIVE_LOCALS && !frame.has_too_many_locals {
            frame.has_too_many_locals = true;
            errors.push(too_many_locals(span, MAX_ACTIVE_LOCALS, frame.header));
        }
    }

    /// A use at `span` in the current function of `upvalue`, which each
    /// function between the one that holds it and this one captures.
    ///
    /// The compiler captures it outermost first, and stops at the first
    /// function that has no room, so a use that takes two past the limit is
    /// one error, the outer function's.
    pub(super) fn capture(&mut self, upvalue: Upvalue, span: Span, errors: &mut Vec<Diagnostic>) {
        let held = match upvalue {
            Upvalue::Symbol(id) => match self.symbols.get(id.index()).copied().flatten() {
                Some(Tracked::Variable(depth)) => depth,
                Some(Tracked::Constant) | None => return,
            },
            Upvalue::Env => self.env_depth,
        };
        let current = self.frames.len().saturating_sub(1);
        if held >= current {
            return;
        }
        let limit = self.version.max_upvalues();
        let mut is_reported = false;
        for frame in self.frames.get_mut(held.saturating_add(1)..).unwrap_or_default() {
            frame.upvalues.push(upvalue);
            if frame.upvalues.len() > limit && !frame.has_too_many_upvalues {
                frame.has_too_many_upvalues = true;
                if !is_reported {
                    is_reported = true;
                    errors.push(too_many_upvalues(span, limit, frame.header));
                }
            }
        }
        match upvalue {
            Upvalue::Symbol(id) => {
                if let Some(slot) = self.symbols.get_mut(id.index()) {
                    *slot = Some(Tracked::Variable(current));
                }
            }
            Upvalue::Env => self.env_depth = current,
        }
    }

    /// Lua 5.4+: whether a `<const>` local `expr` initializes may be a
    /// compile-time constant, a value `luaK_exp2const` folds.
    ///
    /// That's a literal, a reference to another constant, or an operator
    /// the compiler folds over constants. It counts every arithmetic,
    /// bitwise, `and`, or `or` operator over constants, though the compiler
    /// leaves some unfolded, such as a division by zero, so a local this
    /// takes for a constant may not be one, and a limit it misses by one
    /// goes unreported, but none is reported that the compiler doesn't
    /// report.
    pub(super) fn is_constant(&self, expr: &Expression<'_>, scoping: &Scoping) -> bool {
        let is_constant_name = |name: &Name<'_>| {
            scoping
                .reference_of(name)
                .and_then(|reference| scoping.reference(reference).resolved)
                .and_then(|symbol| self.symbols.get(symbol.index()).copied().flatten())
                .is_some_and(|tracked| matches!(tracked, Tracked::Constant))
        };
        match expr {
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_) => true,
            Expression::Name(name) => is_constant_name(name),
            Expression::Parenthesized(parenthesized) => {
                self.is_constant(&parenthesized.expr, scoping)
            }
            Expression::UnaryOp(unary) => {
                unary.op != UnOp::Len && self.is_constant(&unary.operand, scoping)
            }
            Expression::BinaryOp(run) => {
                is_folded(run.links.first().op)
                    && self.is_constant(&run.first, scoping)
                    && run.links.iter().all(|link| self.is_constant(&link.operand, scoping))
            }
            Expression::Integer(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Suffixed(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => false,
        }
    }
}

/// Whether the compiler folds `op` over constants: an arithmetic or bitwise
/// operator, which `constfolding` computes, or `and` or `or`, which picks an
/// operand. A comparison or `..` always runs.
const fn is_folded(op: BinOp) -> bool {
    match op {
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr
        | BinOp::And
        | BinOp::Or => true,
        BinOp::Concat | BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            false
        }
    }
}
