use crate::terminal::color::TerminalColor;
use crate::terminal::style::Style;
use crate::terminal_bg::TerminalBg;

/// Neovim `NvimLightYellow`.
pub(crate) const NVIM_LIGHT_YELLOW: TerminalColor = TerminalColor::Rgb(0xfc, 0xe0, 0x94);
/// Neovim `NvimDarkYellow`.
pub(crate) const NVIM_DARK_YELLOW: TerminalColor = TerminalColor::Rgb(0x6b, 0x53, 0x00);
/// Neovim `NvimLightGrey1`.
pub(crate) const NVIM_LIGHT_GREY1: TerminalColor = TerminalColor::Rgb(0xee, 0xf1, 0xf8);
/// Neovim `NvimDarkGrey1`.
pub(crate) const NVIM_DARK_GREY1: TerminalColor = TerminalColor::Rgb(0x07, 0x08, 0x0d);

/// Neovim Search / CurSearch: two yellow shades. The current hit uses the
/// shade that contrasts with the terminal background.
pub(crate) fn search_highlight(base: Style, terminal_bg: TerminalBg, current: bool) -> Style {
    // Light yellow background when it contrasts with the terminal:
    // current on dark, or a non-current match on light.
    let light_yellow_bg = match (terminal_bg, current) {
        (TerminalBg::Dark, true) | (TerminalBg::Light, false) => true,
        (TerminalBg::Dark, false) | (TerminalBg::Light, true) => false,
    };
    let mut style = base;
    if light_yellow_bg {
        style.fg = Some(NVIM_DARK_GREY1);
        style.bg = Some(NVIM_LIGHT_YELLOW);
    } else {
        style.fg = Some(NVIM_LIGHT_GREY1);
        style.bg = Some(NVIM_DARK_YELLOW);
    }
    style.reverse = false;
    style
}

#[cfg(test)]
mod tests {
    use crate::terminal::style::Style;
    use crate::terminal_bg::TerminalBg;
    use crate::theme::NVIM_DARK_YELLOW;
    use crate::theme::NVIM_LIGHT_YELLOW;
    use crate::theme::search_highlight;

    #[test]
    fn test_search_highlight_is_two_yellow_shades() {
        let plain = Style::plain();
        let dark_other = search_highlight(plain, TerminalBg::Dark, false);
        let dark_current = search_highlight(plain, TerminalBg::Dark, true);
        assert_eq!(Some(NVIM_DARK_YELLOW), dark_other.bg);
        assert_eq!(Some(NVIM_LIGHT_YELLOW), dark_current.bg);
        let light_other = search_highlight(plain, TerminalBg::Light, false);
        let light_current = search_highlight(plain, TerminalBg::Light, true);
        assert_eq!(Some(NVIM_LIGHT_YELLOW), light_other.bg);
        assert_eq!(Some(NVIM_DARK_YELLOW), light_current.bg);
    }
}
