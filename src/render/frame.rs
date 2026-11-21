use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;

/// A finished terminal frame: the bytes to write to the screen.
///
/// The presenter moves to (0, 0) and clears before writing these
/// bytes. They are the frame contents only.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct Frame {
    pub(crate) bytes: Vec<u8>,
}

/// Stateful frame output. Keeps the previous frame and skips the
/// terminal write when the new frame equals it.
pub(crate) struct FrameRenderer {
    prev: Option<Frame>,
}

impl FrameRenderer {
    pub(crate) fn new() -> FrameRenderer {
        FrameRenderer { prev: None }
    }

    pub(crate) fn render(&mut self, out: &mut impl Write, frame: Frame) -> io::Result<()> {
        if self.prev.as_ref() == Some(&frame) {
            return Ok(());
        }
        // Home the cursor and clear, then paint the frame from (0, 0).
        queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
        out.write_all(&frame.bytes)?;
        out.flush()?;
        self.prev = Some(frame);
        Ok(())
    }
}
