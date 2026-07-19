//! Byte lookup tables for batched scanning, modeled on oxc's
//! `SafeByteMatchTable`. A table marks the bytes a scan must stop at;
//! `find_match` walks fixed-size batches so the inner loop vectorizes.

pub const SEARCH_BATCH_SIZE: usize = 32;

/// 256-entry stop-byte table, cache-line aligned. Build with
/// `byte_match_table!` at compile time.
#[repr(C, align(64))]
pub struct ByteMatchTable([bool; 256]);

impl ByteMatchTable {
    pub const fn new(table: [bool; 256]) -> Self {
        Self(table)
    }

    #[inline]
    pub const fn matches(&self, byte: u8) -> bool {
        self.0[byte as usize]
    }
}

/// `byte_match_table!(|byte| ...)` evaluates the predicate for every byte
/// value at compile time.
macro_rules! byte_match_table {
    (|$byte:ident| $test:expr) => {{
        const TABLE: [bool; 256] = {
            let mut table = [false; 256];
            let mut index = 0;
            while index < 256 {
                let $byte = index as u8;
                table[index] = $test;
                index += 1;
            }
            table
        };
        $crate::search::ByteMatchTable::new(TABLE)
    }};
}
pub(crate) use byte_match_table;

/// Offset of the first byte matching `table`, or `bytes.len()` if none
/// match. Most runs (identifiers, single spaces) are short, so the scan
/// starts scalar and only long runs pay for batches; a batch is tested
/// whole before locating the match so long runs go branch-light.
#[inline]
pub fn find_match(bytes: &[u8], table: &ByteMatchTable) -> usize {
    const SCALAR_PREFIX: usize = 8;
    let mut offset = 0;
    let prefix_end = bytes.len().min(SCALAR_PREFIX);
    while offset < prefix_end {
        if table.matches(bytes[offset]) {
            return offset;
        }
        offset += 1;
    }
    while offset + SEARCH_BATCH_SIZE <= bytes.len() {
        let mut any_match = false;
        for &byte in &bytes[offset..offset + SEARCH_BATCH_SIZE] {
            any_match |= table.matches(byte);
        }
        if any_match {
            break;
        }
        offset += SEARCH_BATCH_SIZE;
    }
    while offset < bytes.len() && !table.matches(bytes[offset]) {
        offset += 1;
    }
    offset
}
