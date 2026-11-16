use std::cmp;
use std::io;
use std::io::Write;

use crossterm::cursor::Hide;
use crossterm::cursor::MoveTo;
use crossterm::cursor::Show;
use crossterm::queue;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use unicode_width::UnicodeWidthChar;

use crate::diff::DiffOutput;
use crate::diff::FileHeaderSpan;
use crate::diff::file_header_index;
use crate::height::Height;
use crate::lines_with_wrapped_row_count::LinesWithWrappedRowCount;
use crate::size::Size;
use crate::terminal::line::Line;
use crate::terminal::line::compile_search;
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
    drawn: Option<DrawnFrame>,
    terminal_bg: TerminalBg,
    /// Left side of the status bar while a prompt or message is showing.
    /// The right side is the viewport ruler (`Top` / `Bot` / `NN%`).
    status_label: Option<String>,
    /// Caret column in `status_label`. `None` means the end of the label.
    status_caret: Option<u32>,
    /// Compiled search regex whose matches are drawn with a search highlight.
    highlight: Option<Highlight>,
    /// Bumped when body rows may have changed (`set_lines` / highlight).
    content_gen: u64,
    /// Terminal size last given to [`Renderer::update_size`] (or `new`).
    size: Size,
    /// File-header spans in the projected diff view. Empty when `lines`
    /// were set directly (help text, or a non-diff test buffer).
    file_headers: Vec<FileHeaderSpan>,
}

/// Active search highlight. `current` is the selected match's line and byte offset.
struct Highlight {
    regex: regex::Regex,
    current: Option<(usize, usize)>,
}

struct DrawnFrame {
    size: Size,
    rows: Vec<String>,
    status: String,
    caret: u32,
    cursor: bool,
    pos: LinePos,
    content_gen: u64,
    ruler: String,
}

impl Renderer {
    pub(crate) fn new(terminal_bg: TerminalBg, size: Size) -> Renderer {
        Renderer {
            pos: LinePos::default(),
            lines: LinesWithWrappedRowCount::new(Vec::new(), size.width),
            drawn: None,
            terminal_bg,
            status_label: None,
            status_caret: None,
            highlight: None,
            content_gen: 0,
            size,
            file_headers: Vec::new(),
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

    pub(crate) fn set_status_label(&mut self, label: Option<String>) {
        self.status_label = label;
        self.status_caret = None;
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
        self.lines = LinesWithWrappedRowCount::new(lines, self.size.width);
        self.clamp_pos_to_lines();
        let top = self.viewport_top_row();
        self.set_top_row(top);
    }

    /// Replace the buffer with a parsed diff. Display lines and pinned
    /// headers are projected from the file-diff structure.
    pub(crate) fn set_diff(&mut self, output: DiffOutput) {
        let view = output.view();
        self.content_gen = self.content_gen.wrapping_add(1);
        self.file_headers = view.headers;
        self.lines = LinesWithWrappedRowCount::new(view.lines, self.size.width);
        self.clamp_pos_to_lines();
        let top = self.viewport_top_row();
        self.set_top_row(top);
    }

    pub(crate) fn scroll_down(&mut self) {
        let top = self.viewport_top_row();
        // Step to the next body row. Header rows are not scroll stops:
        // they are drawn only as the pin, so moving onto one would not
        // shift the body and would drop a line at the bottom.
        let next = self.step_content_row(top, 1);
        self.set_top_row(next);
    }

    pub(crate) fn scroll_up(&mut self) {
        let top = self.viewport_top_row();
        if top == WrappedRow(0) {
            return;
        }
        let prev = self.step_content_row_back(top);
        self.set_top_row(prev);
    }

    pub(crate) fn scroll_top(&mut self) {
        self.pos = LinePos::default();
    }

    pub(crate) fn scroll_bottom(&mut self) {
        self.set_top_row(WrappedRow::MAX);
    }

    pub(crate) fn scroll_page_down(&mut self) {
        let layout = self.view_layout();
        let step = cmp::max(layout.content_rows, 1);
        let next = self.step_content_row(layout.content_start, step);
        self.set_top_row(next);
    }

    pub(crate) fn scroll_page_up(&mut self) {
        let layout = self.view_layout();
        let step = cmp::max(layout.content_rows, 1);
        let mut row = layout.content_start;
        for _ in 0..step {
            if row == WrappedRow(0) {
                break;
            }
            row = self.step_content_row_back(row);
        }
        self.set_top_row(row);
    }

    pub(crate) fn scroll_to_pos(&mut self, pos: LinePos) {
        self.pos = pos;
        self.clamp_pos_to_lines();
        let top = self.viewport_top_row();
        self.set_top_row(top);
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn scroll_to_line(&mut self, line_idx: usize) {
        self.set_top_row(visual_row_of_line(self.lines.row_counts(), line_idx));
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

    pub(crate) fn draw(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.draw_frame(out, false)
    }

    pub(crate) fn redraw(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.draw_frame(out, true)
    }

    fn draw_frame(&mut self, out: &mut impl Write, force: bool) -> io::Result<()> {
        let size = self.size;
        if !force && self.body_cache_valid() {
            let ruler = match &self.drawn {
                Some(drawn) => drawn.ruler.clone(),
                None => String::new(),
            };
            return self.paint_status(out, &ruler);
        }
        let terminal_width = size.width;
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
        let ruler_cols = display_width(&ruler).0.min(size.width.0);
        // Keep one column between the label and the ruler when both fit.
        let label_cols = if ruler_cols == 0 {
            size.width.0
        } else {
            size.width.0.saturating_sub(ruler_cols).saturating_sub(1)
        };
        let visible = self.visible_rows(&layout);
        let keys: Vec<String> = visible.iter().map(row_fingerprint).collect();
        // Cursor only for an editable prompt (`/?`), which sets a caret.
        let show_cursor = self.status_caret.is_some();
        let (label, caret) = match &self.status_label {
            Some(label) => {
                let caret = self.status_caret.unwrap_or_else(|| display_width(label).0);
                fit_label_to_caret(label, Width(label_cols), caret)
            }
            None => (String::new(), 0),
        };
        let status = status_bar_fingerprint(self.terminal_bg, size.width, &label, &ruler);
        let unchanged = match &self.drawn {
            Some(drawn) => {
                !force
                    && drawn.size == size
                    && drawn.rows == keys
                    && drawn.status == status
                    && drawn.caret == caret
                    && drawn.cursor == show_cursor
            }
            None => false,
        };
        if unchanged {
            return Ok(());
        }
        let mut stream = Vec::new();
        queue!(&mut stream, MoveTo(0, 0), Clear(ClearType::All))?;
        write_one_line(&visible, terminal_width, &mut stream)?;
        write_status_bar(&mut stream, size, &status)?;
        stream.extend_from_slice(b"\x1b[0m");
        write_cursor(&mut stream, size, caret, show_cursor)?;
        out.write_all(&stream)?;
        out.flush()?;
        self.drawn = Some(DrawnFrame {
            size,
            rows: keys,
            status,
            caret,
            cursor: show_cursor,
            pos: self.pos,
            content_gen: self.content_gen,
            ruler,
        });
        Ok(())
    }

    fn body_cache_valid(&self) -> bool {
        match &self.drawn {
            Some(drawn) => {
                drawn.size == self.size
                    && drawn.pos == self.pos
                    && drawn.content_gen == self.content_gen
            }
            None => false,
        }
    }

    /// Repaint only the status row and cursor. Body rows stay as they are.
    fn paint_status(&mut self, out: &mut impl Write, ruler: &str) -> io::Result<()> {
        let size = self.size;
        let ruler_cols = display_width(ruler).0.min(size.width.0);
        let label_cols = if ruler_cols == 0 {
            size.width.0
        } else {
            size.width.0.saturating_sub(ruler_cols).saturating_sub(1)
        };
        let show_cursor = self.status_caret.is_some();
        let (label, caret) = match &self.status_label {
            Some(label) => {
                let caret = self.status_caret.unwrap_or_else(|| display_width(label).0);
                fit_label_to_caret(label, Width(label_cols), caret)
            }
            None => (String::new(), 0),
        };
        let status = status_bar_fingerprint(self.terminal_bg, size.width, &label, ruler);
        let unchanged = match &self.drawn {
            Some(drawn) => {
                drawn.status == status && drawn.caret == caret && drawn.cursor == show_cursor
            }
            None => false,
        };
        if unchanged {
            return Ok(());
        }
        let mut stream = Vec::new();
        write_status_bar(&mut stream, size, &status)?;
        stream.extend_from_slice(b"\x1b[0m");
        write_cursor(&mut stream, size, caret, show_cursor)?;
        out.write_all(&stream)?;
        out.flush()?;
        if let Some(drawn) = self.drawn.as_mut() {
            drawn.status = status;
            drawn.caret = caret;
            drawn.cursor = show_cursor;
        }
        Ok(())
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
    /// screen, including at the top of the file. Header rows are not
    /// part of the content window, so a one-line scroll shifts the body
    /// by one line.
    fn view_layout(&self) -> ViewLayout {
        let raw = wrapped_row_of_pos_at(
            self.lines.lines(),
            self.lines.width(),
            self.lines.row_counts(),
            self.pos,
        );
        let content_start = self.clamp_top_row(self.skip_header_rows(raw));
        let body = content_height(self.size.height);
        let pin_header = self.file_header_at_row(content_start);
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
        let row = cmp::min(desired, self.bottom_top_row());
        // bottom_top_row is a body row. If desired was a header, the
        // caller has usually already skipped it; skip again in case
        // the clamp landed on one.
        let skipped = self.skip_header_rows(row);
        if skipped.as_usize() >= self.lines.total() {
            return row;
        }
        // Skipping forward can pass the bottom; don't scroll past it.
        let bottom = self.bottom_top_row();
        if skipped > bottom { bottom } else { skipped }
    }

    /// Largest content-start row that still shows the last body row.
    ///
    /// Header rows are excluded from the window, and a pinned header
    /// shrinks the content area, so this is the body row `capacity`
    /// non-header rows before the end.
    fn bottom_top_row(&self) -> WrappedRow {
        let total = self.lines.total();
        let body = content_height(self.size.height);
        if total == 0 || body == 0 {
            return WrappedRow(0);
        }
        let last = WrappedRow::try_from(total - 1).unwrap_or(WrappedRow::MAX);
        let pin = self.pin_visual_rows_for_content_row(last);
        let capacity = cmp::max(content_capacity(body, pin), 1);
        let start = self.content_row_back(last, capacity - 1);
        // Pin height at `start` may differ from the pin at `last`.
        // Recompute once with that height.
        let pin_at_start = self.pin_visual_rows_for_content_row(start);
        let capacity_at_start = cmp::max(content_capacity(body, pin_at_start), 1);
        if capacity_at_start == capacity {
            return start;
        }
        self.content_row_back(last, capacity_at_start - 1)
    }

    /// The non-header row `steps` body rows before `from`, inclusive of
    /// `from` when `from` is itself a body row and `steps` is 0.
    /// `steps` counts only non-header rows. Stops at the first body row.
    fn content_row_back(&self, from: WrappedRow, steps: u32) -> WrappedRow {
        let total = self.lines.total();
        if total == 0 {
            return WrappedRow(0);
        }
        let mut row = cmp::min(
            from,
            WrappedRow::try_from(total - 1).unwrap_or(WrappedRow::MAX),
        );
        // If `from` is a header, start from the body row just above it.
        if self.row_is_in_header(row) {
            match self.step_content_row_back_from_header(row) {
                Some(prev) => row = prev,
                None => return WrappedRow(0),
            }
        }
        for _ in 0..steps {
            if row == WrappedRow(0) {
                break;
            }
            row = self.step_content_row_back(row);
        }
        row
    }

    /// Next body row at or after `row`. Header rows are skipped.
    fn skip_header_rows(&self, row: WrappedRow) -> WrappedRow {
        let total = self.lines.total();
        let mut row = row;
        while row.as_usize() < total && self.row_is_in_header(row) {
            let line = line_at_visual_row(self.lines.row_counts(), row);
            // Skip the rest of this header span, not just the current line.
            let end = match self
                .file_headers
                .iter()
                .find(|h| line >= h.start && line < h.end)
            {
                Some(header) => self.header_end_row(*header),
                None => row.saturating_add(WrappedRow(1)),
            };
            if end <= row {
                // No progress (empty span); avoid a spin.
                row = row.saturating_add(WrappedRow(1));
            } else {
                row = end;
            }
        }
        row
    }

    /// Move forward by `steps` non-header rows from `start`.
    /// `start` should already be a body row (or a preamble row).
    fn step_content_row(&self, start: WrappedRow, steps: u32) -> WrappedRow {
        let total = self.lines.total();
        let mut row = start;
        for _ in 0..steps {
            if row.as_usize() + 1 >= total {
                // Past the last row: stay, clamp will hold us.
                return self.skip_header_rows(row.saturating_add(WrappedRow(1)));
            }
            row = row.saturating_add(WrappedRow(1));
            row = self.skip_header_rows(row);
            if row.as_usize() >= total {
                return row;
            }
        }
        row
    }

    /// Previous non-header row strictly before `start`.
    /// `start` is a body row. If there is none, returns `start` when
    /// `start` is already 0, otherwise the earliest row.
    fn step_content_row_back(&self, start: WrappedRow) -> WrappedRow {
        match self.step_content_row_back_from_header(start) {
            Some(prev) => prev,
            None => start,
        }
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
        match self.file_header_at_row(content_row) {
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
        if row < layout.content_start || self.row_is_in_header(row) {
            return false;
        }
        // Visible body rows are the next `content_rows` non-header rows.
        let mut cursor = layout.content_start;
        let mut left = layout.content_rows;
        let total = self.lines.total();
        while left > 0 && cursor.as_usize() < total {
            if self.row_is_in_header(cursor) {
                cursor = self.skip_header_rows(cursor);
                continue;
            }
            if cursor == row {
                return true;
            }
            if cursor > row {
                return false;
            }
            cursor = cursor.saturating_add(WrappedRow(1));
            left = left.saturating_sub(1);
        }
        false
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
        let mut content = self.collect_content_rows(layout.content_start, layout.content_rows);
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

    /// Flat visual-row cursor after taking `max_rows` non-header rows
    /// from `start`. Used for the viewport ruler.
    fn flat_end_after_content(&self, start: WrappedRow, max_rows: u32) -> usize {
        let total = self.lines.total();
        let mut cursor = start;
        let mut remaining = max_rows;
        while remaining > 0 && cursor.as_usize() < total {
            if self.row_is_in_header(cursor) {
                cursor = self.skip_header_rows(cursor);
                continue;
            }
            cursor = cursor.saturating_add(WrappedRow(1));
            remaining = remaining.saturating_sub(1);
        }
        // If the window ran off the end of the buffer, report `total` so
        // the ruler reads Bot.
        if remaining > 0 {
            total
        } else {
            cmp::min(cursor.as_usize(), total)
        }
    }

    /// Up to `max_rows` non-header visual rows starting at `start`.
    /// Header rows in the way are skipped so they are not drawn twice
    /// (once in the pin, once in the body).
    fn collect_content_rows(&self, start: WrappedRow, max_rows: u32) -> Vec<VisualRow> {
        let mut rows = Vec::new();
        if max_rows == 0 {
            return rows;
        }
        let total = self.lines.total();
        let mut cursor = start;
        let mut remaining = max_rows;
        while remaining > 0 && cursor.as_usize() < total {
            if self.row_is_in_header(cursor) {
                cursor = self.skip_header_rows(cursor);
                continue;
            }
            let line = line_at_visual_row(self.lines.row_counts(), cursor);
            let line_start = visual_row_of_line(self.lines.row_counts(), line);
            let line_rows = self.lines.row_counts()[line];
            let line_end = line_start.saturating_add(line_rows);
            // Rows of this line from `cursor` to the end of the line,
            // capped by `remaining`.
            let available = line_end.saturating_sub(cursor);
            let take = cmp::min(available, WrappedRow(remaining));
            if take == WrappedRow(0) {
                cursor = cursor.saturating_add(WrappedRow(1));
                continue;
            }
            let part_end = cursor.saturating_add(take);
            let part = visual_rows_in_range(
                &self.lines,
                self.highlight.as_ref(),
                self.terminal_bg,
                cursor,
                part_end,
            );
            let took = u32::try_from(part.len()).unwrap_or(remaining);
            remaining = remaining.saturating_sub(took);
            rows.extend(part);
            cursor = part_end;
        }
        rows
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

struct VisualRow {
    pieces: Vec<Piece>,
    width: Width,
    /// True when this row continues the same logical line after a width wrap.
    soft_wrap: bool,
}

struct Piece {
    sgr: String,
    text: String,
}

fn row_fingerprint(row: &VisualRow) -> String {
    let mut s = String::new();
    for piece in &row.pieces {
        s.push_str(&piece.sgr);
        s.push_str(&piece.text);
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

/// Nvim-style ruler for the viewport: `Top`, `Bot`, or `NN%`.
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
    format!("{pct}%")
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

fn write_one_line(
    rows: &[VisualRow],
    terminal_width: Width,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut last_sgr: Option<String> = None;
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            let prev = &rows[i - 1];
            let soft = row.soft_wrap && prev.width == terminal_width;
            if !soft {
                out.write_all(b"\r\n")?;
            }
        }
        for piece in &row.pieces {
            if last_sgr.as_deref() != Some(piece.sgr.as_str()) {
                out.write_all(piece.sgr.as_bytes())?;
                last_sgr = Some(piece.sgr.clone());
            }
            out.write_all(piece.text.as_bytes())?;
        }
    }
    Ok(())
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
                });
                pieces = Vec::new();
                col = Width(0);
                soft_wrap = true;
            }
            match pieces.last_mut() {
                Some(last) if last.sgr == *sgr => {
                    last.text.push_str(text);
                }
                _ => {
                    let mut owned = String::new();
                    owned.push_str(text);
                    pieces.push(Piece {
                        sgr: sgr.clone(),
                        text: owned,
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

#[cfg(test)]
mod tests {
    use std::cmp;
    use std::num::NonZeroU16;

    use crate::height::Height;
    use crate::lines_with_wrapped_row_count::LinesWithWrappedRowCount;
    use crate::lines_with_wrapped_row_count::line_visual_row_count;
    use crate::render::Renderer;
    use crate::render::line_visual_rows;
    use crate::render::row_fingerprint;
    use crate::render::visual_rows_in_range;
    use crate::size::Size;
    use crate::terminal::color::TerminalColor;
    use crate::terminal::parse::parse_command_output;
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
        renderer.set_lines(parse_command_output(input));
        for _ in 0..scroll_downs {
            renderer.scroll_down();
        }
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
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
        renderer.set_lines(parse_command_output(b"0123456789abcdefghij"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
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
        renderer.set_lines(parse_command_output(b"a\nb\nc\nd\n"));
        renderer.scroll_bottom();
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        let mut parser = vt_parser(3, 10);
        parser.process(&buf);
        assert_eq!("c", row_text(&parser, 0, 10));
        assert_eq!("d", row_text(&parser, 1, 10));
        assert_eq!("       Bot", row_text(&parser, 2, 10));
        renderer.scroll_top();
        buf.clear();
        renderer.draw(&mut buf).unwrap();
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
        // Width 5 wraps "0123456789abcdefghij" into 01234/56789/abcde/fghij.
        renderer.set_lines(parse_command_output(b"0123456789abcdefghij\nWXYZ\n"));
        renderer.scroll_down();
        renderer.scroll_down();
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        let mut parser = vt_parser(3, 5);
        parser.process(&buf);
        assert_eq!("abcde", row_text(&parser, 0, 5));
        // Width 8 wraps that line into 01234567/89abcdef/ghij. Byte 10
        // ('a') sits on "89abcdef". A stale wrapped-row index of 2 would
        // show "ghij" instead.
        buf.clear();
        renderer.update_size(size(8, 3));
        renderer.draw(&mut buf).unwrap();
        let mut parser = vt_parser(3, 8);
        parser.process(&buf);
        assert_eq!("89abcdef", row_text(&parser, 0, 8));
    }

    #[test]
    fn test_scroll_page_down_and_up() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        renderer.set_lines(parse_command_output(b"a\nb\nc\nd\ne\nf\n"));
        renderer.scroll_page_down();
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        let mut parser = vt_parser(3, 10);
        parser.process(&buf);
        assert_eq!("c", row_text(&parser, 0, 10));
        assert_eq!("d", row_text(&parser, 1, 10));
        assert_eq!("       66%", row_text(&parser, 2, 10));
        renderer.scroll_page_up();
        buf.clear();
        renderer.draw(&mut buf).unwrap();
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
        renderer.set_lines(parse_command_output(b"a\nb\nc\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        assert_eq!(false, buf.is_empty());
        buf.clear();
        renderer.draw(&mut buf).unwrap();
        assert_eq!(true, buf.is_empty());
    }

    #[test]
    fn test_draw_skips_when_change_is_below_visible_area() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        renderer.set_lines(parse_command_output(b"a\nb\nc\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        buf.clear();
        renderer.set_lines(parse_command_output(b"a\nb\nX\n"));
        renderer.draw(&mut buf).unwrap();
        assert_eq!(true, buf.is_empty());
    }

    #[test]
    fn test_draw_writes_when_visible_area_changes() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        renderer.set_lines(parse_command_output(b"a\nb\nc\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        buf.clear();
        renderer.set_lines(parse_command_output(b"a\nB\nc\n"));
        renderer.draw(&mut buf).unwrap();
        assert_eq!(false, buf.is_empty());
    }

    #[test]
    fn test_redraw_writes_when_visible_unchanged() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 2));
        renderer.set_lines(parse_command_output(b"a\nb\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        buf.clear();
        renderer.redraw(&mut buf).unwrap();
        assert_eq!(false, buf.is_empty());
    }

    #[test]
    fn test_search_highlight_sets_bg() {
        let size = size(20, 3);
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        renderer.set_lines(parse_command_output(b"xxFOOyy\na\x1b[31mFOO\x1b[0mb\n"));
        renderer.set_highlight(Some("FOO".to_owned()), Some((0, 2)), false);
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
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
    fn test_status_bar_light_grey_on_light_bg() {
        let mut renderer = Renderer::new(TerminalBg::Light, size(10, 2));
        renderer.set_lines(parse_command_output(b"a\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
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
        renderer.set_lines(parse_command_output(b"a\nb\nc\nd\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        assert_eq!(true, buf.windows(4).any(|w| w == b"\x1b[2J"));
        buf.clear();
        renderer.set_prompt_status("/hi".to_owned(), 3);
        renderer.draw(&mut buf).unwrap();
        assert_eq!(false, buf.is_empty());
        assert_eq!(false, buf.windows(4).any(|w| w == b"\x1b[2J"));
        assert_eq!(true, buf.windows(3).any(|w| w == b"/hi"));
    }

    #[test]
    fn test_draw_writes_when_end_indicator_appears() {
        let mut renderer = Renderer::new(TerminalBg::Dark, size(10, 3));
        renderer.set_lines(parse_command_output(b"a\nb\nc\n"));
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        buf.clear();
        renderer.set_lines(parse_command_output(b"a\nb\n"));
        renderer.draw(&mut buf).unwrap();
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
        renderer.set_diff(crate::diff::parse_diff(input));
        for _ in 0..scroll_downs {
            renderer.scroll_down();
        }
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        let mut parser = vt_parser(height, width);
        parser.process(&buf);
        parser
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
        let parser = draw_diff(input.as_bytes(), 40, 8, 21);
        assert_eq!("bar", row_text(&parser, 0, 40));
        assert_eq!("@@ -1,20 +1,20 @@", row_text(&parser, 1, 40));
        let parser = draw_diff(input.as_bytes(), 40, 8, 23);
        assert_eq!("bar", row_text(&parser, 0, 40));
        assert_eq!(" line1", row_text(&parser, 1, 40));
        assert_eq!(false, row_text(&parser, 0, 40).contains("foo"));
    }

    #[test]
    fn test_scroll_bottom_keeps_last_line_and_file_header() {
        let mut input = context_file_diff("foo", 20);
        input.push_str(&context_file_diff("bar", 20));
        let mut renderer = Renderer::new(TerminalBg::Dark, size(40, 8));
        renderer.set_diff(crate::diff::parse_diff(input.as_bytes()));
        renderer.scroll_bottom();
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
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
        renderer.set_diff(crate::diff::parse_diff(text.as_bytes()));
        renderer.scroll_bottom();
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
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
        renderer.set_lines(parse_command_output(b"a\nb\nc\nd\ne\nf\n"));
        renderer.scroll_bottom();
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        let mut parser = vt_parser(3, 10);
        parser.process(&buf);
        assert_eq!("e", row_text(&parser, 0, 10));
        assert_eq!("f", row_text(&parser, 1, 10));
        assert_eq!("       Bot", row_text(&parser, 2, 10));
    }
}
