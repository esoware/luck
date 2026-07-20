use luck_token::CompactString;

const FIRST_CHARS: &[u8] = b"luckLUCKabdefghijmnopqrstvwxyzABDEFGHIJMNOPQRSTVWXYZ_";
const REST_CHARS: &[u8] = b"luckLUCKabdefghijmnopqrstvwxyz_ABDEFGHIJMNOPQRSTVWXYZ0123456789";

/// The `idx`-th candidate identifier, shortest first, by mixed-radix
/// encoding: the first character draws from `FIRST_CHARS` (base 53) and
/// each subsequent character from `REST_CHARS` (base 63, digits included).
/// Indices 0..52 are one char, the next 53*63 are two chars, then
/// 53*63^2 three, and so on.
///
/// The sequence is dense and includes keyword spellings; skipping keywords
/// and names already in use is the caller's job (see `CandidatePool`).
pub fn name_for_index(idx: usize) -> CompactString {
    let first_base = FIRST_CHARS.len();
    let rest_base = REST_CHARS.len();

    if idx < first_base {
        let mut name = CompactString::with_capacity(1);
        name.push(FIRST_CHARS[idx] as char);
        return name;
    }

    let mut remaining = idx - first_base;
    let mut length = 2u32;
    loop {
        let combinations = first_base * rest_base.pow(length - 1);
        if remaining < combinations {
            break;
        }
        remaining -= combinations;
        length += 1;
    }

    let mut name = CompactString::with_capacity(length as usize);
    let lead_divisor = rest_base.pow(length - 1);
    name.push(FIRST_CHARS[remaining / lead_divisor] as char);
    remaining %= lead_divisor;
    for place in (0..length - 1).rev() {
        let divisor = rest_base.pow(place);
        name.push(REST_CHARS[remaining / divisor] as char);
        remaining %= divisor;
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_chars_come_first() {
        assert_eq!(name_for_index(0), "l");
        assert_eq!(name_for_index(1), "u");
        assert_eq!(name_for_index(2), "c");
        assert_eq!(name_for_index(3), "k");
    }

    #[test]
    fn rolls_over_to_two_chars() {
        assert_eq!(name_for_index(FIRST_CHARS.len() - 1), "_");
        assert_eq!(name_for_index(FIRST_CHARS.len()), "ll");
    }

    #[test]
    fn is_injective_over_the_low_range() {
        let mut seen = std::collections::HashSet::new();
        for idx in 0..10_000 {
            assert!(seen.insert(name_for_index(idx)), "collision at {idx}");
        }
    }
}
