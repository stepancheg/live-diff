//! Generic terminal code only. App-specific styles and behavior live elsewhere.

pub(crate) mod color;
pub(crate) mod line;
pub(crate) mod parse;
mod span;
pub(crate) mod style;
mod terminal_char;
mod terminal_str;
pub(crate) mod width;
