use crate::util::str::longest_utf8;

/// Drop CSI, OSC, and other ESC sequences. The remaining text is what
/// the diff grammar is matched against.
pub(crate) fn strip_esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match rest.split_once('\x1b') {
            None => {
                out.push_str(rest);
                break;
            }
            Some((text, after_esc)) => {
                out.push_str(text);
                rest = skip_esc_body(after_esc);
            }
        }
    }
    out
}

/// `after_esc` is the text immediately following an ESC. Returns the
/// suffix that follows the completed sequence.
fn skip_esc_body(after_esc: &str) -> &str {
    match after_esc.as_bytes().split_first() {
        None => "",
        Some((&b'[', rest)) => skip_csi(longest_utf8(rest)),
        Some((&b']', rest)) => skip_osc(longest_utf8(rest)),
        Some(_) => skip_one_byte(after_esc),
    }
}

fn skip_csi(body: &str) -> &str {
    for (i, b) in body.bytes().enumerate() {
        if (0x40..=0x7e).contains(&b) {
            return match body.get(i + 1..) {
                Some(rest) => rest,
                None => "",
            };
        }
    }
    ""
}

fn skip_osc(body: &str) -> &str {
    // BEL, or ST (`ESC \`). An ESC that is not ST stays inside the OSC.
    let bel = body.find('\x07');
    let st = body.find("\x1b\\");
    match (bel, st) {
        (Some(bel_at), Some(st_at)) => {
            if bel_at < st_at {
                return body.get(bel_at + 1..).unwrap_or("");
            }
            body.get(st_at + 2..).unwrap_or("")
        }
        (Some(bel_at), None) => body.get(bel_at + 1..).unwrap_or(""),
        (None, Some(st_at)) => body.get(st_at + 2..).unwrap_or(""),
        (None, None) => "",
    }
}

/// Two-byte ESC: the final is one byte. Resume at the next char boundary.
fn skip_one_byte(after_esc: &str) -> &str {
    let mut i = 1;
    while i < after_esc.len() {
        if after_esc.is_char_boundary(i) {
            return &after_esc[i..];
        }
        i += 1;
    }
    ""
}

#[cfg(test)]
mod tests {
    use crate::terminal::esc::strip_esc;

    #[test]
    fn test_strip_esc_leaves_plain_text() {
        assert_eq!("hello", strip_esc("hello"));
        assert_eq!("café", strip_esc("café"));
        assert_eq!("", strip_esc(""));
    }

    #[test]
    fn test_strip_esc_drops_csi() {
        assert_eq!("red", strip_esc("\x1b[31mred\x1b[0m"));
        assert_eq!("ab", strip_esc("a\x1b[2Kb"));
        // Unterminated CSI consumes the rest.
        assert_eq!("a", strip_esc("a\x1b[31"));
    }

    #[test]
    fn test_strip_esc_drops_osc() {
        assert_eq!("x", strip_esc("\x1b]0;title\x07x"));
        assert_eq!("y", strip_esc("\x1b]8;;https://example.com\x1b\\y"));
    }

    #[test]
    fn test_strip_esc_drops_two_byte_esc() {
        assert_eq!("z", strip_esc("\x1bMz"));
        // A lone ESC at end of input is dropped.
        assert_eq!("a", strip_esc("a\x1b"));
        // The second ESC is the final byte of the first sequence.
        assert_eq!("Mx", strip_esc("\x1b\x1bMx"));
    }

    #[test]
    fn test_strip_esc_osc_keeps_inner_esc_until_terminator() {
        assert_eq!("z", strip_esc("\x1b]ab\x1bXc\x07z"));
    }
}
