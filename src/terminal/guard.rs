use std::io;
use std::io::Write;

use crossterm::cursor::Hide;
use crossterm::cursor::Show;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::EnableBracketedPaste;
use crossterm::queue;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;

/// Enters raw mode and the alternate screen; restores both on drop.
pub(crate) struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    pub(crate) fn enter_alt_mode() -> io::Result<TerminalGuard> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        queue!(stdout, EnterAlternateScreen, Hide, EnableBracketedPaste)?;
        stdout.flush()?;
        Ok(TerminalGuard { active: true })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        restore_terminal();
    }
}

/// Leave the alternate screen and disable raw mode.
///
/// Used by `Drop` and by the panic hook (`panic = "abort"` skips `Drop`).
pub(crate) fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = queue!(stdout, DisableBracketedPaste, LeaveAlternateScreen, Show);
    let _ = stdout.flush();
    let _ = disable_raw_mode();
}
