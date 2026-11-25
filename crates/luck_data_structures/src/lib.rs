//! # `luck_data_structures`
//!
//! Data structures that luck's crates share and that know nothing of Luau,
//! as `oxc_data_structures` holds oxc's.
//!
//! - [`CodeBuffer`]: the output buffer the printers write into, which holds
//!   bytes, as a source is.
//! - [`BitSet`]: a fixed number of bits, one apiece.
//!
//! This crate has zero internal dependencies.
//!
//! # Usage
//!
//! ```
//! use luck_data_structures::CodeBuffer;
//!
//! let mut output = CodeBuffer::with_capacity(16);
//! output.print_bytes(b"local");
//! output.print_byte(b' ');
//! output.print_bytes(b"x");
//! assert_eq!(output.into_bytes(), b"local x");
//! ```

mod bit_set;
mod code_buffer;

pub use bit_set::BitSet;
pub use code_buffer::CodeBuffer;
