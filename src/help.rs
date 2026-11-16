use crate::terminal::line::Line;
use crate::terminal::parse::parse_command_output;

const HELP: &str = "\
Keys

  h              Show this help
  q              Quit
  r              Run the command now (kills the current run)
  j, Down, Enter Scroll down
  k, Up          Scroll up
  Space, PgDn    Page down
  PgUp           Page up
  g, Home        Top
  G, End         Bottom
  /              Regex search forward from the top of the screen
  ?              Regex search backward from the bottom of the screen
  n              Repeat search
  N              Repeat search in reverse
  mx             Mark current position as x
  'x             Go to mark x
  -i             Toggle ignore case
  -I             Always ignore case
  ^l             Redraw

q or h closes this page.
In the prompt, Enter searches. Up and Down recall previous searches.
Esc cancels the prompt or clears Pattern not found.
";

pub(crate) fn help_lines() -> Vec<Line> {
    parse_command_output(HELP.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use crate::height::Height;
    use crate::help::help_lines;
    use crate::render::Renderer;
    use crate::size::Size;
    use crate::terminal::width::Width;
    use crate::terminal_bg::TerminalBg;

    fn vt_parser(rows: u16, cols: u16) -> vt100::Parser {
        vt100::Parser::new(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(),
            0,
        )
    }

    #[test]
    fn test_help_lists_keys() {
        let text: String = help_lines()
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref().as_str())
                    .collect::<String>()
            })
            .collect::<Vec<String>>()
            .join("\n");
        assert_eq!(true, text.contains("h              Show this help"));
        assert_eq!(
            true,
            text.contains("r              Run the command now (kills the current run)")
        );
        assert_eq!(true, text.contains("q              Quit"));
        assert_eq!(true, text.contains("q or h closes this page."));
        assert_eq!(
            true,
            text.contains("/              Regex search forward from the top of the screen")
        );
        assert_eq!(
            true,
            text.contains("?              Regex search backward from the bottom of the screen")
        );
        assert_eq!(true, text.contains("g, Home        Top"));
        assert_eq!(true, text.contains("G, End         Bottom"));
        assert_eq!(true, text.contains("n              Repeat search"));
        assert_eq!(
            true,
            text.contains("mx             Mark current position as x")
        );
        assert_eq!(true, text.contains("'x             Go to mark x"));
        assert_eq!(true, text.contains("Up and Down recall previous searches."));
    }

    #[test]
    fn test_help_draws_key_list() {
        let size = Size {
            width: Width(80),
            height: Height(30),
        };
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        renderer.set_lines(help_lines());
        let mut buf = Vec::new();
        renderer.draw(&mut buf).unwrap();
        let mut parser = vt_parser(30, 80);
        parser.process(&buf);
        assert_eq!(true, parser.screen().contents().contains("Show this help"));
        let mut ruler = String::new();
        for col in 77..80 {
            let cell = parser.screen().cell(29, col).unwrap();
            if cell.has_contents() {
                ruler.push_str(cell.contents());
            }
        }
        assert_eq!("Bot", ruler);
    }
}
