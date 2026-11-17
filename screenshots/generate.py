#!/usr/bin/env python3
"""Render sl-tui screenshots with tmux and textimg.

Writes:
  screenshots/diff.png       default diff view
  screenshots/diffstat.png   diff-stat view (key s)
  screenshots/smartlog.png   smartlog view (key t)

Pass `smartlog` as the only argument to render just that shot.

Needs tmux, dotslash, and a debug build of sl-tui (cargo build).
screenshots/textimg is a dotslash for textimg. The diff is a fixed
fixture in a temporary git repository, not the sl-tui working tree.
The pane is captured at a fixed size and each row is padded to that
width. SGR state is replayed only onto soft-wrapped continuations.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

COLS = 53
ROWS = 12
# Scroll to the end of the first file so the pane shows its tail
# (top of that diff off-screen) and the next file's diff. The ruler
# is a percentage, not Top/Bot.
SCROLL_LINES = 85
SESSION_PREFIX = "sl-tui-shot"


ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = Path(__file__).resolve().parent
BINARY = ROOT / "target" / "debug" / "sl-tui"
TEXTIMG = OUT_DIR / "textimg"
# textimg cells are fontSize/2 by fontSize*1.1. Stock Monaco's advance
# is 1229/2048 em (0.6), so its outlines overlap that grid. We rewrite
# unitsPerEm to 2458 (= 1229*2) so the advance is exactly half an em.
MONACO_SRC = Path("/System/Library/Fonts/Monaco.ttf")
# Menlo has the arc box-drawing corners (╭╮╰╯) that Monaco lacks.
# Smartlog shots use Menlo so those glyphs are not .notdef boxes.
MENLO_SRC = Path("/System/Library/Fonts/Menlo.ttc")
# Larger than 14 so the PNG is readable. Spacing ratio does not depend
# on this value.
FONT_SIZE = 28

# CSI SGR only. Other CSI (cursor motion) is dropped if present.
SGR_RE = re.compile(r"\x1b\[([0-9;]*)m")
CSI_OTHER_RE = re.compile(r"\x1b\[[0-9;:?]*[ -/]*[@-~]")
# OSC 8 hyperlinks (and other OSC) are not painted by textimg.
OSC_RE = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(1)


def require_tool(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        die(f"{name} not found on PATH")
    return path


def run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        check=True,
        text=True,
        **kwargs,  # type: ignore[arg-type]
    )


def build_binary() -> None:
    print(f"building {BINARY.relative_to(ROOT)}")
    run(["cargo", "build", "--quiet"], cwd=ROOT)
    if not BINARY.is_file():
        die(f"missing binary {BINARY}")


def make_fixture(repo: Path, real_git: str) -> None:
    repo.mkdir(parents=True)
    env = os.environ.copy()

    def git(*args: str) -> None:
        run(
            [
                real_git,
                "-c",
                "init.defaultBranch=master",
                "-C",
                str(repo),
                *args,
            ],
            env=env,
        )

    git("init", "-q")
    src = repo / "src"
    src.mkdir()
    (repo / "lib.rs").write_text(lib_rs_source(changed=False))
    (src / "main.rs").write_text('fn main() {\n    println!("hello");\n}\n')
    git("add", "lib.rs", "src/main.rs")
    git("commit", "-q", "-m", "init")
    # Long first-file diff so a 12-row pane cannot show it all.
    (repo / "lib.rs").write_text(lib_rs_source(changed=True))
    (src / "main.rs").write_text(
        "fn main() {\n"
        '    let row = format_row("src/host.rs", 2, 1, 40);\n'
        '    println!("{row}");\n'
        "}\n"
    )
    # Untracked, so the status bar count and the stat list both show it.
    (repo / "notes.txt").write_text("scratch\n")


def lib_rs_source(changed: bool) -> str:
    """lib.rs before and after a left-ellipsis path fit.

    The changed form keeps component boundaries and measures chars,
    not bytes. Both versions are long enough that the hunk does not
    fit in the screenshot pane. Every source line is short enough
    that a diff marker plus the text fits in COLS (no wrap).
    """
    # Longest body line below is 46 chars; with a '-'/'+'/' ' marker
    # that is 47, inside COLS=53.
    if not changed:
        return """\
pub fn fit_name(s: &str, w: usize) -> String {
    let n = s.len();
    if n <= w {
        return s.to_owned();
    }
    if w == 0 {
        return String::new();
    }
    // No room for "..."; keep a raw suffix.
    if w < 3 {
        return s[n - w..].to_owned();
    }
    let keep = w - 3;
    let start = n - keep;
    // Byte slice. May split a multibyte char.
    let mut out = String::from("...");
    out.push_str(&s[start..]);
    out
}

pub fn change_bar(
    add: u32,
    del: u32,
    w: usize,
) -> String {
    if w == 0 {
        return String::new();
    }
    let total = add + del;
    if total == 0 {
        return " ".repeat(w);
    }
    // Fill in proportion to the larger count.
    let mut out = String::new();
    let plus = (add as usize * w) / total as usize;
    let minus = w - plus;
    for _ in 0..plus {
        out.push('+');
    }
    for _ in 0..minus {
        out.push('-');
    }
    out
}

pub fn format_row(
    path: &str,
    add: u32,
    del: u32,
    w: usize,
) -> String {
    let name_w = w.saturating_sub(12);
    let name = fit_name(path, name_w);
    let bar = change_bar(add, del, 8);
    format!("{name} | {add} {del} {bar}")
}
"""
    return """\
pub fn fit_name(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        return s.to_owned();
    }
    if w == 0 {
        return String::new();
    }
    // Prefer "..." plus a suffix cut at '/'.
    if w < 4 {
        let skip = n - w;
        return s.chars().skip(skip).collect();
    }
    let keep = w - 3;
    let skip = n - keep;
    let suffix: String =
        s.chars().skip(skip).collect();
    // Cut at '/' so a component stays intact.
    let aligned = match suffix.find('/') {
        Some(0) => suffix,
        Some(i) => suffix[i..].to_owned(),
        None => suffix,
    };
    let mut out = String::from("...");
    out.push_str(&aligned);
    // Pad if the slash cut shortened the name.
    while out.chars().count() < w {
        out.push(' ');
    }
    out
}

pub fn change_bar(
    add: u32,
    del: u32,
    w: usize,
) -> String {
    if w == 0 {
        return String::new();
    }
    let total = add + del;
    if total == 0 {
        return " ".repeat(w);
    }
    // Give each non-zero side at least one mark.
    let mut plus = if add == 0 {
        0
    } else {
        ((add as usize * w) / total as usize).max(1)
    };
    let mut minus = if del == 0 {
        0
    } else {
        ((del as usize * w) / total as usize).max(1)
    };
    // max(1) on both sides can exceed w; trim.
    while plus + minus > w {
        if plus >= minus {
            plus -= 1;
        } else {
            minus -= 1;
        }
    }
    let mut out = String::new();
    for _ in 0..plus {
        out.push('+');
    }
    for _ in 0..minus {
        out.push('-');
    }
    while out.len() < w {
        out.push(' ');
    }
    out
}

pub fn format_row(
    path: &str,
    add: u32,
    del: u32,
    w: usize,
) -> String {
    let name_w = w.saturating_sub(14);
    let name = fit_name(path, name_w);
    let bar = change_bar(add, del, 10);
    format!("{name} | +{add} -{del} {bar}")
}
"""


def apply_sgr(state: dict[str, object], params: list[str]) -> None:
    if not params or params == [""]:
        params = ["0"]
    i = 0
    while i < len(params):
        p = params[i]
        if p in ("", "0"):
            state["bold"] = False
            state["dim"] = False
            state["fg"] = None
            state["bg"] = None
        elif p == "1":
            state["bold"] = True
        elif p == "2":
            state["dim"] = True
        elif p == "22":
            state["bold"] = False
            state["dim"] = False
        elif p == "39":
            state["fg"] = None
        elif p == "49":
            state["bg"] = None
        elif p in ("30", "31", "32", "33", "34", "35", "36", "37"):
            state["fg"] = p
        elif p == "38" and i + 1 < len(params) and params[i + 1] == "5":
            if i + 2 < len(params):
                state["fg"] = f"38;5;{params[i + 2]}"
                i += 2
        elif p == "38" and i + 1 < len(params) and params[i + 1] == "2":
            if i + 4 < len(params):
                state["fg"] = "38;2;" + ";".join(params[i + 2 : i + 5])
                i += 4
        elif p == "48" and i + 1 < len(params) and params[i + 1] == "5":
            if i + 2 < len(params):
                state["bg"] = f"48;5;{params[i + 2]}"
                i += 2
        elif p == "48" and i + 1 < len(params) and params[i + 1] == "2":
            if i + 4 < len(params):
                state["bg"] = "48;2;" + ";".join(params[i + 2 : i + 5])
                i += 4
        i += 1


def sgr_for(state: dict[str, object]) -> str:
    """Emit a style textimg will actually paint.

    textimg ignores SGR 1 and SGR 2. Bold is mapped to the bright
    color (default foreground -> 97; 30-37 -> 90-97). Dim default
    foreground becomes a darker gray. An explicit foreground is also
    set when a background is present so the glyph does not pick up
    the dimmer default -g.
    """
    parts = ["0"]
    fg = state.get("fg")
    bg = state.get("bg")
    bold = bool(state.get("bold"))
    dim = bool(state.get("dim"))
    if bold:
        if fg is None:
            parts.append("97")
        elif fg.isdigit() and 30 <= int(fg) <= 37:
            parts.append(str(int(fg) + 60))
        else:
            parts.append(str(fg))
    elif dim:
        if fg is None:
            parts.append("38;2;128;128;128")
        else:
            parts.append(str(fg))
    elif fg is None:
        if bg is not None:
            # Status bar and selection: light text on the gray bg.
            parts.append("97")
    else:
        parts.append(str(fg))
    if isinstance(bg, str):
        parts.append(bg)
    return "\x1b[" + ";".join(parts) + "m"


def visible_len(s: str) -> int:
    return len(SGR_RE.sub("", CSI_OTHER_RE.sub("", s)))


def normalize_capture(raw: str, width: int) -> str:
    """One terminal row per line, each `width` cells, SGR carried over.

    tmux capture-pane does not repeat the current style at the start of
    a wrapped row, and it may omit trailing spaces. A line-oriented
    renderer then loses color on the continuation and sizes the image
    to the longest row.

    Carry style only onto a row that starts with text (a soft-wrapped
    continuation). Do not prefix a carried background onto a row that
    already begins with SGR: that run has no text. Empty rows are the
    terminal default, not the previous line's background.
    """
    lines = raw.split("\n")
    if lines and lines[-1] == "":
        lines = lines[:-1]
    state: dict[str, object] = {
        "bold": False,
        "dim": False,
        "fg": None,
        "bg": None,
    }
    out: list[str] = []
    blank = "\x1b[0m" + (" " * width)
    for line in lines:
        # Drop OSC (hyperlinks) and non-SGR CSI so textimg only sees
        # text and colors.
        line = OSC_RE.sub("", line)
        line = CSI_OTHER_RE.sub(
            lambda m: m.group(0) if m.group(0).endswith("m") else "",
            line,
        )
        if visible_len(line) == 0:
            out.append(blank)
            continue
        buf: list[str] = []
        if not line.startswith("\x1b["):
            buf.append(sgr_for(state))
        pos = 0
        for m in SGR_RE.finditer(line):
            chunk = line[pos : m.start()]
            buf.append(chunk)
            params = m.group(1).split(";") if m.group(1) else ["0"]
            apply_sgr(state, params)
            # Replace the original SGR with one textimg will honor.
            buf.append(sgr_for(state))
            pos = m.end()
        buf.append(line[pos:])
        row = "".join(buf)
        pad = width - visible_len(row)
        if pad > 0:
            row += " " * pad
        out.append(row)
    # Keep exactly ROWS lines so the status bar stays on the last row.
    if len(out) < ROWS:
        out.extend([blank] * (ROWS - len(out)))
    elif len(out) > ROWS:
        out = out[:ROWS]
    return "\n".join(out) + "\n"


def plain_text(raw: str) -> str:
    return SGR_RE.sub("", CSI_OTHER_RE.sub("", raw))


def tmux_capture(session: str) -> str:
    result = subprocess.run(
        ["tmux", "capture-pane", "-t", session, "-e", "-N", "-p"],
        check=True,
        text=True,
        capture_output=True,
    )
    return result.stdout


def write_half_em_monaco(dst: Path) -> None:
    """Copy Monaco and set unitsPerEm so advance is 0.5 em.

    textimg ignores the font's advance and steps by fontSize/2.
    Raising unitsPerEm scales the outlines down to that cell.
    """
    if not MONACO_SRC.is_file():
        die(f"missing {MONACO_SRC}")
    data = bytearray(MONACO_SRC.read_bytes())
    num_tables = int.from_bytes(data[4:6], "big")
    head_off = None
    for i in range(num_tables):
        off = 12 + i * 16
        tag = bytes(data[off : off + 4])
        toff = int.from_bytes(data[off + 8 : off + 12], "big")
        if tag == b"head":
            head_off = toff
            break
    if head_off is None:
        die("Monaco.ttf has no head table")
    # head.unitsPerEm is a uint16 at offset 18.
    data[head_off + 18 : head_off + 20] = (2458).to_bytes(2, "big")
    dst.write_bytes(data)


def write_half_em_menlo(dst: Path) -> None:
    """Copy Menlo Regular (TTC face 0) and set unitsPerEm to 2*advance.

    textimg steps by fontSize/2. Menlo's advance is 1233/2048 em, so
    unitsPerEm 2466 makes that advance exactly half an em. Face 0 is
    Menlo Regular and includes U+256D..U+2570.
    """
    if not MENLO_SRC.is_file():
        die(f"missing {MENLO_SRC}")
    data = bytearray(MENLO_SRC.read_bytes())
    if data[:4] != b"ttcf":
        die(f"{MENLO_SRC} is not a TTC")
    num_fonts = int.from_bytes(data[8:12], "big")
    if num_fonts < 1:
        die(f"{MENLO_SRC} has no faces")
    face0 = int.from_bytes(data[12:16], "big")
    num_tables = int.from_bytes(data[face0 + 4 : face0 + 6], "big")
    head_off = None
    for i in range(num_tables):
        off = face0 + 12 + i * 16
        tag = bytes(data[off : off + 4])
        toff = int.from_bytes(data[off + 8 : off + 12], "big")
        if tag == b"head":
            head_off = toff
            break
    if head_off is None:
        die("Menlo face 0 has no head table")
    data[head_off + 18 : head_off + 20] = (2466).to_bytes(2, "big")
    dst.write_bytes(data)


def render_png(ansi_path: Path, png_path: Path, textimg: Path, font: Path) -> None:
    # textimg reads the ANSI text from stdin when no text argument
    # is given. It ignores SGR 1; the capture rewrite turns bold
    # into bright white. -g is gray so that bright white reads as
    # brighter than the body text.
    argv = [
        str(textimg),
        "-o",
        str(png_path),
        "-f",
        str(font),
        "-F",
        str(FONT_SIZE),
        "-b",
        "black",
        "-g",
        "192,192,192,255",
    ]
    subprocess.run(argv, check=True, input=ansi_path.read_bytes())


def git_identity(real_git: str) -> tuple[str, str]:
    name = subprocess.run(
        [real_git, "config", "--get", "user.name"],
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    email = subprocess.run(
        [real_git, "config", "--get", "user.email"],
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    return name, email


def make_sl_fixture(repo: Path, home: Path, name: str, email: str) -> None:
    """Branched Sapling repo so the smartlog graph has a fork.

    @ sits on the right-hand stack; `side` is a sibling draft:

        @  tip
        o  right
        | o  side
        |/
        o  root
    """
    repo.mkdir(parents=True)
    home.mkdir(parents=True)
    conf_dir = home / "Library" / "Preferences" / "sapling"
    conf_dir.mkdir(parents=True)
    (conf_dir / "sapling.conf").write_text(
        "[ui]\n"
        f"username = {name} <{email}>\n"
        "color = never\n"
        "paginate = never\n"
        "[fsmonitor]\n"
        "mode = off\n"
    )
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["GIT_CONFIG_GLOBAL"] = "/dev/null"
    env["GIT_CONFIG_SYSTEM"] = "/dev/null"

    def sl(*args: str) -> str:
        result = subprocess.run(
            ["sl", "--cwd", str(repo), *args],
            check=True,
            text=True,
            capture_output=True,
            env=env,
        )
        return result.stdout

    sl("init", "-q")

    def commit(filename: str, message: str, date: str) -> None:
        (repo / filename).write_text(message + "\n")
        sl("addremove")
        sl("commit", "-m", message, "--date", date)

    commit("root.txt", "root", "3 days ago")
    commit("side.txt", "side", "2 days ago")
    root = sl("log", "-r", ".^", "-T", "{node}\\n").strip()
    sl("goto", "-q", root)
    commit("right.txt", "right", "1 day ago")
    commit("tip.txt", "tip", "2 hours ago")


def _smartlog_selection_below_tip(raw: str) -> bool:
    """True when a non-tip commit row carries the row-highlight bg.

    `@` is the first commit. Its header line contains `@`. A selected
    row is painted with SGR 48;5;8 (the dark row highlight). The status
    bar uses that bg too, but it has no `@` / commit hash.
    """
    tip_highlighted = False
    other_highlighted = False
    for line in raw.split("\n"):
        cleaned = OSC_RE.sub("", line)
        if "48;5;8" not in cleaned and "48;5;7" not in cleaned:
            continue
        text = plain_text(cleaned)
        if text.startswith("Enter:") or "Top" in text or "Bot" in text:
            continue
        if "@" in text:
            tip_highlighted = True
        elif "o" in text:
            other_highlighted = True
    return other_highlighted and not tip_highlighted


def shot_smartlog(tmux: str, real_git: str) -> None:
    session = f"{SESSION_PREFIX}-sl-{os.getpid()}"
    subprocess.run(["tmux", "kill-session", "-t", session], capture_output=True)
    name, email = git_identity(real_git)
    with tempfile.TemporaryDirectory(prefix="sl-tui-sl-shot-") as tmp_s:
        tmp = Path(tmp_s)
        repo = tmp / "repo"
        home = tmp / "home"
        make_sl_fixture(repo, home, name, email)
        # sl-tui must see the same HOME so `sl smartlog` uses this
        # repo's commits and the fixture username, not another config.
        argv = [
            tmux,
            "new-session",
            "-d",
            "-s",
            session,
            "-x",
            str(COLS),
            "-y",
            str(ROWS),
            "-c",
            str(repo),
            "--",
            "env",
            f"HOME={home}",
            "GIT_CONFIG_GLOBAL=/dev/null",
            "GIT_CONFIG_SYSTEM=/dev/null",
            str(BINARY),
            "--sl",
            "-n",
            "60",
        ]
        print(f"starting tmux session {session}")
        run(argv)
        try:
            time.sleep(2.0)
            subprocess.run(
                ["tmux", "send-keys", "-t", session, "--", "t"],
                check=True,
            )
            time.sleep(0.8)
            # Default selection is `@` (tip). Move to the next commit
            # so the highlight is not on the newest commit.
            subprocess.run(
                ["tmux", "send-keys", "-t", session, "--", "j"],
                check=True,
            )
            time.sleep(0.3)
            raw = tmux_capture(session)
            plain = plain_text(raw)
            if "tip" not in plain:
                die(f"smartlog view did not show tip\n{plain}")
            if "side" not in plain and "right" not in plain:
                die(f"smartlog view did not show the fork\n{plain}")
            if "@" not in plain and "o" not in plain:
                die(f"smartlog graph missing\n{plain}")
            if "\u2502" not in plain and "\u256f" not in plain:
                die(f"smartlog graph is not box-drawing\n{plain}")
            # Selection must sit on a commit below `@`, not the tip.
            if not _smartlog_selection_below_tip(raw):
                die(f"smartlog selection is still on the tip\n{plain}")
            ansi = tmp / "smartlog.ansi"
            ansi.write_text(normalize_capture(raw, COLS))
            # Monaco omits ╭╮╰╯; Menlo draws the Sapling arc corners.
            font = tmp / "Menlo-halfem.ttc"
            write_half_em_menlo(font)
            OUT_DIR.mkdir(parents=True, exist_ok=True)
            png = OUT_DIR / "smartlog.png"
            print(f"writing {png}")
            render_png(ansi, png, TEXTIMG, font)
        finally:
            subprocess.run(
                ["tmux", "kill-session", "-t", session],
                capture_output=True,
            )


def main() -> None:
    tmux = require_tool("tmux")
    require_tool("dotslash")
    require_tool("sl")
    real_git = require_tool("git")
    if not TEXTIMG.is_file():
        die(f"missing {TEXTIMG}")
    build_binary()

    session = f"{SESSION_PREFIX}-{os.getpid()}"
    subprocess.run(["tmux", "kill-session", "-t", session], capture_output=True)

    with tempfile.TemporaryDirectory(prefix="sl-tui-shot-") as tmp_s:
        tmp = Path(tmp_s)
        repo = tmp / "repo"
        make_fixture(repo, real_git)

        # Long interval so a refresh cannot repaint between the two shots.
        argv = [
            tmux,
            "new-session",
            "-d",
            "-s",
            session,
            "-x",
            str(COLS),
            "-y",
            str(ROWS),
            "-c",
            str(repo),
            "--",
            str(BINARY),
            "--git",
            "-n",
            "60",
        ]
        print(f"starting tmux session {session}")
        run(argv)
        try:
            time.sleep(2.0)
            top_plain = plain_text(tmux_capture(session))
            if "@@" not in top_plain:
                die(f"diff view did not render a hunk\n{top_plain}")
            # Line-by-line scroll into the first file. One send-keys
            # with a repeated j is a single paste on some tmux builds;
            # separate keys keep each press a ScrollDown.
            for _ in range(SCROLL_LINES):
                subprocess.run(
                    ["tmux", "send-keys", "-t", session, "--", "j"],
                    check=True,
                )
                time.sleep(0.03)
            time.sleep(0.15)
            diff_raw = tmux_capture(session)
            diff_plain = plain_text(diff_raw)
            if "lib.rs" not in diff_plain:
                die(f"scrolled off the first file\n{diff_plain}")
            if "main.rs" not in diff_plain:
                die(f"second file not visible\n{diff_plain}")
            if "fit_name(s: &str, w: usize)" in diff_plain:
                die(
                    "first file diff is not truncated; "
                    f"its opening is still visible\n{diff_plain}"
                )
            if "+{add} -{del}" not in diff_plain:
                die(f"tail of the first file not visible\n{diff_plain}")
            if "Top" in diff_plain.splitlines()[-1] or diff_plain.splitlines()[
                -1
            ].rstrip().endswith("Bot"):
                die(f"expected a mid-scroll ruler\n{diff_plain}")
            # /bar<enter>: highlight the visible bar / change_bar hits.
            # Wait for the prompt before typing: in output mode, h opens
            # help and g jumps to the top.
            subprocess.run(
                ["tmux", "send-keys", "-t", session, "--", "/"],
                check=True,
            )
            time.sleep(0.2)
            subprocess.run(
                ["tmux", "send-keys", "-t", session, "-l", "--", "bar"],
                check=True,
            )
            subprocess.run(
                ["tmux", "send-keys", "-t", session, "--", "Enter"],
                check=True,
            )
            time.sleep(0.2)
            diff_raw = tmux_capture(session)
            diff_plain = plain_text(diff_raw)
            if "bar" not in diff_plain:
                die(f"search target not visible\n{diff_plain}")
            status = diff_plain.splitlines()[-1] if diff_plain.splitlines() else ""
            # Match label is [n/N] on the status bar.
            if "[" not in status or "/" not in status or "]" not in status:
                die(f"search highlight label missing\n{diff_plain}")

            subprocess.run(
                ["tmux", "send-keys", "-t", session, "--", "s"],
                check=True,
            )
            time.sleep(0.4)
            stat_raw = tmux_capture(session)
            stat_plain = plain_text(stat_raw)
            if " | " not in stat_plain:
                die(f"diff-stat view did not render\n{stat_plain}")
            if "untracked" not in stat_plain:
                die(f"diff-stat view did not list untracked files\n{stat_plain}")

            diff_ansi = tmp / "diff.ansi"
            stat_ansi = tmp / "diffstat.ansi"
            diff_ansi.write_text(normalize_capture(diff_raw, COLS))
            stat_ansi.write_text(normalize_capture(stat_raw, COLS))

            font = tmp / "Monaco-halfem.ttf"
            write_half_em_monaco(font)
            OUT_DIR.mkdir(parents=True, exist_ok=True)
            diff_png = OUT_DIR / "diff.png"
            stat_png = OUT_DIR / "diffstat.png"
            print(f"writing {diff_png}")
            render_png(diff_ansi, diff_png, TEXTIMG, font)
            print(f"writing {stat_png}")
            render_png(stat_ansi, stat_png, TEXTIMG, font)
        finally:
            subprocess.run(
                ["tmux", "kill-session", "-t", session],
                capture_output=True,
            )

    shot_smartlog(tmux, real_git)
    print("done")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "smartlog":
        require_tool("tmux")
        require_tool("dotslash")
        require_tool("sl")
        real_git = require_tool("git")
        if not TEXTIMG.is_file():
            die(f"missing {TEXTIMG}")
        build_binary()
        shot_smartlog(require_tool("tmux"), real_git)
        print("done")
    else:
        main()
