use std::convert::TryFrom;
use std::num::TryFromIntError;

/// Terminal columns. Not a byte or character count; a wide glyph counts as two.
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
pub(crate) struct Width(pub u32);

impl Width {
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn checked_sub(self, other: Width) -> Option<Width> {
        self.0.checked_sub(other.0).map(Width)
    }

    #[expect(dead_code)]
    pub(crate) fn saturating_sub(self, other: Width) -> Width {
        Width(self.0.saturating_sub(other.0))
    }
}

impl TryFrom<usize> for Width {
    type Error = TryFromIntError;

    fn try_from(w: usize) -> Result<Width, TryFromIntError> {
        u32::try_from(w).map(Width)
    }
}

impl PartialEq<u32> for Width {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}
