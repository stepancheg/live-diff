/// Longest contiguous substring of `bytes` that is valid UTF-8.
/// On a length tie, the leftmost substring wins.
pub(crate) fn longest_utf8(bytes: &[u8]) -> &str {
    let mut best = "";
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid();
        if valid.len() > best.len() {
            best = valid;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use crate::util::str::longest_utf8;

    #[test]
    fn test_longest_utf8() {
        assert_eq!("ok", longest_utf8(b"ok"));
        assert_eq!("", longest_utf8(b""));
        // Truncated 2-byte sequence: C3 A9 is 'é'; C3 alone is invalid.
        assert_eq!("a", longest_utf8(b"a\xC3"));
        assert_eq!("aé", longest_utf8("aé".as_bytes()));
        // Longest run is not a prefix.
        assert_eq!("hello", longest_utf8(b"\xFFhi\xFFhello\xFFyo"));
        assert_eq!("abc", longest_utf8(b"\x80abc"));
    }
}
