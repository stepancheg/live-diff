use crate::terminal::line::Line;

/// File header inside a unified or git diff: the `diff ...` line through
/// the metadata lines that precede the first hunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileHeader {
    /// Index of the first header line.
    pub(crate) start: usize,
    /// Index one past the last header line. Hunks and other body lines
    /// start here.
    pub(crate) end: usize,
}

/// Locate each file header in diff output.
///
/// Understands git extended headers (`diff --git`, `diff --cc`,
/// `diff --combined`), Sapling/Mercurial headers (`diff -r`), and a
/// classic unified pair (`---` / `+++`) with no preceding `diff` line.
/// Lines before the first header, and lines that are not part of a
/// header, are left alone. The section for a file runs from its
/// header through the line before the next header.
pub(crate) fn file_headers(lines: &[Line]) -> Vec<FileHeader> {
    let mut headers = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let text = lines[i].plain_text();
        if is_diff_command_start(&text) || is_unified_pair_start(lines, i) {
            let start = i;
            i += 1;
            while i < lines.len() {
                let next = lines[i].plain_text();
                if is_diff_command_start(&next) || !is_header_meta(&next) {
                    break;
                }
                i += 1;
            }
            headers.push(FileHeader { start, end: i });
            continue;
        }
        i += 1;
    }
    headers
}

/// Index of the file header whose section contains `line`.
///
/// A section runs from `headers[i].start` to `headers[i + 1].start`
/// (or the end of `lines`). Returns `None` when `line` is before the
/// first header.
pub(crate) fn file_header_index(headers: &[FileHeader], line: usize) -> Option<usize> {
    let i = headers.partition_point(|header| header.start <= line);
    if i == 0 {
        return None;
    }
    Some(i - 1)
}

fn is_diff_command_start(text: &str) -> bool {
    text.starts_with("diff --git ")
        || text.starts_with("diff --cc ")
        || text.starts_with("diff --combined ")
        || text.starts_with("diff -r ")
}

fn is_unified_pair_start(lines: &[Line], index: usize) -> bool {
    let Some(next) = lines.get(index + 1) else {
        return false;
    };
    is_minus_file_line(&lines[index].plain_text()) && is_plus_file_line(&next.plain_text())
}

fn is_header_meta(text: &str) -> bool {
    is_minus_file_line(text)
        || is_plus_file_line(text)
        || text.starts_with("old mode ")
        || text.starts_with("new mode ")
        || text.starts_with("deleted file mode ")
        || text.starts_with("new file mode ")
        || text.starts_with("copy from ")
        || text.starts_with("copy to ")
        || text.starts_with("rename from ")
        || text.starts_with("rename to ")
        || text.starts_with("similarity index ")
        || text.starts_with("dissimilarity index ")
        || text.starts_with("index ")
}

fn is_minus_file_line(text: &str) -> bool {
    text.starts_with("--- ") || text.starts_with("---\t")
}

fn is_plus_file_line(text: &str) -> bool {
    text.starts_with("+++ ") || text.starts_with("+++\t")
}

#[cfg(test)]
mod tests {
    use crate::diff::FileHeader;
    use crate::diff::file_header_index;
    use crate::diff::file_headers;
    use crate::terminal::parse::parse_command_output;

    fn headers_of(input: &[u8]) -> Vec<FileHeader> {
        file_headers(&parse_command_output(input))
    }

    #[test]
    fn test_git_extended_headers() {
        let input = b"\
diff --git a/foo.rs b/foo.rs
index 1111111..2222222 100644
--- a/foo.rs
+++ b/foo.rs
@@ -1,2 +1,2 @@
-old
+new
diff --git a/bar.rs b/bar.rs
new file mode 100644
--- /dev/null
+++ b/bar.rs
@@ -0,0 +1 @@
+hello
";
        assert_eq!(
            vec![
                FileHeader { start: 0, end: 4 },
                FileHeader { start: 7, end: 11 },
            ],
            headers_of(input)
        );
    }

    #[test]
    fn test_colored_git_header() {
        let input = b"\
\x1b[1mdiff --git a/foo b/foo\x1b[0m
\x1b[1m\x1b[36mnew file mode 100644\x1b[0m
\x1b[1m\x1b[31m--- /dev/null\x1b[0m
\x1b[1m\x1b[32m+++ b/foo\x1b[0m
\x1b[35m@@ -0,0 +1 @@\x1b[39m
\x1b[32m+x\x1b[39m
";
        assert_eq!(vec![FileHeader { start: 0, end: 4 }], headers_of(input));
    }

    #[test]
    fn test_sl_unified_header() {
        let input = b"\
diff -r 84ed658d4664 foo.rs
--- a/foo.rs\tThu Jan 01 00:00:00 1970 +0000
+++ b/foo.rs\tMon Nov 16 02:57:15 2026 +0000
@@ -1 +1 @@
-old
+new
";
        assert_eq!(vec![FileHeader { start: 0, end: 3 }], headers_of(input));
    }

    #[test]
    fn test_rename_header_includes_extended_meta() {
        let input = b"\
diff --git a/old b/new
similarity index 95%
rename from old
rename to new
index abcdef0..1234567 100644
--- a/old
+++ b/new
@@ -1 +1 @@
-a
+b
";
        assert_eq!(vec![FileHeader { start: 0, end: 7 }], headers_of(input));
    }

    #[test]
    fn test_binary_notice_ends_header() {
        let input = b"\
diff --git a/foo.bin b/foo.bin
index 1111111..2222222 100644
Binary files a/foo.bin and b/foo.bin differ
diff --git a/bar b/bar
--- a/bar
+++ b/bar
@@ -1 +1 @@
-a
+b
";
        assert_eq!(
            vec![
                FileHeader { start: 0, end: 2 },
                FileHeader { start: 3, end: 6 },
            ],
            headers_of(input)
        );
    }

    #[test]
    fn test_classic_unified_pair() {
        let input = b"\
--- a/foo
+++ b/foo
@@ -1 +1 @@
-a
+b
";
        assert_eq!(vec![FileHeader { start: 0, end: 2 }], headers_of(input));
    }

    #[test]
    fn test_preamble_is_not_a_header() {
        let input = b"\
exit status: 1
diff --git a/foo b/foo
--- a/foo
+++ b/foo
@@ -1 +1 @@
-a
+b
";
        let headers = headers_of(input);
        assert_eq!(vec![FileHeader { start: 1, end: 4 }], headers);
        assert_eq!(None, file_header_index(&headers, 0));
        assert_eq!(Some(0), file_header_index(&headers, 1));
        assert_eq!(Some(0), file_header_index(&headers, 6));
    }

    #[test]
    fn test_non_diff_has_no_headers() {
        assert_eq!(Vec::<FileHeader>::new(), headers_of(b"hello\nworld\n"));
    }

    #[test]
    fn test_hunk_body_does_not_start_a_file() {
        // A body line that mentions a diff header is prefixed, so it
        // must not open a new file.
        let input = b"\
diff --git a/foo b/foo
--- a/foo
+++ b/foo
@@ -1,2 +1,2 @@
-diff --git a/x b/x
+diff --git a/y b/y
 context diff --git a/z b/z
";
        assert_eq!(vec![FileHeader { start: 0, end: 3 }], headers_of(input));
    }

    #[test]
    fn test_header_index_selects_section() {
        let headers = vec![
            FileHeader { start: 2, end: 5 },
            FileHeader { start: 10, end: 13 },
        ];
        assert_eq!(None, file_header_index(&headers, 0));
        assert_eq!(None, file_header_index(&headers, 1));
        assert_eq!(Some(0), file_header_index(&headers, 2));
        assert_eq!(Some(0), file_header_index(&headers, 9));
        assert_eq!(Some(1), file_header_index(&headers, 10));
        assert_eq!(Some(1), file_header_index(&headers, 40));
    }
}
