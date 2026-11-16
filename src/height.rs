use std::convert::TryFrom;
use std::num::TryFromIntError;

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
pub(crate) struct Height(pub u32);

impl TryFrom<usize> for Height {
    type Error = TryFromIntError;

    fn try_from(h: usize) -> Result<Height, TryFromIntError> {
        u32::try_from(h).map(Height)
    }
}
