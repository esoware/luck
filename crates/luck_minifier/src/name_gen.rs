use luck_token::CompactString;
use rustc_hash::FxHashSet;

const FIRST_CHARS: &[u8] = b"luckLUCKabdefghijmnopqrstvwxyzABDEFGHIJMNOPQRSTVWXYZ_";
const REST_CHARS: &[u8] = b"luckLUCKabdefghijmnopqrstvwxyz_ABDEFGHIJMNOPQRSTVWXYZ0123456789";

/// Generates short, unique variable names for the rename pass.
pub struct NameGenerator {
    counter: usize,
    used_names: FxHashSet<CompactString>,
    keywords: FxHashSet<&'static str>,
}

#[allow(dead_code)]
impl NameGenerator {
    pub fn new(keywords: &[&'static str]) -> Self {
        Self {
            counter: 0,
            used_names: FxHashSet::default(),
            keywords: keywords.iter().copied().collect(),
        }
    }

    /// Generate the next shortest available name.
    pub fn generate(&mut self) -> CompactString {
        loop {
            let name = self.index_to_name(self.counter);
            self.counter += 1;
            if !self.keywords.contains(name.as_str()) && !self.used_names.contains(&name) {
                self.used_names.insert(name.clone());
                return name;
            }
        }
    }

    /// Converts a sequential index into a unique identifier using mixed-radix
    /// encoding. The first character draws from `FIRST_CHARS` (base 53), and
    /// each subsequent character draws from `REST_CHARS` (base 63, includes
    /// digits). Single-char names are indices 0..52, two-char names fill
    /// the next 53*63 slots, three-char the next 53*63^2, and so on.
    pub fn index_to_name(&self, idx: usize) -> CompactString {
        let first_base = FIRST_CHARS.len();
        let rest_base = REST_CHARS.len();

        if idx < first_base {
            let mut name = CompactString::with_capacity(1);
            name.push(FIRST_CHARS[idx] as char);
            return name;
        }

        let mut remaining = idx - first_base;
        let mut length = 2;

        loop {
            let combinations_at_length = first_base * rest_base.pow(length as u32 - 1);
            if remaining < combinations_at_length {
                break;
            }
            remaining -= combinations_at_length;
            length += 1;
        }

        let mut result = CompactString::with_capacity(length);
        let first_idx = remaining / rest_base.pow(length as u32 - 1);
        result.push(FIRST_CHARS[first_idx] as char);
        remaining %= rest_base.pow(length as u32 - 1);

        for i in (0..length - 1).rev() {
            let divisor = rest_base.pow(i as u32);
            let char_idx = remaining / divisor;
            result.push(REST_CHARS[char_idx] as char);
            remaining %= divisor;
        }

        result
    }

    pub fn mark_used(&mut self, name: &str) {
        self.used_names.insert(name.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generates_single_chars_first() {
        let mut ng = NameGenerator::new(&[]);
        assert_eq!(ng.generate(), "l");
        assert_eq!(ng.generate(), "u");
        assert_eq!(ng.generate(), "c");
        assert_eq!(ng.generate(), "k");
    }

    #[test]
    fn test_skips_keywords() {
        let mut ng = NameGenerator::new(&["l", "u"]);
        assert_eq!(ng.generate(), "c");
    }

    #[test]
    fn test_skips_used() {
        let mut ng = NameGenerator::new(&[]);
        ng.mark_used("l");
        assert_eq!(ng.generate(), "u");
    }
}
