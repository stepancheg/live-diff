use std::cmp;
use std::io;
use std::io::Write;

use crossterm::cursor::Hide;
use crossterm::cursor::MoveTo;
use crossterm::cursor::Show;
use crossterm::queue;
use unicode_width::UnicodeWidthChar;

use crate::diff::DiffOutput;
use crate::diff::FileHeaderSpan;
use crate::diff::FileKey;
use crate::diff::file_header_index;
use crate::lines_with_wrapped_row_count::LinesWithWrappedRowCount;
use crate::render::frame::Frame;
use crate::render::frame::FrameRenderer;
use crate::terminal::height::Height;
use crate::terminal::line::Line;
use crate::terminal::line::compile_search;
use crate::terminal::size::Size;
use crate::terminal::width::Width;
use crate::terminal_bg::TerminalBg;
use crate::theme::search_highlight;
use crate::wrapped_row::WrappedRow;

/// Place in the original `\n`-separated lines, before wrapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LinePos {
    /// Index of the original `\n`-separated line.
    pub(crate) line: usize,
    /// Byte offset into that line. The viewport starts on the wrapped
    /// row that contains this byte, so a width change keeps the same
    /// text at the top.
    pub(crate) offset: usize,
}

/// Styled view of parsed lines.
pub(crate) struct Renderer {
    /// Top of the viewport in original-line coordinates.
    pos: LinePos,
    lines: LinesWithWrappedRowCount,
    terminal_bg: TerminalBg,
    /// Left side of the status bar while a prompt or message is showing.
    /// The right side is the viewport ruler (`Top` / `Bot` / `NN%`).
    status_label: Option<String>,
    /// Caret column in `status_label`. `None` means the end of the label.
    status_caret: Option<u32>,
    /// Persistent note drawn just left of the ruler (untracked count).
    /// Kept across [`Renderer::set_status_label`] so search and context
    /// messages do not hide it.
    status_note: Option<String>,
    /// Current diff mode, drawn left of the note and the ruler.
    /// Kept across [`Renderer::set_status_label`] like [`Self::status_note`].
    status_mode: Option<String>,
    /// Compiled search regex whose matches are drawn with a search highlight.
    highlight: Option<Highlight>,
    /// Bumped when body rows may have changed (`set_lines` / highlight).
    content_gen: u64,
    /// Terminal size last given to [`Renderer::update_size`] (or `new`).
    size: Size,
    /// File-header spans in the projected diff view. Empty when `lines`
    /// were set directly (help text, or a non-diff test buffer).
    file_headers: Vec<FileHeaderSpan>,
    /// File identity for each entry in `file_headers`.
    file_keys: Vec<FileKey>,
}

/// Stable viewport location across diff refreshes.
///
/// A flat line index moves when a file above is inserted or removed.
/// This anchor names the file and the offset inside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScrollAnchor {
    /// Plain buffer with no file headers (help text, a message).
    Line {
        line: usize,
        offset: usize,
        text: String,
    },
    /// Text before the first file header.
    Preamble {
        line: usize,
        offset: usize,
        text: String,
    },
    File {
        key: FileKey,
        /// Index of the file when the anchor was taken. Used when that
        /// file has disappeared, so we stay in the same slot.
        index: usize,
        /// Projected lines from this file's header line. 0 is the header.
        line_offset: usize,
        offset: usize,
        text: String,
    },
}

/// Active search highlight. `current` is the selected match's line and byte offset.
struct Highlight {
    regex: regex::Regex,
    current: Option<(usize, usize)>,
}

impl Renderer {
    pub(crate) fn new(terminal_bg: TerminalBg, size: Size) -> Renderer {
        Renderer {
            pos: LinePos::default(),
            lines: LinesWithWrappedRowCount::new(Vec::new(), size.width),
            terminal_bg,
            status_label: None,
            status_caret: None,
            status_note: None,
            status_mode: None,
            highlight: None,
            content_gen: 0,
            size,
            file_headers: Vec::new(),
            file_keys: Vec::new(),
        }
    }

    /// Store a new terminal size. Recomputes wrap counts when the width
    /// changes and keeps the same text at the top of the viewport.
    pub(crate) fn update_size(&mut self, size: Size) {
        if self.size == size {
            return;
        }
        self.size = size;
        self.lines.update_width(size.width);
        let top = self.viewport_top_row();
        self.set_top_row(top);
    }

    pub(crate) fn set_highlight(
        &mut self,
        pattern: Option<String>,
        current: Option<(usize, usize)>,
        ignore_case: bool,
    ) {
        self.content_gen = self.content_gen.wrapping_add(1);
        self.highlight = match pattern {
            Some(pattern) if !pattern.is_empty() => match compile_search(&pattern, ignore_case) {
                Ok(regex) => Some(Highlight { regex, current }),
                Err(_) => None,
            },
            _ => None,
        };
    }

    pub(crate) fn lines(&self) -> &[Line] {
        self.lines.lines()
    }

    pub(crate) fn terminal_bg(&self) -> TerminalBg {
        self.terminal_bg
    }

    pub(crate) fn set_status_label(&mut self, label: Option<String>) {
        self.status_label = label;
        self.status_caret = None;
    }

    pub(crate) fn set_status_note(&mut self, note: Option<String>) {
        self.status_note = note;
    }

    pub(crate) fn set_status_mode(&mut self, mode: Option<String>) {
        self.status_mode = mode;
    }

    pub(crate) fn status_mode(&self) -> Option<&str> {
        self.status_mode.as_deref()
    }

    /// Right-hand status cluster: optional mode, optional note, and the
    /// ruler. The note is dropped first when the cluster would not fit;
    /// the mode is dropped after that.
    fn status_right(&self, ruler: &str) -> String {
        compose_status_right(
            self.status_mode.as_deref().unwrap_or(""),
            self.status_note.as_deref().unwrap_or(""),
            ruler,
            self.size.width,
        )
    }

    pub(crate) fn status_label(&self) -> Option<&str> {
        self.status_label.as_deref()
    }

    pub(crate) fn set_prompt_status(&mut self, label: String, caret_cols: u32) {
        self.status_label = Some(label);
        self.status_caret = Some(caret_cols);
    }

    pub(crate) fn set_lines(&mut self, lines: Vec<Line>) {
        self.content_gen = self.content_gen.wrapping_add(1);
        self.file_headers = Vec::new();
        self.file_keys = Vec::new();
        self.lines = LinesWithWrappedRowCount::new(lines, self.size.width);
        self.clamp_pos_to_lines();
        let top = self.viewport_top_row();
        self.set_top_row(top);
    }

    /// Replace the buffer with a parsed diff. Display lines and pinned
    /// headers are projected from the file-diff structure. The viewport
    /// is restored by file identity and offset within that file, not by
    /// the previous flat line index.
    pub(crate) fn set_diff(&mut self, output: DiffOutput) {
        let anchor = self.anchor_at(self.top_pos());
        let view = output.view();
        self.content_gen = self.content_gen.wrapping_add(1);
        self.file_headers = view.headers;
        self.file_keys = view.keys;
        self.lines = LinesWithWrappedRowCount::new(view.lines, self.size.width);
        self.pos = self.pos_for_anchor(&anchor);
        self.clamp_pos_to_lines();
        let top = self.viewport_top_row();
        self.set_top_row(top);
    }

    /// Describe `pos` in file + offset terms against the current buffer.
    pub(crate) fn anchor_at(&self, pos: LinePos) -> ScrollAnchor {
        let text = self.line_text(pos.line);
        if self.file_headers.is_empty() || self.file_keys.len() != self.file_headers.len() {
            return ScrollAnchor::Line {
                line: pos.line,
                offset: pos.offset,
                text,
            };
        }
        match file_header_index(&self.file_headers, pos.line) {
            None => ScrollAnchor::Preamble {
                line: pos.line,
                offset: pos.offset,
                text,
            },
            Some(index) => {
                let start = self.file_headers[index].start;
                ScrollAnchor::File {
                    key: self.file_keys[index].clone(),
                    index,
                    line_offset: pos.line.saturating_sub(start),
                    offset: pos.offset,
                    text,
                }
            }
        }
    }

    /// Map an anchor onto the current buffer.
    pub(crate) fn pos_for_anchor(&self, anchor: &ScrollAnchor) -> LinePos {
        match anchor {
            ScrollAnchor::Line { line, offset, text } => {
                let line = self.line_near(*line, text);
                LinePos {
                    line,
                    offset: *offset,
                }
            }
            ScrollAnchor::Preamble { line, offset, text } => {
                let limit = self
                    .file_headers
                    .first()
                    .map(|h| h.start)
                    .unwrap_or(self.lines.len());
                let line = if limit == 0 {
                    0
                } else {
                    let hint = (*line).min(limit - 1);
                    self.line_near_in(hint, 0, limit, text)
                };
                LinePos {
                    line,
                    offset: *offset,
                }
            }
            ScrollAnchor::File {
                key,
                index,
                line_offset,
                offset,
                text,
            } => {
                let Some((file_index, key_matched)) = self.find_file(key, *index) else {
                    return LinePos { line: 0, offset: 0 };
                };
                let (start, end) = self.file_section(file_index);
                if start >= end {
                    return LinePos {
                        line: start.min(self.lines.len().saturating_sub(1)),
                        offset: 0,
                    };
                }
                // The file we were in is gone: stay in that slot, at the
                // replacement file's header.
                if !key_matched {
                    return LinePos {
                        line: start,
                        offset: 0,
                    };
                }
                let hint = start.saturating_add(*line_offset).min(end - 1);
                let line = if text.is_empty() {
                    hint
                } else {
                    self.line_near_in(hint, start, end, text)
                };
                LinePos {
                    line,
                    offset: *offset,
                }
            }
        }
    }

    /// `(file index, true)` when `key` is still present. If the file
    /// was deleted, `(slot, false)` where slot is the old index clamped
    /// into the new file list.
    fn find_file(&self, key: &FileKey, index: usize) -> Option<(usize, bool)> {
        let mut found: Option<usize> = None;
        for (i, candidate) in self.file_keys.iter().enumerate() {
            if candidate != key {
                continue;
            }
            found = Some(match found {
                None => i,
                Some(prev) => {
                    if i.abs_diff(index) < prev.abs_diff(index) {
                        i
                    } else {
                        prev
                    }
                }
            });
        }
        if let Some(i) = found {
            return Some((i, true));
        }
        if self.file_keys.is_empty() {
            None
        } else {
            Some((index.min(self.file_keys.len() - 1), false))
        }
    }

    /// Half-open projected-line range covered by file `index`, from its
    /// header through the line before the next header (or EOF).
    fn file_section(&self, index: usize) -> (usize, usize) {
        let Some(header) = self.file_headers.get(index) else {
            return (0, 0);
        };
        let end = self
            .file_headers
            .get(index + 1)
            .map(|h| h.start)
            .unwrap_or(self.lines.len());
        (header.start, end)
    }

    fn line_text(&self, line: usize) -> String {
        match self.lines.lines().get(line) {
            Some(line) => line.plain_text(),
            None => String::new(),
        }
    }

    fn line_near(&self, hint: usize, text: &str) -> usize {
        let len = self.lines.len();
        if len == 0 {
            return 0;
        }
        let hint = hint.min(len - 1);
        if text.is_empty() {
            return hint;
        }
        self.line_near_in(hint, 0, len, text)
    }

    fn line_near_in(&self, hint: usize, start: usize, end: usize, text: &str) -> usize {
        if start >= end || self.lines.is_empty() {
            return start.min(self.lines.len().saturating_sub(1));
        }
        let hint = hint.clamp(start, end - 1);
        if self.line_text(hint) == text {
            return hint;
        }
        let mut dist = 1usize;
        while dist < end - start {
            if hint >= start + dist && self.line_text(hint - dist) == text {
                return hint - dist;
            }
            if hint + dist < end && self.line_text(hint + dist) == text {
                return hint + dist;
            }
            dist += 1;
        }
        hint
    }

    pub(crate) fn scroll_down(&mut self) {
        // One document row, including file headers. A header is its own
        // stop so crossing into the next file shifts the view by one
        // line. A header with no hunks stays visible. The first file's
        // header is not a stop when lines precede it and the file has
        // a body: there is no previous file to pin, so that row is a
        // duplicate of the pinned-header view.
        let top = self.viewport_top_row();
        let next = top.saturating_add(WrappedRow(1));
        self.set_top_row(self.skip_first_header_stop(next, true));
    }

    pub(crate) fn scroll_up(&mut self) {
        let top = self.viewport_top_row();
        if top == WrappedRow(0) {
            return;
        }
        let prev = top.saturating_sub(WrappedRow(1));
        self.set_top_row(self.skip_first_header_stop(prev, false));
    }

    pub(crate) fn scroll_top(&mut self) {
        self.pos = LinePos::default();
    }

    pub(crate) fn scroll_bottom(&mut self) {
        self.set_top_row(WrappedRow::MAX);
    }

    pub(crate) fn scroll_page_down(&mut self) {
        let layout = self.view_layout();
        // Each content slot is one document row. A pinned header's own
        // row stays in the slice as a blank, so a page is `content_rows`.
        let step = cmp::max(layout.content_rows, 1);
        let next = layout.content_start.saturating_add(WrappedRow(step));
        self.set_top_row(self.skip_first_header_stop(next, true));
    }

    pub(crate) fn scroll_page_up(&mut self) {
        let layout = self.view_layout();
        let step = cmp::max(layout.content_rows, 1);
        let prev = layout.content_start.saturating_sub(WrappedRow(step));
        self.set_top_row(self.skip_first_header_stop(prev, false));
    }

    pub(crate) fn scroll_to_pos(&mut self, pos: LinePos) {
        self.pos = pos;
        self.clamp_pos_to_lines();
        let top = self.viewport_top_row();
        self.set_top_row(top);
    }

    pub(crate) fn scroll_to_line(&mut self, line_idx: usize) {
        if self.lines.is_empty() {
            return;
        }
        let line_idx = line_idx.min(self.lines.len().saturating_sub(1));
        self.set_top_row(visual_row_of_line(self.lines.row_counts(), line_idx));
    }

    /// Scroll the diff so file `index` (into the header list) is at the
    /// top. The path header is pinned and its hunks follow. A file with
    /// no hunks lands on the header itself.
    pub(crate) fn scroll_to_file(&mut self, index: usize) {
        let Some(header) = self.file_headers.get(index).copied() else {
            return;
        };
        let body = self.header_end_row(header);
        if body.as_usize() < self.lines.total() {
            let line = line_at_visual_row(self.lines.row_counts(), body);
            if file_header_index(&self.file_headers, line) == Some(index)
                && !self.row_is_in_header(body)
            {
                self.set_top_row(body);
                return;
            }
        }
        self.scroll_to_line(header.start);
    }

    /// Index of the file whose body is at the top of the viewport.
    pub(crate) fn current_file_index(&self) -> Option<usize> {
        if self.file_headers.is_empty() || self.lines.is_empty() {
            return None;
        }
        file_header_index(&self.file_headers, self.top_line_index())
    }

    /// Scroll the minimum amount so `line_idx` is inside the viewport.
    pub(crate) fn ensure_line_visible(&mut self, line_idx: usize) {
        if self.lines.is_empty() {
            return;
        }
        let line_idx = line_idx.min(self.lines.len().saturating_sub(1));
        let top = self.top_line_index();
        let bottom = self.bottom_line_index();
        if line_idx < top {
            self.scroll_to_line(line_idx);
            return;
        }
        if line_idx > bottom {
            let body = content_height(self.size.height);
            let target = visual_row_of_line(self.lines.row_counts(), line_idx);
            let back = WrappedRow(body.saturating_sub(1));
            self.set_top_row(target.saturating_sub(back));
        }
    }

    pub(crate) fn scroll_to_match(&mut self, line_idx: usize, byte_start: usize) {
        let base = visual_row_of_line(self.lines.row_counts(), line_idx);
        let offset = match self.lines.get(line_idx) {
            Some(line) => visual_row_offset_for_byte(line, self.size.width, byte_start),
            None => WrappedRow(0),
        };
        let match_row = base.saturating_add(offset);
        let body = content_height(self.size.height);
        // Leave the page alone when the hit is already visible. If it is
        // off-screen, center it in the content area, as Neovim does for a
        // jump that moves the cursor outside the window.
        if body == 0 {
            self.set_top_row(match_row);
            return;
        }
        if self.row_visible(match_row) {
            return;
        }
        // Center inside the content area that remains under the pin the
        // match's row would show. A hit in a file header has no pin at
        // that row, so the full body height is used.
        let pin = self.pin_visual_rows_for_content_row(match_row);
        let capacity = cmp::max(content_capacity(body, pin), 1);
        self.set_top_row(match_row.saturating_sub(WrappedRow(capacity / 2)));
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn top_row(&self) -> WrappedRow {
        self.viewport_top_row()
    }

    pub(crate) fn top_pos(&self) -> LinePos {
        let top = self.viewport_top_row();
        pos_of_wrapped_row_at(
            self.lines.lines(),
            self.lines.width(),
            self.lines.row_counts(),
            self.lines.total(),
            top,
        )
    }

    pub(crate) fn top_line_index(&self) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        self.top_pos().line
    }

    pub(crate) fn bottom_line_index(&self) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let total = self.lines.total();
        if total == 0 {
            return 0;
        }
        let layout = self.view_layout();
        let start = cmp::min(layout.content_start.as_usize(), total);
        let end = cmp::min(start.saturating_add(layout.content_rows as usize), total);
        if end == 0 {
            return line_at_visual_row(self.lines.row_counts(), layout.content_start);
        }
        let last = WrappedRow::try_from(end - 1).unwrap_or(WrappedRow::MAX);
        line_at_visual_row(self.lines.row_counts(), last)
    }

    pub(crate) fn draw(
        &mut self,
        out: &mut impl Write,
        frames: &mut FrameRenderer,
    ) -> io::Result<()> {
        let frame = self.compose_frame();
        frames.render(out, frame)
    }

    pub(crate) fn redraw(
        &mut self,
        out: &mut impl Write,
        frames: &mut FrameRenderer,
    ) -> io::Result<()> {
        // A full frame goes through the same presenter. It skips the
        // write when that frame is already on screen.
        self.draw(out, frames)
    }

    fn compose_frame(&self) -> Frame {
        let size = self.size;
        let total = self.lines.total();
        let layout = self.view_layout();
        let start = cmp::min(layout.content_start.as_usize(), total);
        let end = self.flat_end_after_content(layout.content_start, layout.content_rows);
        // Header rows sit above the first body row but are not a scroll
        // offset, so the ruler still reads Top at the start of the buffer.
        let ruler_top = match self.step_content_row_back_from_header(layout.content_start) {
            None => 0,
            Some(_) => start,
        };
        let ruler = viewport_ruler(ruler_top, end, total);
        let right = self.status_right(&ruler);
        let right_cols = cmp::min(display_width(&right).0, size.width.0);
        // Keep one column between the label and the right-hand cluster.
        let label_cols = if right_cols == 0 {
            size.width.0
        } else {
            size.width.0.saturating_sub(right_cols).saturating_sub(1)
        };
        let rows = self.visible_rows(&layout);
        let mut body = Vec::new();
        // `Vec` as `Write` does not return an error.
        let _ = write_one_line(&rows, size.width, &mut body);
        // Cursor only for an editable prompt (`/?`), which sets a caret.
        let cursor = self.status_caret.is_some();
        let (label, caret) = match &self.status_label {
            Some(label) => {
                let caret = self.status_caret.unwrap_or_else(|| display_width(label).0);
                fit_label_to_caret(label, Width(label_cols), caret)
            }
            None => (String::new(), 0),
        };
        let status = status_bar_fingerprint(self.terminal_bg, size.width, &label, &right);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&body);
        let _ = write_status_bar(&mut bytes, size, &status);
        bytes.extend_from_slice(b"\x1b[0m");
        let _ = write_cursor(&mut bytes, size, caret, cursor);
        Frame { bytes }
    }

    fn viewport_top_row(&self) -> WrappedRow {
        self.view_layout().content_start
    }

    fn set_top_row(&mut self, row: WrappedRow) {
        let row = self.clamp_top_row(row);
        self.pos = pos_of_wrapped_row_at(
            self.lines.lines(),
            self.lines.width(),
            self.lines.row_counts(),
            self.lines.total(),
            row,
        );
    }

    /// Content window and optional pinned file header for the current
    /// scroll position.
    ///
    /// The file header is pinned for the whole time that file is on
    /// screen. Header rows are scroll stops. Landing on a header keeps
    /// the *previous* file pinned (that file may then have zero diff
    /// lines on screen) and draws this header in flow, so the two
    /// paths stack and the view shifts by one line.
    fn view_layout(&self) -> ViewLayout {
        let raw = wrapped_row_of_pos_at(
            self.lines.lines(),
            self.lines.width(),
            self.lines.row_counts(),
            self.pos,
        );
        let content_start = self.clamp_top_row(raw);
        let body = content_height(self.size.height);
        let pin_header = self.pin_header_for_content_start(content_start);
        let pin_rows = match pin_header {
            Some(header) => self.pin_visual_rows_for_header(header),
            None => 0,
        };
        let pin_header = if pin_rows == 0 { None } else { pin_header };
        ViewLayout {
            content_start,
            content_rows: content_capacity(body, pin_rows),
            pin_rows,
            pin_header,
        }
    }

    fn clamp_top_row(&self, desired: WrappedRow) -> WrappedRow {
        let min = self.min_top_row();
        let max = self.bottom_top_row();
        let max = cmp::max(max, min);
        cmp::min(cmp::max(desired, min), max)
    }

    /// First scroll stop. A leading file header that has a body is not
    /// a stop, so the opening view is the pin plus hunks with no blank
    /// slot. A leading header with no body is a stop, so that file is
    /// still shown.
    fn min_top_row(&self) -> WrappedRow {
        let total = self.lines.total();
        if total == 0 || !self.row_is_in_header(WrappedRow(0)) {
            return WrappedRow(0);
        }
        let Some(header) = self.file_header_at_row(WrappedRow(0)) else {
            return WrappedRow(0);
        };
        let body = self.header_end_row(header);
        if body.as_usize() >= total || self.row_is_in_header(body) {
            // No body lines in this file.
            return WrappedRow(0);
        }
        let line = line_at_visual_row(self.lines.row_counts(), body);
        if file_header_index(&self.file_headers, line) != Some(0) {
            return WrappedRow(0);
        }
        body
    }

    /// Largest content-start row that still shows the last row.
    ///
    /// The content window is a flat slice of `capacity` document rows.
    /// Pinned-header rows inside that slice are drawn blank but still
    /// occupy their slot, so the window length is the flat distance.
    fn bottom_top_row(&self) -> WrappedRow {
        let total = self.lines.total();
        let body = content_height(self.size.height);
        let min = self.min_top_row();
        if total == 0 || body == 0 {
            return min;
        }
        let last = WrappedRow::try_from(total - 1).unwrap_or(WrappedRow::MAX);
        let pin = self.pin_visual_rows_for_content_row(last);
        let capacity = cmp::max(content_capacity(body, pin), 1) as usize;
        let start = self.bottom_start_for_capacity(total, capacity, min);
        let pin_at_start = self.pin_visual_rows_for_content_row(start);
        let capacity_at_start = cmp::max(content_capacity(body, pin_at_start), 1) as usize;
        if capacity_at_start == capacity {
            return start;
        }
        self.bottom_start_for_capacity(total, capacity_at_start, min)
    }

    fn bottom_start_for_capacity(
        &self,
        total: usize,
        capacity: usize,
        min: WrappedRow,
    ) -> WrappedRow {
        let min_usize = min.as_usize();
        if total.saturating_sub(min_usize) <= capacity {
            return min;
        }
        let start = total - capacity;
        cmp::max(WrappedRow::try_from(start).unwrap_or(WrappedRow::MAX), min)
    }

    /// Move `row` off the first file's header when that header is not a
    /// real scroll stop. `forward` continues toward the file body;
    /// otherwise toward the lines above the header.
    fn skip_first_header_stop(&self, row: WrappedRow, forward: bool) -> WrappedRow {
        let total = self.lines.total();
        if total == 0 {
            return row;
        }
        let mut row = row;
        loop {
            if row.as_usize() >= total {
                return row;
            }
            if !self.row_is_skippable_first_header(row) {
                return row;
            }
            if forward {
                row = row.saturating_add(WrappedRow(1));
            } else {
                if row == WrappedRow(0) {
                    return row;
                }
                row = row.saturating_sub(WrappedRow(1));
            }
        }
    }

    /// The first file's header, when the file has a body and something
    /// precedes the header. Stopping there draws the same path line as
    /// the pinned-header view of the body, with no previous file above
    /// it, so Up/Down must step over it.
    fn row_is_skippable_first_header(&self, row: WrappedRow) -> bool {
        if !self.row_is_in_header(row) {
            return false;
        }
        let Some(header) = self.file_header_at_row(row) else {
            return false;
        };
        let line = line_at_visual_row(self.lines.row_counts(), row);
        let Some(0) = file_header_index(&self.file_headers, line) else {
            return false;
        };
        let body = self.header_end_row(header);
        if body.as_usize() >= self.lines.total() {
            return false;
        }
        if self.row_is_in_header(body) {
            return false;
        }
        let body_line = line_at_visual_row(self.lines.row_counts(), body);
        file_header_index(&self.file_headers, body_line) == Some(0)
    }

    fn step_content_row_back_from_header(&self, start: WrappedRow) -> Option<WrappedRow> {
        if start == WrappedRow(0) {
            return None;
        }
        let mut row = start.saturating_sub(WrappedRow(1));
        loop {
            if !self.row_is_in_header(row) {
                return Some(row);
            }
            if row == WrappedRow(0) {
                return None;
            }
            row = row.saturating_sub(WrappedRow(1));
        }
    }

    fn row_is_in_header(&self, row: WrappedRow) -> bool {
        let total = self.lines.total();
        if total == 0 || row.as_usize() >= total || self.file_headers.is_empty() {
            return false;
        }
        let line = line_at_visual_row(self.lines.row_counts(), row);
        self.file_headers
            .iter()
            .any(|header| line >= header.start && line < header.end)
    }

    fn header_end_row(&self, header: FileHeaderSpan) -> WrappedRow {
        let row_counts = self.lines.row_counts();
        if header.end >= row_counts.len() {
            WrappedRow::try_from(self.lines.total()).unwrap_or(WrappedRow::MAX)
        } else {
            visual_row_of_line(row_counts, header.end)
        }
    }

    /// Visual rows to pin for the file that contains `content_row`.
    /// The header is pinned whenever that file is the one on screen,
    /// including when `content_row` still lies inside the header.
    fn pin_visual_rows_for_content_row(&self, content_row: WrappedRow) -> u32 {
        match self.pin_header_for_content_start(content_row) {
            Some(header) => self.pin_visual_rows_for_header(header),
            None => 0,
        }
    }

    fn pin_visual_rows_for_header(&self, header: FileHeaderSpan) -> u32 {
        let body = content_height(self.size.height);
        if body <= 1 || self.lines.total() == 0 {
            return 0;
        }
        let header_start_row = visual_row_of_line(self.lines.row_counts(), header.start);
        let header_end_row = self.header_end_row(header);
        let header_rows = header_end_row.saturating_sub(header_start_row).0;
        pin_row_budget(header_rows, body)
    }

    fn file_header_at_row(&self, content_row: WrappedRow) -> Option<FileHeaderSpan> {
        let total = self.lines.total();
        if total == 0 || self.file_headers.is_empty() {
            return None;
        }
        let row = cmp::min(
            content_row,
            WrappedRow::try_from(total - 1).unwrap_or(WrappedRow::MAX),
        );
        let line = line_at_visual_row(self.lines.row_counts(), row);
        let index = file_header_index(&self.file_headers, line)?;
        Some(self.file_headers[index])
    }

    fn row_visible(&self, row: WrappedRow) -> bool {
        let layout = self.view_layout();
        if layout.pin_rows > 0 {
            if let Some(header) = layout.pin_header {
                let start = visual_row_of_line(self.lines.row_counts(), header.start);
                let end = start.saturating_add(WrappedRow(layout.pin_rows));
                if row >= start && row < end {
                    return true;
                }
            }
        }
        // Content is a flat slice. In-flow headers of files that start
        // below the pin are part of that slice. The pinned header itself
        // sits above content_start and was handled above.
        let content_end = layout
            .content_start
            .saturating_add(WrappedRow(layout.content_rows));
        row >= layout.content_start && row < content_end
    }

    fn visible_rows(&self, layout: &ViewLayout) -> Vec<VisualRow> {
        let mut rows = Vec::new();
        if layout.pin_rows > 0 {
            if let Some(header) = layout.pin_header {
                let header_start = visual_row_of_line(self.lines.row_counts(), header.start);
                let header_end = header_start.saturating_add(WrappedRow(layout.pin_rows));
                rows = visual_rows_in_range(
                    &self.lines,
                    self.highlight.as_ref(),
                    self.terminal_bg,
                    header_start,
                    header_end,
                );
            }
        }
        let content_end = layout
            .content_start
            .saturating_add(WrappedRow(layout.content_rows));
        let mut content = visual_rows_in_range(
            &self.lines,
            self.highlight.as_ref(),
            self.terminal_bg,
            layout.content_start,
            content_end,
        );
        // The first content row may be a wrap continuation. It must not
        // glue onto the pinned header.
        if !rows.is_empty() {
            if let Some(first) = content.first_mut() {
                first.soft_wrap = false;
            }
        }
        rows.extend(content);
        rows
    }

    /// Flat visual-row cursor after taking `max_rows` rows from `start`.
    /// In-flow file headers count as rows. Used for the viewport ruler.
    fn flat_end_after_content(&self, start: WrappedRow, max_rows: u32) -> usize {
        let total = self.lines.total();
        let end = start.as_usize().saturating_add(max_rows as usize);
        // If the window runs off the end of the buffer, report `total`
        // so the ruler reads Bot.
        cmp::min(end, total)
    }

    /// Header to pin above a content window that starts at `content_start`.
    ///
    /// A body row pins its own file. A header row pins the previous
    /// file, so this header is drawn in flow under that pin. With no
    /// previous file there is no pin.
    fn pin_header_for_content_start(&self, content_start: WrappedRow) -> Option<FileHeaderSpan> {
        let total = self.lines.total();
        if total == 0 || self.file_headers.is_empty() {
            return None;
        }
        if content_start.as_usize() >= total {
            return self
                .file_header_at_row(WrappedRow::try_from(total - 1).unwrap_or(WrappedRow::MAX));
        }
        if self.row_is_in_header(content_start) {
            let line = line_at_visual_row(self.lines.row_counts(), content_start);
            let idx = file_header_index(&self.file_headers, line)?;
            if idx == 0 {
                return None;
            }
            return Some(self.file_headers[idx - 1]);
        }
        self.file_header_at_row(content_start)
    }

    fn clamp_pos_to_lines(&mut self) {
        if self.lines.is_empty() {
            self.pos = LinePos::default();
            return;
        }
        if self.pos.line >= self.lines.len() {
            self.pos.line = self.lines.len() - 1;
        }
        let len = line_byte_len(&self.lines.lines()[self.pos.line]);
        if self.pos.offset > len {
            self.pos.offset = len;
        }
    }
}

/// Rows available for command output. The last terminal row is the status bar.
fn content_height(terminal_height: Height) -> u32 {
    terminal_height.0.saturating_sub(1)
}

/// Content rows left in the body after a pinned header of `pin_rows`.
fn content_capacity(body_height: u32, pin_rows: u32) -> u32 {
    body_height.saturating_sub(pin_rows)
}

/// How many of a header's visual rows may be pinned. At least one body
/// row is kept for content when the body has more than one row.
fn pin_row_budget(header_rows: u32, body_height: u32) -> u32 {
    if body_height <= 1 || header_rows == 0 {
        return 0;
    }
    cmp::min(header_rows, body_height - 1)
}

/// Where the viewport's content rows start, and which file header (if
/// any) is pinned above them.
struct ViewLayout {
    content_start: WrappedRow,
    /// Capacity of the content area, in wrapped rows. The drawn slice
    /// may be shorter at the end of the buffer.
    content_rows: u32,
    pin_rows: u32,
    pin_header: Option<FileHeaderSpan>,
}

/// `mode` and `note` sit just left of `ruler` when they fit in `width`.
/// Parts are separated by spaces. If the full cluster is too wide, the
/// note is omitted first, then the mode.
fn compose_status_right(mode: &str, note: &str, ruler: &str, width: Width) -> String {
    let ruler_cols = display_width(ruler).0;
    if ruler_cols >= width.0 {
        return take_prefix_cols(ruler, width);
    }
    let both = join_status_parts(mode, note, ruler);
    if display_width(&both).0 <= width.0 {
        return both;
    }
    if !mode.is_empty() {
        let mode_ruler = join_status_parts(mode, "", ruler);
        if display_width(&mode_ruler).0 <= width.0 {
            return mode_ruler;
        }
    }
    if !note.is_empty() {
        let note_ruler = join_status_parts("", note, ruler);
        if display_width(&note_ruler).0 <= width.0 {
            return note_ruler;
        }
    }
    ruler.to_owned()
}

fn join_status_parts(mode: &str, note: &str, ruler: &str) -> String {
    let mut s = String::new();
    if !mode.is_empty() {
        s.push_str(mode);
    }
    if !note.is_empty() {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(note);
    }
    if !ruler.is_empty() {
        if !s.is_empty() {
            // Two spaces between the mode and the ruler when they are
            // adjacent. The untracked note keeps a single space.
            if !mode.is_empty() {
                if note.is_empty() {
                    s.push_str("  ");
                } else {
                    s.push(' ');
                }
            } else {
                s.push(' ');
            }
        }
        s.push_str(ruler);
    }
    s
}

fn status_bar_fingerprint(
    terminal_bg: TerminalBg,
    width: Width,
    label: &str,
    ruler: &str,
) -> String {
    if width == 0 {
        return String::new();
    }
    let mut line = String::from(terminal_bg.status_bar_sgr());
    let ruler_cols = display_width(ruler).0.min(width.0);
    let ruler = if display_width(ruler).0 > width.0 {
        take_prefix_cols(ruler, width)
    } else {
        ruler.to_owned()
    };
    let label_cols = display_width(label).0;
    let mut cols = Width(0);
    if label_cols + ruler_cols <= width.0 {
        line.push_str(label);
        cols += Width(label_cols);
    } else if ruler_cols < width.0 {
        let room = width.0 - ruler_cols - 1;
        let fitted = take_prefix_cols(label, Width(room));
        line.push_str(&fitted);
        cols += display_width(&fitted);
    }
    let pad_until = Width(width.0.saturating_sub(ruler_cols));
    while cols < pad_until {
        line.push(' ');
        cols += Width(1);
    }
    line.push_str(&ruler);
    line
}

/// Nvim-style ruler for the viewport: `Top`, `Bot`, or a percent.
/// Always three columns (`Top`, `Bot`, ` 5%`, `50%`) so the mode
/// label to its left does not shift while scrolling.
/// `top` is the first visible row, `end` is one past the last visible
/// row, and `total` is the number of wrapped rows.
fn viewport_ruler(top: usize, end: usize, total: usize) -> String {
    if total == 0 || end >= total {
        return String::from("Bot");
    }
    if top == 0 {
        return String::from("Top");
    }
    let pct = (end as u64 * 100 / total as u64) as u32;
    let pct = cmp::min(cmp::max(pct, 1), 99);
    format!("{pct:>2}%")
}

fn display_width(s: &str) -> Width {
    let mut cols = 0u32;
    for c in s.chars() {
        cols = cols.saturating_add(char_width(c));
    }
    Width(cols)
}

fn char_width(c: char) -> u32 {
    u32::try_from(c.width_cjk().unwrap_or(0)).unwrap_or(0)
}

fn take_prefix_cols(s: &str, width: Width) -> String {
    let mut out = String::new();
    let mut cols = Width(0);
    for c in s.chars() {
        let w = Width(char_width(c));
        if cols + w > width {
            break;
        }
        out.push(c);
        cols += w;
    }
    out
}

/// Visible slice of `label` that keeps the caret on screen, and the caret
/// column within that slice.
fn fit_label_to_caret(label: &str, width: Width, caret_cols: u32) -> (String, u32) {
    let total = display_width(label).0;
    if width == 0 {
        return (String::new(), 0);
    }
    if total <= width.0 {
        return (label.to_owned(), caret_cols.min(total));
    }
    let caret = caret_cols.min(total);
    // Keep the caret inside a `width`-column window. Prefer showing the
    // text before the caret (caret near the right edge).
    let mut start = caret.saturating_sub(width.0.saturating_sub(1));
    if start + width.0 > total {
        start = total - width.0;
    }
    let visible = slice_cols(label, start, width.0);
    (visible, caret - start)
}

fn slice_cols(s: &str, skip_cols: u32, max_cols: u32) -> String {
    let mut out = String::new();
    let mut skipped = 0u32;
    let mut taken = 0u32;
    for c in s.chars() {
        let w = char_width(c);
        if skipped < skip_cols {
            // Drop whole characters until we have skipped enough columns.
            // A double-width char that would overshoot the skip is dropped too
            // so the window stays on a character boundary.
            skipped = skipped.saturating_add(w);
            continue;
        }
        if taken + w > max_cols {
            break;
        }
        out.push(c);
        taken += w;
    }
    out
}

fn line_byte_len(line: &Line) -> usize {
    let mut n = 0;
    for span in &line.spans {
        n += span.content.as_ref().len();
    }
    n
}

fn wrapped_row_of_pos_at(
    lines: &[Line],
    width: Width,
    row_counts: &[WrappedRow],
    pos: LinePos,
) -> WrappedRow {
    if lines.is_empty() {
        return WrappedRow(0);
    }
    let line_idx = cmp::min(pos.line, lines.len() - 1);
    let base = visual_row_of_line(row_counts, line_idx);
    let offset = visual_row_offset_for_byte(&lines[line_idx], width, pos.offset);
    base.saturating_add(offset)
}

fn pos_of_wrapped_row_at(
    lines: &[Line],
    width: Width,
    row_counts: &[WrappedRow],
    total: usize,
    row: WrappedRow,
) -> LinePos {
    if lines.is_empty() {
        return LinePos::default();
    }
    if total == 0 {
        return LinePos::default();
    }
    let last = WrappedRow::try_from(total - 1).unwrap_or(WrappedRow::MAX);
    let row = cmp::min(row, last);
    let line = line_at_visual_row(row_counts, row);
    let base = visual_row_of_line(row_counts, line);
    let within = row.saturating_sub(base);
    let offset = byte_offset_at_wrap_row(&lines[line], width, within);
    LinePos { line, offset }
}

fn visual_row_of_line(row_counts: &[WrappedRow], line_idx: usize) -> WrappedRow {
    let mut acc = WrappedRow(0);
    let end = cmp::min(line_idx, row_counts.len());
    for &n in &row_counts[..end] {
        acc = acc.saturating_add(n);
    }
    acc
}

fn line_at_visual_row(row_counts: &[WrappedRow], visual_row: WrappedRow) -> usize {
    let mut acc = WrappedRow(0);
    for (i, &n) in row_counts.iter().enumerate() {
        // A blank line still occupies one visual row, so n is at least 1
        // unless the terminal width is 0 (every count is then 0).
        let n = cmp::max(n, WrappedRow(1));
        if visual_row < acc.saturating_add(n) {
            return i;
        }
        acc = acc.saturating_add(n);
    }
    row_counts.len().saturating_sub(1)
}

/// Byte offset of the first character placed on wrap-row `target` of `line`.
fn byte_offset_at_wrap_row(line: &Line, terminal_width: Width, target: WrappedRow) -> usize {
    if terminal_width == 0 || target == WrappedRow(0) {
        return 0;
    }
    let mut col = Width(0);
    let mut row = WrappedRow(0);
    let mut byte_off = 0usize;
    for span in &line.spans {
        for ch in span.content.as_ref().chars() {
            let text = ch.as_str().as_str();
            let w = ch.width();
            if w > terminal_width {
                byte_off += text.len();
                continue;
            }
            if col + w > terminal_width {
                row = row.saturating_add(WrappedRow(1));
                col = Width(0);
                if row == target {
                    return byte_off;
                }
            }
            col += w;
            byte_off += text.len();
        }
    }
    byte_off
}

/// Build the wrapped rows in the half-open range `start..end`.
///
/// `start` and `end` are [`WrappedRow`] indices: positions in the
/// sequence of rows after width-wrapping, not indices into `lines`.
/// Lines entirely outside that range are counted, not built.
/// A line that crosses an edge is built and then trimmed.
fn visual_rows_in_range(
    lines: &LinesWithWrappedRowCount,
    highlight: Option<&Highlight>,
    terminal_bg: TerminalBg,
    start: WrappedRow,
    end: WrappedRow,
) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    let terminal_width = lines.width();
    if terminal_width == 0 || start >= end {
        return rows;
    }
    let mut acc = WrappedRow(0);
    for (line_idx, line) in lines.lines().iter().enumerate() {
        let n = lines.row_counts()[line_idx];
        if n == WrappedRow(0) {
            continue;
        }
        let line_end = acc.saturating_add(n);
        if line_end <= start {
            acc = line_end;
            continue;
        }
        if acc >= end {
            break;
        }
        // Rows of this line before `start`, and how many of its rows
        // fall in `start..end`.
        let skip = start.saturating_sub(acc);
        let keep = cmp::min(line_end, end).saturating_sub(cmp::max(acc, start));
        let built = line_visual_rows(line, terminal_width, highlight, line_idx, terminal_bg);
        for part in built
            .into_iter()
            .skip(skip.as_usize())
            .take(keep.as_usize())
        {
            rows.push(part);
        }
        acc = line_end;
        if acc >= end {
            break;
        }
    }
    rows
}

fn visual_row_offset_for_byte(line: &Line, terminal_width: Width, byte_start: usize) -> WrappedRow {
    if terminal_width == 0 {
        return WrappedRow(0);
    }
    let mut col = Width(0);
    let mut row = WrappedRow(0);
    let mut byte_off = 0usize;
    for span in &line.spans {
        for ch in span.content.as_ref().chars() {
            let text = ch.as_str().as_str();
            let w = ch.width();
            if byte_off >= byte_start {
                // The char at byte_start may itself open the next wrap row.
                if w <= terminal_width && col + w > terminal_width {
                    row = row.saturating_add(WrappedRow(1));
                }
                return row;
            }
            byte_off += text.len();
            if w > terminal_width {
                continue;
            }
            if col + w > terminal_width {
                row = row.saturating_add(WrappedRow(1));
                col = Width(0);
            }
            col += w;
        }
    }
    row
}

fn line_visual_rows(
    line: &Line,
    terminal_width: Width,
    highlight: Option<&Highlight>,
    line_idx: usize,
    terminal_bg: TerminalBg,
) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    if terminal_width == 0 {
        return rows;
    }
    let ranges = match highlight {
        Some(highlight) => line.match_ranges(&highlight.regex),
        None => Vec::new(),
    };
    let current_byte = match highlight {
        Some(highlight) => match highlight.current {
            Some((cur_line, byte)) if cur_line == line_idx => Some(byte),
            _ => None,
        },
        None => None,
    };
    let mut pieces: Vec<Piece> = Vec::new();
    let mut col = Width(0);
    let mut soft_wrap = false;
    let mut byte_off = 0usize;
    for span in &line.spans {
        let plain_sgr = span.style.sgr_sequence();
        let highlight_sgr = search_highlight(span.style, terminal_bg, false).sgr_sequence();
        let current_sgr = search_highlight(span.style, terminal_bg, true).sgr_sequence();
        let span_link = span.link.clone();
        for ch in span.content.as_ref().chars() {
            let text = ch.as_str().as_str();
            let ch_end = byte_off + text.len();
            let in_match = ranges
                .iter()
                .find(|&&(start, end)| byte_off < end && ch_end > start);
            let sgr = match in_match {
                Some(&(start, _)) if current_byte == Some(start) => &current_sgr,
                Some(_) => &highlight_sgr,
                None => &plain_sgr,
            };
            byte_off = ch_end;
            let w = ch.width();
            if w > terminal_width {
                continue;
            }
            if col + w > terminal_width {
                rows.push(VisualRow {
                    pieces,
                    width: col,
                    soft_wrap,
                    fill_eol: line.fill_eol,
                });
                pieces = Vec::new();
                col = Width(0);
                soft_wrap = true;
            }
            let link = span_link.clone();
            match pieces.last_mut() {
                Some(last) if last.sgr == *sgr && last.link == link => {
                    last.text.push_str(text);
                }
                _ => {
                    let mut owned = String::new();
                    owned.push_str(text);
                    pieces.push(Piece {
                        sgr: sgr.clone(),
                        text: owned,
                        link,
                    });
                }
            }
            col += w;
        }
    }
    if !pieces.is_empty() || rows.is_empty() {
        rows.push(VisualRow {
            pieces,
            width: col,
            soft_wrap,
            fill_eol: line.fill_eol,
        });
    }
    rows
}

pub(crate) fn terminal_size() -> io::Result<Size> {
    let (cols, rows) = crossterm::terminal::size()?;
    Ok(Size {
        width: Width(u32::from(cols)),
        height: Height(u32::from(rows)),
    })
}

#[derive(Clone, Eq, PartialEq)]
struct VisualRow {
    pieces: Vec<Piece>,
    width: Width,
    /// True when this row continues the same logical line after a width wrap.
    soft_wrap: bool,
    /// Erase to the end of this screen row with the current background.
    fill_eol: bool,
}

#[derive(Clone, Eq, PartialEq)]
struct Piece {
    sgr: String,
    text: String,
    link: Option<String>,
}

#[cfg_attr(not(test), expect(dead_code))]
fn row_fingerprint(row: &VisualRow) -> String {
    let mut s = String::new();
    for piece in &row.pieces {
        s.push_str(&piece.sgr);
        if let Some(link) = &piece.link {
            s.push('\u{1}');
            s.push_str(link);
            s.push('\u{1}');
        }
        s.push_str(&piece.text);
    }
    s
}

fn write_cursor(out: &mut impl Write, size: Size, caret: u32, show_cursor: bool) -> io::Result<()> {
    if size.height.0 == 0 || size.width == 0 {
        return Ok(());
    }
    if !show_cursor {
        queue!(out, Hide)?;
        return Ok(());
    }
    let Ok(row) = u16::try_from(size.height.0 - 1) else {
        return Ok(());
    };
    let col = if caret < size.width.0 {
        caret
    } else {
        size.width.0.saturating_sub(1)
    };
    let Ok(col) = u16::try_from(col) else {
        return Ok(());
    };
    queue!(out, MoveTo(col, row), Show)?;
    Ok(())
}

fn write_status_bar(out: &mut impl Write, size: Size, status: &str) -> io::Result<()> {
    if size.height.0 == 0 || size.width == 0 || status.is_empty() {
        return Ok(());
    }
    let Ok(row) = u16::try_from(size.height.0 - 1) else {
        return Ok(());
    };
    queue!(out, MoveTo(0, row))?;
    out.write_all(status.as_bytes())?;
    Ok(())
}

/// OSC 8 hyperlink closer.
const OSC8_CLOSE: &[u8] = b"\x1b]8;;\x1b\\";

fn write_one_line(
    rows: &[VisualRow],
    terminal_width: Width,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut last_sgr: Option<String> = None;
    let mut open_link: Option<String> = None;
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            let prev = &rows[i - 1];
            let soft = row.soft_wrap && prev.width == terminal_width;
            if !soft {
                out.write_all(b"\r\n")?;
            }
        }
        for piece in &row.pieces {
            if open_link != piece.link {
                if open_link.is_some() {
                    out.write_all(OSC8_CLOSE)?;
                }
                if let Some(link) = &piece.link {
                    out.write_all(b"\x1b]8;;")?;
                    out.write_all(link.as_bytes())?;
                    out.write_all(b"\x1b\\")?;
                }
                open_link = piece.link.clone();
            }
            if last_sgr.as_deref() != Some(piece.sgr.as_str()) {
                out.write_all(piece.sgr.as_bytes())?;
                last_sgr = Some(piece.sgr.clone());
            }
            out.write_all(piece.text.as_bytes())?;
        }
        // Skip EL on a full-width row that soft-wraps: the terminal
        // cursor may sit on the right margin, and the next row's text
        // continues without a newline.
        let next_soft = match rows.get(i + 1) {
            Some(next) => next.soft_wrap && row.width == terminal_width,
            None => false,
        };
        if row.fill_eol {
            if !next_soft {
                // Close the link so EL cannot extend an OSC 8 run.
                if open_link.is_some() {
                    out.write_all(OSC8_CLOSE)?;
                    open_link = None;
                }
                out.write_all(b"\x1b[K")?;
            }
        }
    }
    if open_link.is_some() {
        out.write_all(OSC8_CLOSE)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cmp;
    use std::num::NonZeroU16;

    use super::line_visual_rows;
    use super::row_fingerprint;
    use super::viewport_ruler;
    use super::visual_rows_in_range;
    use crate::lines_with_wrapped_row_count::LinesWithWrappedRowCount;
    use crate::lines_with_wrapped_row_count::line_visual_row_count;
    use crate::render::renderer::Renderer;
    use crate::terminal::color::TerminalColor;
    use crate::terminal::height::Height;
    use crate::terminal::parse::parse_command_output;
    use crate::terminal::size::Size;
    use crate::terminal::width::Width;
    use crate::terminal_bg::TerminalBg;
    use crate::theme::NVIM_DARK_GREY1;
    use crate::theme::NVIM_DARK_YELLOW;
    use crate::theme::NVIM_LIGHT_YELLOW;
    use crate::wrapped_row::WrappedRow;

    fn vt_parser(rows: u16, cols: u16) -> vt100::Parser {
        vt100::Parser::new(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(),
            0,
        )
    }

    fn vt_color(color: TerminalColor) -> vt100::Color {
        match color {
            TerminalColor::Rgb(r, g, b) => vt100::Color::Rgb(r, g, b),
            _ => vt100::Color::Default,
        }
    }

    fn size(width: u32, height: u32) -> Size {
        Size {
            width: Width(width),
            height: Height(height),
        }
    }

    fn draw_screen(input: &[u8], width: u16, height: u16, scroll_downs: u32) -> vt100::Parser {
        let size = size(u32::from(width), u32::from(height));
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(input));
        for _ in 0..scroll_downs {
            renderer.scroll_down();
        }
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(height, width);
        parser.process(&buf);
        parser
    }

    fn row_text(parser: &vt100::Parser, row: u16, width: u16) -> String {
        let mut text = String::new();
        for col in 0..width {
            let Some(cell) = parser.screen().cell(row, col) else {
                break;
            };
            if cell.has_contents() {
                text.push_str(cell.contents());
            } else {
                text.push(' ');
            }
        }
        text.trim_end().to_owned()
    }

    #[test]
    fn test_draw_short_lines() {
        let parser = draw_screen(b"a\nb\nc\n", 10, 5, 0);
        assert_eq!("a", row_text(&parser, 0, 10));
        assert_eq!("b", row_text(&parser, 1, 10));
        assert_eq!("c", row_text(&parser, 2, 10));
        assert_eq!("", row_text(&parser, 3, 10));
        assert_eq!("       Bot", row_text(&parser, 4, 10));
    }

    #[test]
    fn test_draw_limits_to_terminal_height() {
        let parser = draw_screen(b"a\nb\nc\nd\n", 10, 3, 0);
        assert_eq!("a", row_text(&parser, 0, 10));
        assert_eq!("b", row_text(&parser, 1, 10));
        assert_eq!("       Top", row_text(&parser, 2, 10));
    }

    #[test]
    fn test_draw_scroll_down() {
        let parser = draw_screen(b"a\nb\nc\nd\n", 10, 3, 1);
        assert_eq!("b", row_text(&parser, 0, 10));
        assert_eq!("c", row_text(&parser, 1, 10));
        assert_eq!("       75%", row_text(&parser, 2, 10));
    }

    #[test]
    fn test_viewport_ruler_is_three_columns() {
        assert_eq!("Top", viewport_ruler(0, 1, 10));
        assert_eq!("Bot", viewport_ruler(0, 10, 10));
        assert_eq!("Bot", viewport_ruler(9, 10, 10));
        assert_eq!(" 1%", viewport_ruler(1, 1, 100));
        assert_eq!(" 9%", viewport_ruler(1, 9, 100));
        assert_eq!("50%", viewport_ruler(1, 50, 100));
        assert_eq!("99%", viewport_ruler(1, 99, 100));
    }

    #[test]
    fn test_draw_does_not_write_past_last_line() {
        let parser = draw_screen(b"0123456789abcdefghij\nTAIL\n", 10, 3, 0);
        assert_eq!("0123456789", row_text(&parser, 0, 10));
        assert_eq!("abcdefghij", row_text(&parser, 1, 10));
        assert_eq!("       Top", row_text(&parser, 2, 10));
        assert_eq!(
            true,
            parser.screen().contents().contains("0123456789abcdefghij")
        );
        assert_eq!(false, parser.screen().contents().contains("TAIL"));
    }

    #[test]
    fn test_draw_exact_width_line_then_next() {
        let parser = draw_screen(b"0123456789\nnext\n", 10, 3, 0);
        assert_eq!("0123456789", row_text(&parser, 0, 10));
        assert_eq!("next", row_text(&parser, 1, 10));
        assert_eq!("       Bot", row_text(&parser, 2, 10));
    }

    #[test]
    fn test_draw_blank_line_consumes_a_row() {
        let parser = draw_screen(b"a\n\nb\n", 10, 5, 0);
        assert_eq!("a", row_text(&parser, 0, 10));
        assert_eq!("", row_text(&parser, 1, 10));
        assert_eq!("b", row_text(&parser, 2, 10));
    }

    #[test]
    fn test_wrapped_line_is_emitted_as_one_line() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 5));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"0123456789abcdefghij"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert_eq!(true, text.contains("0123456789abcdefghij"));
        let content_at = text.find("0123456789abcdefghij").unwrap();
        let content = &text[content_at..content_at + 20];
        assert_eq!("0123456789abcdefghij", content);
        assert_eq!(false, text.contains('\n'));
        assert_eq!(false, text.contains('\r'));
    }

    #[test]
    fn test_visual_row_count_matches_built_rows() {
        // Width 1 drops every double-width glyph; the line still takes a row.
        let inputs: [&[u8]; 8] = [
            b"",
            b"\n",
            b"a",
            b"abcdef\ngh\n",
            b"\n\n\n",
            b"\x1b[31mhello world\x1b[0m\n",
            "宽宽宽\n".as_bytes(),
            b"0123456789abcdefghij\n",
        ];
        for input in inputs {
            let lines = parse_command_output(input);
            for width in [0_u32, 1, 2, 3, 5, 8, 10, 80] {
                let terminal_width = Width(width);
                let mut built_total = 0usize;
                for (line_idx, line) in lines.iter().enumerate() {
                    let built =
                        line_visual_rows(line, terminal_width, None, line_idx, TerminalBg::Dark)
                            .len();
                    assert_eq!(
                        built,
                        line_visual_row_count(line, terminal_width).as_usize()
                    );
                    built_total = built_total.saturating_add(built);
                }
                let counted = LinesWithWrappedRowCount::new(lines.clone(), terminal_width);
                assert_eq!(built_total, counted.total());
                // A window that starts and ends mid-buffer, including a
                // window that splits a wrapped line, matches a full build.
                let total = built_total;
                if total == 0 {
                    continue;
                }
                let mut full = Vec::new();
                for (line_idx, line) in lines.iter().enumerate() {
                    for part in
                        line_visual_rows(line, terminal_width, None, line_idx, TerminalBg::Dark)
                    {
                        full.push(part);
                    }
                }
                let start = total / 3;
                let end = cmp::min(total, start.saturating_add(cmp::max(1, total / 2)));
                let sliced = visual_rows_in_range(
                    &counted,
                    None,
                    TerminalBg::Dark,
                    WrappedRow::try_from(start).unwrap(),
                    WrappedRow::try_from(end).unwrap(),
                );
                assert_eq!(end - start, sliced.len());
                for (got, expected) in sliced.iter().zip(full[start..end].iter()) {
                    assert_eq!(expected.width, got.width);
                    assert_eq!(expected.soft_wrap, got.soft_wrap);
                    assert_eq!(row_fingerprint(expected), row_fingerprint(got));
                }
            }
        }
    }

    #[test]
    fn test_scroll_moves_by_rendered_row() {
        let parser = draw_screen(b"0123456789abcdefghij\nTAIL\n", 10, 3, 1);
        assert_eq!("abcdefghij", row_text(&parser, 0, 10));
        assert_eq!("TAIL", row_text(&parser, 1, 10));
        assert_eq!("       Bot", row_text(&parser, 2, 10));
    }

    #[test]
    fn test_scroll_top_and_bottom() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\nc\nd\n"));
        renderer.scroll_bottom();
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(3, 10);
        parser.process(&buf);
        assert_eq!("c", row_text(&parser, 0, 10));
        assert_eq!("d", row_text(&parser, 1, 10));
        assert_eq!("       Bot", row_text(&parser, 2, 10));
        renderer.scroll_top();
        buf.clear();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(3, 10);
        parser.process(&buf);
        assert_eq!("a", row_text(&parser, 0, 10));
        assert_eq!("b", row_text(&parser, 1, 10));
        assert_eq!("       Top", row_text(&parser, 2, 10));
        renderer.scroll_bottom();
        let saved = renderer.top_pos();
        renderer.scroll_top();
        renderer.scroll_to_pos(saved);
        assert_eq!(saved, renderer.top_pos());
    }

    #[test]
    fn test_scroll_position_follows_text_across_width_change() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(5, 3));
        let mut prev = crate::render::frame::FrameRenderer::new();
        // Width 5 wraps "0123456789abcdefghij" into 01234/56789/abcde/fghij.
        renderer.set_lines(parse_command_output(b"0123456789abcdefghij\nWXYZ\n"));
        renderer.scroll_down();
        renderer.scroll_down();
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(3, 5);
        parser.process(&buf);
        assert_eq!("abcde", row_text(&parser, 0, 5));
        // Width 8 wraps that line into 01234567/89abcdef/ghij. Byte 10
        // ('a') sits on "89abcdef". A stale wrapped-row index of 2 would
        // show "ghij" instead.
        buf.clear();
        renderer.update_size(size(8, 3));
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(3, 8);
        parser.process(&buf);
        assert_eq!("89abcdef", row_text(&parser, 0, 8));
    }

    #[test]
    fn test_scroll_page_down_and_up() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\nc\nd\ne\nf\n"));
        renderer.scroll_page_down();
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(3, 10);
        parser.process(&buf);
        assert_eq!("c", row_text(&parser, 0, 10));
        assert_eq!("d", row_text(&parser, 1, 10));
        assert_eq!("       66%", row_text(&parser, 2, 10));
        renderer.scroll_page_up();
        buf.clear();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(3, 10);
        parser.process(&buf);
        assert_eq!("a", row_text(&parser, 0, 10));
        assert_eq!("b", row_text(&parser, 1, 10));
        assert_eq!("       Top", row_text(&parser, 2, 10));
    }

    #[test]
    fn test_scroll_clamps_to_last_page() {
        let parser = draw_screen(b"a\nb\nc\nd\n", 10, 3, 3);
        assert_eq!("c", row_text(&parser, 0, 10));
        assert_eq!("d", row_text(&parser, 1, 10));
        assert_eq!("       Bot", row_text(&parser, 2, 10));
    }

    #[test]
    fn test_draw_preserves_fg_and_resets() {
        let parser = draw_screen(b"a\x1b[31mred\x1b[0m b", 10, 2, 0);
        assert_eq!("ared b", row_text(&parser, 0, 10));
        let cell = |col: u16| parser.screen().cell(0, col).unwrap();
        assert_eq!(vt100::Color::Default, cell(0).fgcolor());
        assert_eq!(vt100::Color::Idx(1), cell(1).fgcolor());
        assert_eq!(vt100::Color::Idx(1), cell(2).fgcolor());
        assert_eq!(vt100::Color::Idx(1), cell(3).fgcolor());
        assert_eq!(vt100::Color::Default, cell(4).fgcolor());
        assert_eq!(vt100::Color::Default, cell(5).fgcolor());
    }

    #[test]
    fn test_draw_preserves_bold_and_color_across_wrap() {
        let parser = draw_screen(b"\x1b[1;32m0123456789ab", 10, 3, 0);
        assert_eq!("0123456789", row_text(&parser, 0, 10));
        assert_eq!("ab", row_text(&parser, 1, 10));
        let top = parser.screen().cell(0, 0).unwrap();
        assert_eq!(vt100::Color::Idx(2), top.fgcolor());
        assert_eq!(true, top.bold());
        let wrapped = parser.screen().cell(1, 0).unwrap();
        assert_eq!(vt100::Color::Idx(2), wrapped.fgcolor());
        assert_eq!(true, wrapped.bold());
    }

    #[test]
    fn test_draw_skips_when_visible_unchanged() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 2));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\nc\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(false, buf.is_empty());
        buf.clear();
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(true, buf.is_empty());
    }

    #[test]
    fn test_draw_skips_when_change_is_below_visible_area() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\nc\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        buf.clear();
        renderer.set_lines(parse_command_output(b"a\nb\nX\n"));
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(true, buf.is_empty());
    }

    #[test]
    fn test_draw_writes_when_visible_area_changes() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\nc\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        buf.clear();
        renderer.set_lines(parse_command_output(b"a\nB\nc\n"));
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(false, buf.is_empty());
    }

    #[test]
    fn test_redraw_writes_when_visible_unchanged() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 2));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        buf.clear();
        renderer.redraw(&mut buf, &mut prev).unwrap();
        assert_eq!(false, buf.is_empty());
    }

    #[test]
    fn test_draw_skips_when_file_list_rebuilt_unchanged() {
        // A stat-page refresh rebuilds the rows and calls draw. Identical
        // rows must not clear the screen (that drops a text selection).
        let input = b"\
diff --git a/foo b/foo
--- a/foo
+++ b/foo
@@ -1 +1 @@
-a
+b
";
        let output = crate::diff::parse_diff(input);
        let files = output.files();
        let width = crate::terminal::width::Width(40);
        let lines = crate::diff::file_list_lines(&files, &[], 0, TerminalBg::Dark, width, &[]);
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 10));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(lines);
        renderer.set_status_label(Some(String::from("Enter: go to file")));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(false, buf.is_empty());
        buf.clear();
        let again = crate::diff::file_list_lines(&files, &[], 0, TerminalBg::Dark, width, &[]);
        renderer.set_lines(again);
        renderer.set_status_label(Some(String::from("Enter: go to file")));
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(true, buf.is_empty());
    }

    #[test]
    fn test_search_highlight_sets_bg() {
        let size = size(20, 3);
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"xxFOOyy\na\x1b[31mFOO\x1b[0mb\n"));
        renderer.set_highlight(Some("FOO".to_owned()), Some((0, 2)), false);
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(3, 20);
        parser.process(&buf);
        assert_eq!("xxFOOyy", row_text(&parser, 0, 20));
        let plain = |col: u16| parser.screen().cell(0, col).unwrap();
        assert_eq!(vt100::Color::Default, plain(0).bgcolor());
        assert_eq!(vt100::Color::Default, plain(1).bgcolor());
        // Dark terminal: current hit is light yellow, other hits are dark yellow.
        assert_eq!(vt_color(NVIM_LIGHT_YELLOW), plain(2).bgcolor());
        assert_eq!(vt_color(NVIM_LIGHT_YELLOW), plain(3).bgcolor());
        assert_eq!(vt_color(NVIM_LIGHT_YELLOW), plain(4).bgcolor());
        assert_eq!(vt_color(NVIM_DARK_GREY1), plain(2).fgcolor());
        assert_eq!(vt100::Color::Default, plain(5).bgcolor());
        // Match that crosses a style boundary, on the next line: "a" + "FOO" + "b".
        let crossed = |col: u16| parser.screen().cell(1, col).unwrap();
        assert_eq!("aFOOb", row_text(&parser, 1, 20));
        assert_eq!(vt100::Color::Default, crossed(0).bgcolor());
        assert_eq!(vt_color(NVIM_DARK_YELLOW), crossed(1).bgcolor());
        assert_eq!(vt_color(NVIM_DARK_YELLOW), crossed(2).bgcolor());
        assert_eq!(vt_color(NVIM_DARK_YELLOW), crossed(3).bgcolor());
        assert_eq!(vt100::Color::Default, crossed(4).bgcolor());
    }

    #[test]
    fn test_draw_preserves_bg() {
        let parser = draw_screen(b"\x1b[44mxy", 10, 2, 0);
        assert_eq!("xy", row_text(&parser, 0, 10));
        assert_eq!(
            vt100::Color::Idx(4),
            parser.screen().cell(0, 0).unwrap().bgcolor()
        );
    }

    #[test]
    fn test_status_bar_end_on_dark_bg() {
        let parser = draw_screen(b"a\n", 10, 3, 0);
        assert_eq!("a", row_text(&parser, 0, 10));
        assert_eq!("", row_text(&parser, 1, 10));
        assert_eq!("       Bot", row_text(&parser, 2, 10));
        let cell = parser.screen().cell(2, 0).unwrap();
        assert_eq!(vt100::Color::Idx(8), cell.bgcolor());
        let pad = parser.screen().cell(2, 9).unwrap();
        assert_eq!(vt100::Color::Idx(8), pad.bgcolor());
    }

    #[test]
    fn test_status_bar_shows_top_when_more_below() {
        let parser = draw_screen(b"a\nb\nc\n", 10, 3, 0);
        assert_eq!("a", row_text(&parser, 0, 10));
        assert_eq!("b", row_text(&parser, 1, 10));
        assert_eq!("       Top", row_text(&parser, 2, 10));
        let cell = parser.screen().cell(2, 0).unwrap();
        assert_eq!(vt100::Color::Idx(8), cell.bgcolor());
    }

    #[test]
    fn test_status_bar_shows_untracked_note() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(28, 2));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\n"));
        renderer.set_status_note(Some(String::from("3 untracked")));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(2, 28);
        parser.process(&buf);
        // Right-aligned: note, space, ruler. "3 untracked Bot" is 15 cols.
        assert_eq!("             3 untracked Bot", row_text(&parser, 1, 28));
        // A left-hand label stays on the left; the note stays by the ruler.
        renderer.set_status_label(Some(String::from("context 3")));
        buf.clear();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(2, 28);
        parser.process(&buf);
        assert_eq!("context 3    3 untracked Bot", row_text(&parser, 1, 28));
    }

    #[test]
    fn test_status_bar_shows_diff_mode() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 2));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\n"));
        renderer.set_status_mode(Some(String::from("diff -c .")));
        renderer.set_status_note(Some(String::from("3 untracked")));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(2, 40);
        parser.process(&buf);
        // mode, note, ruler, right-aligned.
        assert_eq!(
            "               diff -c . 3 untracked Bot",
            row_text(&parser, 1, 40)
        );
        // Narrow: keep the mode, drop the untracked note.
        // "diff -c . Bot" is 13 cols; "diff -c . 3 untracked Bot" is 25.
        renderer.update_size(size(16, 2));
        buf.clear();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(2, 16);
        parser.process(&buf);
        assert_eq!("  diff -c .  Bot", row_text(&parser, 1, 16));
    }

    #[test]
    fn test_status_bar_light_grey_on_light_bg() {
        let mut renderer = Renderer::new(TerminalBg::Light, size(10, 2));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(2, 10);
        parser.process(&buf);
        assert_eq!("       Bot", row_text(&parser, 1, 10));
        let cell = parser.screen().cell(1, 0).unwrap();
        assert_eq!(vt100::Color::Idx(7), cell.bgcolor());
        let pad = parser.screen().cell(1, 9).unwrap();
        assert_eq!(vt100::Color::Idx(7), pad.bgcolor());
    }

    #[test]
    fn test_status_update_does_not_repaint_body() {
        let size = size(10, 3);
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\nc\nd\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(true, buf.windows(4).any(|w| w == b"\x1b[2J"));
        buf.clear();
        renderer.set_prompt_status("/hi".to_owned(), 3);
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(false, buf.is_empty());
        assert_eq!(false, buf.windows(4).any(|w| w == b"\x1b[2J"));
        assert_eq!(true, buf.windows(3).any(|w| w == b"/hi"));
    }

    #[test]
    fn test_draw_writes_when_end_indicator_appears() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\nc\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        buf.clear();
        renderer.set_lines(parse_command_output(b"a\nb\n"));
        renderer.draw(&mut buf, &mut prev).unwrap();
        assert_eq!(false, buf.is_empty());
    }

    fn context_file_diff(name: &str, n: usize) -> String {
        let mut text = format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n");
        text.push_str(&format!("@@ -1,{n} +1,{n} @@\n"));
        for i in 0..n {
            text.push_str(&format!(" line{i}\n"));
        }
        text
    }

    fn draw_diff(input: &[u8], width: u16, height: u16, scroll_downs: u32) -> vt100::Parser {
        let size = size(u32::from(width), u32::from(height));
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_diff(crate::diff::parse_diff(input));
        for _ in 0..scroll_downs {
            renderer.scroll_down();
        }
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(height, width);
        parser.process(&buf);
        parser
    }

    #[test]
    fn test_scroll_past_first_header_after_preamble_is_one_step() {
        // A commit message (or any text) above the first file makes
        // that file's header reachable. It must not take two steps:
        // the stop has no previous file to pin, so the view matches
        // the pinned-header body and looks unchanged.
        let mut input = String::from("commit subject\n");
        input.push_str(&context_file_diff("foo", 8));
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 8));
        renderer.set_diff(crate::diff::parse_diff(input.as_bytes()));
        assert_eq!(
            "commit subject",
            renderer.lines()[renderer.top_line_index()].plain_text()
        );
        renderer.scroll_down();
        assert_eq!(
            "@@ -1,8 +1,8 @@",
            renderer.lines()[renderer.top_line_index()].plain_text()
        );
        renderer.scroll_up();
        assert_eq!(
            "commit subject",
            renderer.lines()[renderer.top_line_index()].plain_text()
        );
    }

    #[test]
    fn test_first_header_without_body_after_preamble_is_a_stop() {
        // A mode-only first file has no hunks. Its header is the only
        // row, so Up/Down must stop on it.
        let mut input = String::from("commit subject\n");
        input.push_str("diff --git a/mode b/mode\nold mode 100644\nnew mode 100755\n");
        input.push_str(&context_file_diff("foo", 8));
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 8));
        renderer.set_diff(crate::diff::parse_diff(input.as_bytes()));
        renderer.scroll_down();
        assert_eq!(
            "mode",
            renderer.lines()[renderer.top_line_index()].plain_text()
        );
        renderer.scroll_up();
        assert_eq!(
            "commit subject",
            renderer.lines()[renderer.top_line_index()].plain_text()
        );
    }

    #[test]
    fn test_file_header_not_pinned_at_top() {
        // Height 8 → 7 body rows. The path header is pinned above the
        // hunk from the first frame; it is not a second copy.
        let input = context_file_diff("foo", 6);
        let parser = draw_diff(input.as_bytes(), 40, 8, 0);
        assert_eq!("foo", row_text(&parser, 0, 40));
        assert_eq!("@@ -1,6 +1,6 @@", row_text(&parser, 1, 40));
        assert_eq!(" line0", row_text(&parser, 2, 40));
        assert_eq!(" line1", row_text(&parser, 3, 40));
        assert_eq!(1, parser.screen().contents().matches("foo").count());
    }

    #[test]
    fn test_scroll_down_shifts_body_by_one_line() {
        // One Down must move the body by one line. The path stays put.
        // The line that was directly under the header leaves, and the
        // line that was at the bottom of the body moves up by one — no
        // extra line is dropped to make room for the pin.
        // Height 8 → 7 rows, pin 1, content capacity 6.
        let input = context_file_diff("foo", 12);
        let top = draw_diff(input.as_bytes(), 40, 8, 0);
        assert_eq!("foo", row_text(&top, 0, 40));
        assert_eq!("@@ -1,12 +1,12 @@", row_text(&top, 1, 40));
        assert_eq!(" line0", row_text(&top, 2, 40));
        assert_eq!(" line4", row_text(&top, 6, 40));
        let once = draw_diff(input.as_bytes(), 40, 8, 1);
        assert_eq!("foo", row_text(&once, 0, 40));
        assert_eq!(" line0", row_text(&once, 1, 40));
        assert_eq!(" line1", row_text(&once, 2, 40));
        assert_eq!(" line5", row_text(&once, 6, 40));
        let thrice = draw_diff(input.as_bytes(), 40, 8, 3);
        assert_eq!("foo", row_text(&thrice, 0, 40));
        assert_eq!(" line2", row_text(&thrice, 1, 40));
        assert_eq!(" line3", row_text(&thrice, 2, 40));
    }

    #[test]
    fn test_pinned_header_switches_to_current_file() {
        let mut input = context_file_diff("foo", 20);
        input.push_str(&context_file_diff("bar", 20));
        // foo body is @@ + 20 lines = 21 rows. That many Downs lands on
        // the first body row of bar, with bar's path pinned.
        // 21 Downs land on bar's header: foo is still pinned above it.
        // 22 Downs reach bar's first hunk line and pin bar.
        let parser = draw_diff(input.as_bytes(), 40, 8, 21);
        assert_eq!("foo", row_text(&parser, 0, 40));
        assert_eq!("bar", row_text(&parser, 1, 40));
        let parser = draw_diff(input.as_bytes(), 40, 8, 22);
        assert_eq!("bar", row_text(&parser, 0, 40));
        assert_eq!("@@ -1,20 +1,20 @@", row_text(&parser, 1, 40));
        let parser = draw_diff(input.as_bytes(), 40, 8, 24);
        assert_eq!("bar", row_text(&parser, 0, 40));
        assert_eq!(" line1", row_text(&parser, 1, 40));
        assert_eq!(false, row_text(&parser, 0, 40).contains("foo"));
    }

    #[test]
    fn test_in_flow_header_kept_when_next_file_starts_mid_screen() {
        // Height 8 → 7 body rows, pin 1, capacity 6.
        // foo: header + @@ + 10 lines = 12 rows (indices 0..11).
        // bar starts at index 12.
        // 10 Downs from the first body row (index 1) lands on foo's
        // last line (index 11). bar's path must appear where its hunks
        // begin, under the pinned foo header.
        let mut input = context_file_diff("foo", 10);
        input.push_str(&context_file_diff("bar", 10));
        let junction = draw_diff(input.as_bytes(), 40, 8, 10);
        assert_eq!("foo", row_text(&junction, 0, 40));
        assert_eq!(" line9", row_text(&junction, 1, 40));
        assert_eq!("bar", row_text(&junction, 2, 40));
        assert_eq!("@@ -1,10 +1,10 @@", row_text(&junction, 3, 40));
        assert_eq!(" line0", row_text(&junction, 4, 40));
        // One more Down lands on bar's header. foo stays pinned (zero
        // of its hunks remain on screen) and bar's path is drawn in
        // flow directly under that pin. @@ moves from row 3 to row 2.
        let on_bar_header = draw_diff(input.as_bytes(), 40, 8, 11);
        assert_eq!("foo", row_text(&on_bar_header, 0, 40));
        assert_eq!("bar", row_text(&on_bar_header, 1, 40));
        assert_eq!("@@ -1,10 +1,10 @@", row_text(&on_bar_header, 2, 40));
        assert_eq!(" line0", row_text(&on_bar_header, 3, 40));
        // The next Down pins bar and shows its hunks under the pin.
        let on_bar = draw_diff(input.as_bytes(), 40, 8, 12);
        assert_eq!("bar", row_text(&on_bar, 0, 40));
        assert_eq!("@@ -1,10 +1,10 @@", row_text(&on_bar, 1, 40));
        assert_eq!(" line0", row_text(&on_bar, 2, 40));
    }

    #[test]
    fn test_set_diff_keeps_file_when_earlier_file_removed() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 8));
        let mut first = context_file_diff("aaa", 10);
        first.push_str(&context_file_diff("bbb", 10));
        first.push_str(&context_file_diff("ccc", 10));
        renderer.set_diff(crate::diff::parse_diff(first.as_bytes()));
        renderer.scroll_to_file(2);
        // Move onto a body line inside ccc, not the @@ header.
        renderer.scroll_down();
        renderer.scroll_down();
        let before = renderer.lines()[renderer.top_pos().line].plain_text();
        assert_eq!(" line1", before);
        assert_eq!(Some(2), renderer.current_file_index());

        let mut second = context_file_diff("bbb", 10);
        second.push_str(&context_file_diff("ccc", 10));
        renderer.set_diff(crate::diff::parse_diff(second.as_bytes()));
        assert_eq!(Some(1), renderer.current_file_index());
        assert_eq!(
            before,
            renderer.lines()[renderer.top_pos().line].plain_text()
        );
    }

    #[test]
    fn test_set_diff_deleted_file_stays_in_slot() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 8));
        let mut first = context_file_diff("aaa", 10);
        first.push_str(&context_file_diff("bbb", 10));
        first.push_str(&context_file_diff("ccc", 10));
        renderer.set_diff(crate::diff::parse_diff(first.as_bytes()));
        renderer.scroll_to_file(1);
        renderer.scroll_down();
        renderer.scroll_down();
        assert_eq!(Some(1), renderer.current_file_index());

        // bbb is gone. The slot it occupied is now ccc; land on ccc's header.
        let mut second = context_file_diff("aaa", 10);
        second.push_str(&context_file_diff("ccc", 10));
        renderer.set_diff(crate::diff::parse_diff(second.as_bytes()));
        assert_eq!(Some(1), renderer.current_file_index());
        assert_eq!(
            "ccc",
            renderer.lines()[renderer.top_pos().line].plain_text()
        );
    }

    #[test]
    fn test_header_with_no_hunks_is_a_scroll_stop() {
        // A mode-only file has a header and no hunks. Down from the
        // previous file must stop on that header rather than skipping it.
        let mut input = context_file_diff("foo", 8);
        input.push_str("diff --git a/mode b/mode\nold mode 100644\nnew mode 100755\n");
        input.push_str(&context_file_diff("bar", 8));
        // Projected rows: foo header+@@+8 = 10 (0..9), mode header
        // alone at row 10, bar after that. min stop is row 1.
        // 8 Downs reach foo's last line (row 9).
        let junction = draw_diff(input.as_bytes(), 40, 8, 8);
        assert_eq!("foo", row_text(&junction, 0, 40));
        assert_eq!(" line7", row_text(&junction, 1, 40));
        assert_eq!("mode", row_text(&junction, 2, 40));
        let on_empty = draw_diff(input.as_bytes(), 40, 8, 9);
        // foo stays pinned with zero hunks on screen; mode's path is
        // the next line. One further Down pins mode (still no hunks)
        // above bar.
        assert_eq!("foo", row_text(&on_empty, 0, 40));
        assert_eq!("mode", row_text(&on_empty, 1, 40));
        assert_eq!("bar", row_text(&on_empty, 2, 40));
        let on_mode_pin = draw_diff(input.as_bytes(), 40, 8, 10);
        assert_eq!("mode", row_text(&on_mode_pin, 0, 40));
        assert_eq!("bar", row_text(&on_mode_pin, 1, 40));
    }

    #[test]
    fn test_scroll_bottom_keeps_last_line_and_file_header() {
        let mut input = context_file_diff("foo", 20);
        input.push_str(&context_file_diff("bar", 20));
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 8));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_diff(crate::diff::parse_diff(input.as_bytes()));
        renderer.scroll_bottom();
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(8, 40);
        parser.process(&buf);
        assert_eq!("bar", row_text(&parser, 0, 40));
        // Pin 1, content capacity 6. Last content row is bar's last line.
        assert_eq!(" line19", row_text(&parser, 6, 40));
        assert_eq!(true, row_text(&parser, 7, 40).ends_with("Bot"));
    }

    #[test]
    fn test_scroll_bottom_long_file_shows_last_lines_under_header() {
        // 30 context lines, last one renamed so the assertion is unique.
        // Counts must match: 29 "lineN" lines plus "last".
        let mut text = String::from("diff --git a/foo b/foo\n--- a/foo\n+++ b/foo\n");
        text.push_str("@@ -1,30 +1,30 @@\n");
        for i in 0..29 {
            text.push_str(&format!(" line{i}\n"));
        }
        text.push_str(" last\n");
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 8));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_diff(crate::diff::parse_diff(text.as_bytes()));
        renderer.scroll_bottom();
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(8, 40);
        parser.process(&buf);
        assert_eq!("foo", row_text(&parser, 0, 40));
        // Pin 1, content capacity 6. Last content row is " last".
        assert_eq!(" last", row_text(&parser, 6, 40));
        assert_eq!(true, row_text(&parser, 7, 40).ends_with("Bot"));
    }

    #[test]
    fn test_pinned_header_keeps_style() {
        let input = context_file_diff("foo", 8);
        let parser = draw_diff(input.as_bytes(), 40, 8, 3);
        assert_eq!("foo", row_text(&parser, 0, 40));
        let cell = parser.screen().cell(0, 0).unwrap();
        assert_eq!(true, cell.bold());
    }

    #[test]
    fn test_non_diff_scroll_bottom_unchanged() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        let mut prev = crate::render::frame::FrameRenderer::new();
        renderer.set_lines(parse_command_output(b"a\nb\nc\nd\ne\nf\n"));
        renderer.scroll_bottom();
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &mut prev).unwrap();
        let mut parser = vt_parser(3, 10);
        parser.process(&buf);
        assert_eq!("e", row_text(&parser, 0, 10));
        assert_eq!("f", row_text(&parser, 1, 10));
        assert_eq!("       Bot", row_text(&parser, 2, 10));
    }
}
