//! Input decoding and the arena shared by the fuzz targets.

mod input;

pub use input::{decode_input, with_arena};
