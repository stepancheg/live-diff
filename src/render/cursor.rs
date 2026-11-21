use std::io;
use std::io::Write;

use crossterm::cursor::Hide;
use crossterm::cursor::MoveTo;
use crossterm::cursor::Show;
use crossterm::queue;

/// Where the terminal cursor sits after the frame is painted.
/// Always applied after the frame contents.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum FrameCursor {
    Hidden,
    /// 0-based column and row.
    At {
        col: u16,
        row: u16,
    },
}

impl FrameCursor {
    pub(crate) fn write(self, out: &mut impl Write) -> io::Result<()> {
        match self {
            FrameCursor::Hidden => {
                queue!(out, Hide)?;
            }
            FrameCursor::At { col, row } => {
                queue!(out, MoveTo(col, row), Show)?;
            }
        }
        Ok(())
    }
}
