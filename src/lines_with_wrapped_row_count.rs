use crate::terminal::line::Line;
use crate::terminal::width::Width;
use crate::wrapped_row::WrappedRow;

/// Original lines and the wrapped-row count of each line at one width.
///
/// Counts are computed once for `width` and reused by scroll, the ruler,
/// and draw. Call [`LinesWithWrappedRowCount::update_width`] when the width
/// changes; replace the value when the lines change.
pub(crate) struct LinesWithWrappedRowCount {
    lines: Vec<Line>,
    /// Width `row_counts` and `total` were computed for.
    width: Width,
    /// Wrapped rows occupied by the line at the same index.
    row_counts: Vec<WrappedRow>,
    /// Sum of `row_counts`.
    total: usize,
}

impl LinesWithWrappedRowCount {
    pub(crate) fn new(lines: Vec<Line>, width: Width) -> LinesWithWrappedRowCount {
        let (row_counts, total) = count_wrapped_rows(&lines, width);
        LinesWithWrappedRowCount {
            lines,
            width,
            row_counts,
            total,
        }
    }

    /// Recompute counts when `width` differs from the width they were built for.
    pub(crate) fn update_width(&mut self, width: Width) {
        if self.width == width {
            return;
        }
        let (row_counts, total) = count_wrapped_rows(&self.lines, width);
        self.width = width;
        self.row_counts = row_counts;
        self.total = total;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&Line> {
        self.lines.get(index)
    }

    pub(crate) fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub(crate) fn width(&self) -> Width {
        self.width
    }

    pub(crate) fn row_counts(&self) -> &[WrappedRow] {
        &self.row_counts
    }

    pub(crate) fn total(&self) -> usize {
        self.total
    }
}

fn count_wrapped_rows(lines: &[Line], width: Width) -> (Vec<WrappedRow>, usize) {
    let mut row_counts = Vec::with_capacity(lines.len());
    let mut total = 0usize;
    for line in lines {
        let n = line_visual_row_count(line, width);
        row_counts.push(n);
        total = total.saturating_add(n.as_usize());
    }
    (row_counts, total)
}

/// Wrapped rows `line` occupies at `terminal_width`.
/// Matches building the line's visual rows and taking their length,
/// without allocating those rows.
/// A line with no placeable glyph still occupies one row, unless the
/// terminal width is 0.
pub(crate) fn line_visual_row_count(line: &Line, terminal_width: Width) -> WrappedRow {
    if terminal_width == 0 {
        return WrappedRow(0);
    }
    let mut col = Width(0);
    let mut rows = WrappedRow(1);
    for span in &line.spans {
        for ch in span.content.as_ref().chars() {
            let w = ch.width();
            if w > terminal_width {
                continue;
            }
            if col + w > terminal_width {
                rows = rows.saturating_add(WrappedRow(1));
                col = Width(0);
            }
            col += w;
        }
    }
    rows
}
