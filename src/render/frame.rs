use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;

use crate::render::cursor::FrameCursor;

/// A finished terminal frame: contents plus the cursor that follows.
///
/// The presenter moves to (0, 0) and clears before writing `bytes`.
/// The cursor is applied after the contents.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct Frame {
    pub(crate) bytes: Vec<u8>,
    pub(crate) cursor: FrameCursor,
}

/// Stateful frame output. Keeps the previous frame and skips the
/// terminal write when the new frame equals it. The cursor is written
/// after the frame contents.
pub(crate) struct FrameRenderer {
    prev: Option<Frame>,
}

impl FrameRenderer {
    pub(crate) fn new() -> FrameRenderer {
        FrameRenderer { prev: None }
    }

    pub(crate) fn render(&mut self, out: &mut impl Write, frame: Frame) -> io::Result<()> {
        match &self.prev {
            Some(prev) if *prev == frame => {
                return Ok(());
            }
            Some(prev) if prev.bytes == frame.bytes => {
                frame.cursor.write(out)?;
                out.flush()?;
            }
            _ => {
                // Home the cursor and clear, then paint from (0, 0).
                queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
                out.write_all(&frame.bytes)?;
                // Cursor is always the last command.
                frame.cursor.write(out)?;
                out.flush()?;
            }
        }
        self.prev = Some(frame);
        Ok(())
    }
}
