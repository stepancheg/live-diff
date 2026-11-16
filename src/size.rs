use crate::height::Height;
use crate::terminal::width::Width;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Size {
    pub(crate) width: Width,
    pub(crate) height: Height,
}
