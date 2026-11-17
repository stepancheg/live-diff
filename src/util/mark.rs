use std::collections::HashMap;

use crate::render::LinePos;

pub(crate) const NOT_SET: &str = "Mark not set";

#[derive(Clone, Copy)]
pub(crate) enum MarkOp {
    Set,
    Goto,
}

impl MarkOp {
    pub(crate) fn prompt_label(self) -> &'static str {
        match self {
            MarkOp::Set => "m",
            MarkOp::Goto => "'",
        }
    }
}

#[derive(Default)]
pub(crate) struct Marks {
    /// Mark name to the top viewport position in original-line coordinates.
    pub(crate) rows: HashMap<char, LinePos>,
}
