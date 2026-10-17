//! Glob matcher matching the semantics of the reference patch interpreter.
//!
//! `*` matches zero or more characters (including spaces), `?` matches exactly
//! one character, and the whole string must match (anchored). As in the
//! original, an empty pattern or an empty subject never matches.

fn eq(a: u8, b: u8, icase: bool) -> bool {
    if icase {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

fn matches(data: &str, mask: &str, icase: bool) -> bool {
    if data.is_empty() || mask.is_empty() {
        return false;
    }

    let d = data.as_bytes();
    let m = mask.as_bytes();

    // Iterative glob match with backtracking to the last `*`.
    let mut i = 0usize;
    let mut j = 0usize;
    let mut star: Option<usize> = None;
    let mut mark = 0usize;

    while i < d.len() {
        if j < m.len() && (m[j] == b'?' || eq(m[j], d[i], icase)) {
            i += 1;
            j += 1;
        } else if j < m.len() && m[j] == b'*' {
            star = Some(j);
            mark = i;
            j += 1;
        } else if let Some(s) = star {
            j = s + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }

    while j < m.len() && m[j] == b'*' {
        j += 1;
    }

    j == m.len()
}

/// Case-sensitive match.
pub fn wildcard_match(data: &str, mask: &str) -> bool {
    matches(data, mask, false)
}

/// Case-insensitive match.
pub fn wildcard_match_icase(data: &str, mask: &str) -> bool {
    matches(data, mask, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        assert!(wildcard_match("write at 0x10:00", "write *:*"));
        assert!(wildcard_match_icase("SET POINTER:0x10", "set pointer:*"));
        assert!(!wildcard_match("", "*"));
        assert!(!wildcard_match("x", ""));
        assert!(wildcard_match("crc32", "crc32*"));
        assert!(!wildcard_match("crc-32", "crc32*"));
        assert!(wildcard_match("crc-32", "crc*"));
        assert!(wildcard_match("left(0x1,2)", "left(*,*)*"));
        assert!(!wildcard_match("wadd(0x1,2)", "add(*,*)*"));
        assert!(wildcard_match("a b c", "a*c"));
        assert!(wildcard_match("abc", "a?c"));
    }
}
