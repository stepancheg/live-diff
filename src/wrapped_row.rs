use std::convert::TryFrom;
use std::num::TryFromIntError;

/// 0-based index of a row after wrapping, as drawn on the terminal.
///
/// Not the index of the original `\n`-separated line. A line wider
/// than the terminal becomes several of these rows. Example: with a
/// 3-column terminal, "abcd" and "Z" are rows 0 ("abc"), 1 ("d"),
/// and 2 ("Z"). The original line index is 0 for "abcd" and 1 for "Z".
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    derive_more::Display,
    derive_more::Add,
    derive_more::Sub,
    derive_more::AddAssign,
    derive_more::SubAssign,
    derive_more::Sum,
)]
pub(crate) struct WrappedRow(pub(crate) u32);

impl WrappedRow {
    pub(crate) const MAX: WrappedRow = WrappedRow(u32::MAX);

    pub(crate) fn saturating_add(self, other: WrappedRow) -> WrappedRow {
        WrappedRow(self.0.saturating_add(other.0))
    }

    pub(crate) fn saturating_sub(self, other: WrappedRow) -> WrappedRow {
        WrappedRow(self.0.saturating_sub(other.0))
    }

    pub(crate) fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl TryFrom<usize> for WrappedRow {
    type Error = TryFromIntError;

    fn try_from(row: usize) -> Result<WrappedRow, TryFromIntError> {
        u32::try_from(row).map(WrappedRow)
    }
}
