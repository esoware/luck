/// How many of a few kinds of node a tree holds.
///
/// The parser counts them as it builds the tree, so a consumer can size its
/// tables once. A tree built in code can pass the default, and tables sized
/// from it grow as needed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeCounts {
    /// Blocks, and if-expression branches a binding holds, each of which
    /// opens a scope.
    pub blocks: u32,
    /// Names a declaration binds: locals, parameters, loop variables, and a
    /// method's `self`.
    pub declarations: u32,
    /// Names an expression reads, and each function statement's first name.
    pub name_uses: u32,
    /// Names the parser read, each indexed by its place among them.
    pub names: u32,
}
