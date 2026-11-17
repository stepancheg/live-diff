use std::io;
use std::mem;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::abort_on_drop_handle::AbortOnDropHandle;
use crate::capture::CaptureSpec;
use crate::capture::DIFF_VIEW_KINDS;
use crate::capture::DiffViewKind;
use crate::diff::DiffOutput;
use crate::diff::file_list_lines;
use crate::height::Height;
use crate::help::help_lines;
use crate::mark::MarkOp;
use crate::mark::Marks;
use crate::mark::NOT_SET;
use crate::notify_pair::NotifyTx;
use crate::render::Renderer;
use crate::render::ScrollAnchor;
use crate::repo::repo::DiffTool;
use crate::repo::repo::step_context;
use crate::search::find::SearchOutcome;
use crate::search::find::search_next;
use crate::search::find::submit_prompt;
use crate::search::history::store::History;
use crate::search::prompt::Prompt;
use crate::search::prompt::PromptOutcome;
use crate::search::state::CaseMode;
use crate::search::state::INVALID_PATTERN;
use crate::search::state::NO_PREVIOUS_SEARCH;
use crate::search::state::PATTERN_NOT_FOUND;
use crate::search::state::Search;
use crate::search::state::is_case_mode_status;
use crate::search::state::status_after_output;
use crate::size::Size;
use crate::terminal::line::Line;
use crate::terminal::line::compile_search;
use crate::terminal::line::fold_case;
use crate::terminal::width::Width;
use crate::terminal_bg::TerminalBg;
use crate::theme::diff_file_header_style;
use crate::theme::diff_note_style;

enum KeyResult {
    Continue,
    Quit,
    Refresh,
}

/// Where help returns when it closes.
#[derive(Clone, Copy)]
enum HelpBack {
    Output,
    Files,
    DiffSelect,
}

enum Mode {
    Output,
    /// Help page. q/h return to `back`.
    Help {
        back: HelpBack,
    },
    /// File list: one row per diff file. Enter jumps to that file.
    Files,
    /// Diff-mode menu (`d`): default, `-c .`, `-r prev`.
    DiffSelect,
    Prompt(Prompt),
    /// Waiting for the option letter after `-`.
    Dash,
    /// Waiting for the mark name after `m` or `'`.
    Mark(MarkOp),
}

/// On-screen state for output, help, and the search prompt.
pub(crate) struct EventLoop {
    renderer: Renderer,
    help_renderer: Renderer,
    search: Search,
    history: History,
    marks: Marks,
    mode: Mode,
    size: Size,
    /// Unified diff context (`-U`). The capture loop reads this each run.
    context: Arc<AtomicU32>,
    /// Latest snapshot, kept so the file-list mode can list paths and stats.
    output: DiffOutput,
    /// Selected row in the file list.
    file_list_index: usize,
    /// Capture command. `d` replaces this and wakes the capture loop.
    spec: Arc<Mutex<CaptureSpec>>,
    /// Repo tool for the diff-mode menu. `None` when there is no repo
    /// (a PR diff started outside a working tree).
    tool: Option<DiffTool>,
    /// Selected row in the diff-mode menu.
    diff_mode_index: usize,
}

impl EventLoop {
    pub(crate) fn new(
        terminal_bg: TerminalBg,
        history: History,
        size: Size,
        context: Arc<AtomicU32>,
        spec: Arc<Mutex<CaptureSpec>>,
        tool: Option<DiffTool>,
    ) -> EventLoop {
        EventLoop {
            renderer: Renderer::new(terminal_bg, size),
            help_renderer: Renderer::new(terminal_bg, size),
            search: Search::new(),
            history,
            marks: Marks::default(),
            mode: Mode::Output,
            size,
            context,
            output: DiffOutput::message(""),
            file_list_index: 0,
            spec,
            tool,
            diff_mode_index: 0,
        }
    }

    pub(crate) async fn run(
        &mut self,
        first_lines: DiffOutput,
        output_rx: Receiver<DiffOutput>,
        keys: UnboundedReceiver<io::Result<Event>>,
        refresh_tx: NotifyTx,
        capture_task: AbortOnDropHandle<()>,
    ) -> anyhow::Result<()> {
        let result = self
            .drive(first_lines, output_rx, keys, refresh_tx, capture_task)
            .await;
        self.history.shutdown().await;
        result
    }

    async fn drive(
        &mut self,
        first_lines: DiffOutput,
        mut output_rx: Receiver<DiffOutput>,
        mut keys: UnboundedReceiver<io::Result<Event>>,
        refresh_tx: NotifyTx,
        capture_task: AbortOnDropHandle<()>,
    ) -> anyhow::Result<()> {
        self.on_command_output(first_lines)?;
        loop {
            tokio::select! {
                msg = output_rx.recv() => {
                    let Some(output) = msg else {
                        // Capture loop exited (receiver dropped or task ended).
                        capture_task.await?;
                        return Ok(());
                    };
                    self.on_command_output(output)?;
                }
                event = keys.recv() => {
                    let Some(event) = event else {
                        return Err(io::Error::other("key reader stopped").into());
                    };
                    match event? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            match self.on_key(key)? {
                                KeyResult::Continue => {}
                                KeyResult::Quit => {
                                    // `capture_task` and the key reader abort on drop.
                                    return Ok(());
                                }
                                KeyResult::Refresh => {
                                    // Wakes the interval wait. A permit is stored if a
                                    // capture is in progress, so the next wait is skipped.
                                    refresh_tx.notify_one();
                                }
                            }
                        }
                        Event::Resize(cols, rows) => {
                            self.on_resize(cols, rows)?;
                        }
                        Event::Paste(text) => {
                            self.on_paste(&text)?;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn on_resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.size = Size {
            width: Width(u32::from(cols)),
            height: Height(u32::from(rows)),
        };
        self.renderer.update_size(self.size);
        self.help_renderer.update_size(self.size);
        match self.mode {
            Mode::Help { .. } | Mode::Files | Mode::DiffSelect => {
                self.help_renderer.redraw(&mut io::stdout())?;
            }
            _ => {
                self.renderer.redraw(&mut io::stdout())?;
            }
        }
        Ok(())
    }

    fn resume_search_pattern(&mut self) {
        let entries = self.history.entries();
        self.search.resume_from_history(entries);
    }

    fn on_command_output(&mut self, output: DiffOutput) -> anyhow::Result<()> {
        let anchor = self.search.anchor(self.renderer.lines());
        let mark_anchors: Vec<(char, ScrollAnchor)> = self
            .marks
            .rows
            .iter()
            .map(|(&name, &pos)| (name, self.renderer.anchor_at(pos)))
            .collect();
        self.retain_file_list_selection(&output);
        self.output = output.clone();
        self.renderer.set_diff(output);
        self.renderer
            .set_status_note(untracked_status_note(self.output.untracked().len()));
        self.sync_status_mode();
        for (name, mark_anchor) in mark_anchors {
            self.marks
                .rows
                .insert(name, self.renderer.pos_for_anchor(&mark_anchor));
        }
        self.search.reattach(self.renderer.lines(), anchor);
        self.search.apply_highlight(&mut self.renderer);
        if !matches!(self.mode, Mode::Prompt(_) | Mode::Dash | Mode::Mark(_)) {
            let label = status_after_output(
                self.renderer.status_label(),
                self.search.match_label(self.renderer.lines()),
            );
            self.renderer.set_status_label(label);
        }
        match self.mode {
            Mode::Help { .. } | Mode::DiffSelect => {}
            // In-place refresh. `draw` skips the terminal write when the
            // visible list is unchanged, so a text selection survives.
            Mode::Files => self.paint_file_list(false)?,
            _ => self.renderer.draw(&mut io::stdout())?,
        }
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) -> anyhow::Result<KeyResult> {
        match &self.mode {
            Mode::Prompt(_) => {
                self.on_prompt_key(key)?;
                Ok(KeyResult::Continue)
            }
            Mode::Dash => self.on_dash_key(key),
            Mode::Mark(op) => {
                let op = *op;
                self.on_mark_key(key, op)
            }
            Mode::Help { .. } => self.on_help_key(key),
            Mode::Files => self.on_files_key(key),
            Mode::DiffSelect => self.on_diff_select_key(key),
            Mode::Output => self.on_output_key(key),
        }
    }

    fn on_dash_key(&mut self, key: KeyEvent) -> anyhow::Result<KeyResult> {
        self.mode = Mode::Output;
        let option = match key.code {
            KeyCode::Char('i') if !key.modifiers.contains(KeyModifiers::CONTROL) => Some(false),
            KeyCode::Char('I') if !key.modifiers.contains(KeyModifiers::CONTROL) => Some(true),
            _ => None,
        };
        if let Some(always) = option {
            let anchor = self.search.anchor(self.renderer.lines());
            if always {
                self.search.toggle_ignore_case_always();
            } else {
                self.search.toggle_ignore_case();
            }
            self.search.reattach(self.renderer.lines(), anchor);
            let top_line = self.renderer.top_line_index();
            let bottom_line = self.renderer.bottom_line_index();
            self.search
                .select_visible_match(self.renderer.lines(), top_line, bottom_line);
            self.search.apply_highlight(&mut self.renderer);
            let status = self
                .search
                .case_mode()
                .status(self.search.match_label(self.renderer.lines()).as_deref());
            self.renderer.set_status_label(Some(status));
            self.renderer.redraw(&mut io::stdout())?;
            return Ok(KeyResult::Continue);
        }
        // Unknown option or cancel: drop the `-` prompt.
        let label = self.search.match_label(self.renderer.lines());
        self.renderer.set_status_label(label);
        self.renderer.redraw(&mut io::stdout())?;
        Ok(KeyResult::Continue)
    }

    fn on_paste(&mut self, text: &str) -> anyhow::Result<()> {
        let mut prompt = match mem::replace(&mut self.mode, Mode::Output) {
            Mode::Prompt(prompt) => prompt,
            other => {
                self.mode = other;
                return Ok(());
            }
        };
        if prompt.insert_pasted(text) {
            self.renderer
                .set_prompt_status(prompt.status_label(), prompt.caret_cols());
            self.mode = Mode::Prompt(prompt);
            self.renderer.draw(&mut io::stdout())?;
        } else {
            self.mode = Mode::Prompt(prompt);
        }
        Ok(())
    }

    fn on_prompt_key(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        let mut prompt = match mem::replace(&mut self.mode, Mode::Output) {
            Mode::Prompt(prompt) => prompt,
            other => {
                self.mode = other;
                return Ok(());
            }
        };
        match prompt.on_key(&key, self.history.entries()) {
            PromptOutcome::Editing => {
                self.renderer
                    .set_prompt_status(prompt.status_label(), prompt.caret_cols());
                self.mode = Mode::Prompt(prompt);
                self.renderer.draw(&mut io::stdout())?;
            }
            PromptOutcome::Redraw => {
                self.mode = Mode::Prompt(prompt);
                self.renderer.redraw(&mut io::stdout())?;
            }
            PromptOutcome::Ignore => {
                self.mode = Mode::Prompt(prompt);
            }
            PromptOutcome::Cancel => {
                let label = self.search.match_label(self.renderer.lines());
                self.renderer.set_status_label(label);
                self.renderer.redraw(&mut io::stdout())?;
            }
            PromptOutcome::Submit => {
                let forward = prompt.forward();
                let query = prompt.take_query();
                if query.is_empty() {
                    self.resume_search_pattern();
                } else {
                    let mode = self.search.case_mode();
                    let fold = fold_case(
                        &query,
                        mode == CaseMode::IgnoreCase,
                        mode == CaseMode::IgnoreCaseAlways,
                    );
                    if compile_search(&query, fold).is_ok() {
                        self.history.record(&query);
                    }
                }
                let outcome = submit_prompt(&mut self.renderer, &mut self.search, forward, query);
                show_search_outcome(&mut self.renderer, &self.search, outcome)?;
            }
        }
        Ok(())
    }

    fn begin_mark(&mut self, op: MarkOp) -> anyhow::Result<KeyResult> {
        self.mode = Mode::Mark(op);
        self.renderer
            .set_status_label(Some(op.prompt_label().to_owned()));
        self.renderer.redraw(&mut io::stdout())?;
        Ok(KeyResult::Continue)
    }

    fn on_mark_key(&mut self, key: KeyEvent, op: MarkOp) -> anyhow::Result<KeyResult> {
        self.mode = Mode::Output;
        let name = match key.code {
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                c
            }
            _ => {
                let label = self.search.match_label(self.renderer.lines());
                self.renderer.set_status_label(label);
                self.renderer.redraw(&mut io::stdout())?;
                return Ok(KeyResult::Continue);
            }
        };
        match op {
            MarkOp::Set => {
                self.marks.rows.insert(name, self.renderer.top_pos());
                let label = self.search.match_label(self.renderer.lines());
                self.renderer.set_status_label(label);
                self.renderer.redraw(&mut io::stdout())?;
            }
            MarkOp::Goto => {
                let Some(pos) = self.marks.rows.get(&name).copied() else {
                    self.renderer.set_status_label(Some(NOT_SET.to_owned()));
                    self.renderer.redraw(&mut io::stdout())?;
                    return Ok(KeyResult::Continue);
                };
                self.search.clear_match();
                self.search.apply_highlight(&mut self.renderer);
                self.renderer.set_status_label(None);
                self.renderer.scroll_to_pos(pos);
                self.renderer.redraw(&mut io::stdout())?;
            }
        }
        Ok(KeyResult::Continue)
    }

    fn open_file_list(&mut self) -> anyhow::Result<KeyResult> {
        let n = self.output.files().len();
        self.file_list_index = match self.renderer.current_file_index() {
            Some(i) if i < n => i,
            _ => 0,
        };
        self.mode = Mode::Files;
        // Start with the current file at the top of the list.
        self.help_renderer.scroll_top();
        // Screen still shows the diff; the redraw below paints the list.
        self.paint_file_list(false)?;
        self.help_renderer.scroll_to_line(self.file_list_index);
        self.help_renderer.redraw(&mut io::stdout())?;
        Ok(KeyResult::Continue)
    }

    fn close_file_list(&mut self) -> anyhow::Result<KeyResult> {
        self.mode = Mode::Output;
        self.renderer.redraw(&mut io::stdout())?;
        Ok(KeyResult::Continue)
    }

    /// Leave help for the view it was opened from. The stat page and
    /// help share `help_renderer`, so returning to the list must paint
    /// over the help text.
    fn close_help(&mut self) -> anyhow::Result<()> {
        let back = match self.mode {
            Mode::Help { back } => back,
            _ => HelpBack::Output,
        };
        match back {
            HelpBack::Files => {
                self.mode = Mode::Files;
                self.paint_file_list(true)?;
            }
            HelpBack::DiffSelect => {
                self.mode = Mode::DiffSelect;
                self.paint_diff_select(true)?;
            }
            HelpBack::Output => {
                self.mode = Mode::Output;
                self.renderer.redraw(&mut io::stdout())?;
            }
        }
        Ok(())
    }

    fn open_selected_file(&mut self) -> anyhow::Result<KeyResult> {
        let n_files = self.output.files().len();
        // Untracked rows have no diff to open; stay on the stat page.
        if self.file_list_index >= n_files {
            if self.file_list_index < self.output.stat_row_count() {
                return Ok(KeyResult::Continue);
            }
        }
        if n_files > 0 {
            let index = self.file_list_index.min(n_files - 1);
            self.renderer.scroll_to_file(index);
            self.search.clear_match();
            self.search.apply_highlight(&mut self.renderer);
            self.renderer.set_status_label(None);
        }
        self.mode = Mode::Output;
        self.renderer.redraw(&mut io::stdout())?;
        Ok(KeyResult::Continue)
    }

    fn draw_file_list(&mut self) -> anyhow::Result<()> {
        // Selection moved, or the list is already on screen: repaint
        // only if the frame actually changed.
        self.paint_file_list(false)
    }

    /// Build the stat page and paint it.
    ///
    /// `force` must be set when the diff (or another view) was just
    /// written to the screen: the help renderer's cache can still match
    /// this list, and a non-forced draw would leave that other view up.
    /// A refresh of an unchanged list does not write anything, so the
    /// terminal text selection is kept.
    fn paint_file_list(&mut self, force: bool) -> anyhow::Result<()> {
        let files = self.output.files();
        let untracked = self.output.untracked();
        let n = files.len().saturating_add(untracked.len());
        if n == 0 {
            self.file_list_index = 0;
        } else if self.file_list_index >= n {
            self.file_list_index = n - 1;
        }
        let lines = file_list_lines(
            &files,
            untracked,
            self.file_list_index,
            self.help_renderer.terminal_bg(),
            self.size.width,
        );
        let hint = if self.file_list_index >= files.len() {
            if !untracked.is_empty() {
                "untracked"
            } else {
                "Enter: go to file"
            }
        } else {
            "Enter: go to file"
        };
        if !force {
            let same_lines = self.help_renderer.lines() == lines.as_slice();
            let same_label = self.help_renderer.status_label() == Some(hint);
            if same_lines {
                if same_label {
                    return Ok(());
                }
            }
        }
        self.help_renderer.set_lines(lines);
        self.help_renderer
            .set_status_label(Some(String::from(hint)));
        self.help_renderer.ensure_line_visible(self.file_list_index);
        if force {
            self.help_renderer.redraw(&mut io::stdout())?;
        } else {
            self.help_renderer.draw(&mut io::stdout())?;
        }
        Ok(())
    }

    fn retain_file_list_selection(&mut self, output: &DiffOutput) {
        let old_labels = stat_list_labels(&self.output);
        let selected = old_labels.get(self.file_list_index).cloned();
        let new_labels = stat_list_labels(output);
        if let Some(label) = selected {
            if let Some(i) = new_labels.iter().position(|l| *l == label) {
                self.file_list_index = i;
                return;
            }
        }
        if new_labels.is_empty() {
            self.file_list_index = 0;
        } else if self.file_list_index >= new_labels.len() {
            self.file_list_index = new_labels.len() - 1;
        }
    }

    fn move_file_selection(&mut self, target: usize) -> anyhow::Result<()> {
        let n = self.output.stat_row_count();
        if n == 0 {
            self.file_list_index = 0;
        } else {
            self.file_list_index = target.min(n - 1);
        }
        self.draw_file_list()
    }

    fn on_files_key(&mut self, key: KeyEvent) -> anyhow::Result<KeyResult> {
        if key.kind != KeyEventKind::Press {
            return Ok(KeyResult::Continue);
        }
        // Enter opens the file. key_action maps Enter to ScrollDown, so
        // handle it before that table.
        if key.code == KeyCode::Enter {
            return self.open_selected_file();
        }
        let Some(action) = key_action(&key) else {
            return Ok(KeyResult::Continue);
        };
        let n = self.output.stat_row_count();
        let page = list_page_size(self.size);
        match action {
            KeyAction::Quit => Ok(KeyResult::Quit),
            KeyAction::FileList => self.open_selected_file(),
            KeyAction::DismissMessage => self.close_file_list(),
            KeyAction::Help => {
                // Leave the list and show help. Closing help returns here.
                self.mode = Mode::Help {
                    back: HelpBack::Files,
                };
                self.help_renderer.set_lines(help_lines());
                self.help_renderer.set_status_label(None);
                self.help_renderer.scroll_top();
                self.help_renderer.redraw(&mut io::stdout())?;
                Ok(KeyResult::Continue)
            }
            KeyAction::Refresh => Ok(KeyResult::Refresh),
            KeyAction::ContextLess | KeyAction::ContextMore => {
                // Apply on the diff underneath; stay in the list. The
                // next snapshot redraws the list.
                let result = self.change_context(action)?;
                self.mode = Mode::Files;
                // change_context redrew the diff. Paint the list back
                // over it; the cached list frame would otherwise match
                // and a non-forced draw would leave the diff up.
                self.paint_file_list(true)?;
                Ok(result)
            }
            KeyAction::ScrollDown => {
                let next = self.file_list_index.saturating_add(1);
                self.move_file_selection(next)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollUp => {
                let prev = self.file_list_index.saturating_sub(1);
                self.move_file_selection(prev)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollTop => {
                self.move_file_selection(0)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollBottom => {
                self.move_file_selection(n.saturating_sub(1))?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollPageDown => {
                let next = self.file_list_index.saturating_add(page);
                self.move_file_selection(next)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollPageUp => {
                let prev = self.file_list_index.saturating_sub(page);
                self.move_file_selection(prev)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::Redraw => {
                self.help_renderer.redraw(&mut io::stdout())?;
                Ok(KeyResult::Continue)
            }
            KeyAction::Dash
            | KeyAction::DiffMode
            | KeyAction::SearchForward
            | KeyAction::SearchBackward
            | KeyAction::SearchNext
            | KeyAction::SearchPrev
            | KeyAction::SetMark
            | KeyAction::GotoMark => Ok(KeyResult::Continue),
        }
    }

    fn sync_status_mode(&mut self) {
        let text = self.spec.lock().unwrap().mode_status();
        self.renderer.set_status_mode(Some(text));
    }

    fn open_diff_select(&mut self) -> anyhow::Result<KeyResult> {
        if self.tool.is_none() {
            self.renderer
                .set_status_label(Some(String::from("not an sl or git repository")));
            self.renderer.redraw(&mut io::stdout())?;
            return Ok(KeyResult::Continue);
        }
        let current = self.spec.lock().unwrap().view_kind();
        self.diff_mode_index = match current {
            Some(kind) => DIFF_VIEW_KINDS
                .iter()
                .position(|item| *item == kind)
                .unwrap_or(0),
            None => 0,
        };
        self.mode = Mode::DiffSelect;
        self.help_renderer.scroll_top();
        self.paint_diff_select(true)?;
        Ok(KeyResult::Continue)
    }

    fn close_diff_select(&mut self) -> anyhow::Result<KeyResult> {
        self.mode = Mode::Output;
        self.renderer.redraw(&mut io::stdout())?;
        Ok(KeyResult::Continue)
    }

    fn paint_diff_select(&mut self, force: bool) -> anyhow::Result<()> {
        let current = self.spec.lock().unwrap().view_kind();
        let lines = diff_mode_lines(
            self.diff_mode_index,
            current,
            self.help_renderer.terminal_bg(),
        );
        let hint = "Enter: select";
        if !force {
            let same_lines = self.help_renderer.lines() == lines.as_slice();
            let same_label = self.help_renderer.status_label() == Some(hint);
            if same_lines {
                if same_label {
                    return Ok(());
                }
            }
        }
        self.help_renderer.set_lines(lines);
        self.help_renderer
            .set_status_label(Some(String::from(hint)));
        self.help_renderer.ensure_line_visible(self.diff_mode_index);
        if force {
            self.help_renderer.redraw(&mut io::stdout())?;
        } else {
            self.help_renderer.draw(&mut io::stdout())?;
        }
        Ok(())
    }

    fn move_diff_mode_selection(&mut self, target: usize) -> anyhow::Result<()> {
        let last = DIFF_VIEW_KINDS.len().saturating_sub(1);
        self.diff_mode_index = target.min(last);
        self.paint_diff_select(false)
    }

    fn apply_diff_mode_selection(&mut self) -> anyhow::Result<KeyResult> {
        let Some(tool) = self.tool else {
            return self.close_diff_select();
        };
        let Some(kind) = DIFF_VIEW_KINDS.get(self.diff_mode_index).copied() else {
            return self.close_diff_select();
        };
        {
            let mut spec = self.spec.lock().unwrap();
            *spec = CaptureSpec::for_kind(tool, kind);
        }
        self.sync_status_mode();
        self.mode = Mode::Output;
        self.search.clear_match();
        self.search.apply_highlight(&mut self.renderer);
        self.renderer.scroll_top();
        self.renderer.set_status_label(None);
        self.renderer.redraw(&mut io::stdout())?;
        Ok(KeyResult::Refresh)
    }

    fn on_diff_select_key(&mut self, key: KeyEvent) -> anyhow::Result<KeyResult> {
        if key.kind != KeyEventKind::Press {
            return Ok(KeyResult::Continue);
        }
        if key.code == KeyCode::Enter {
            return self.apply_diff_mode_selection();
        }
        let Some(action) = key_action(&key) else {
            return Ok(KeyResult::Continue);
        };
        let n = DIFF_VIEW_KINDS.len();
        let page = list_page_size(self.size);
        match action {
            KeyAction::Quit => Ok(KeyResult::Quit),
            KeyAction::DiffMode | KeyAction::DismissMessage => self.close_diff_select(),
            KeyAction::Help => {
                self.mode = Mode::Help {
                    back: HelpBack::DiffSelect,
                };
                self.help_renderer.set_lines(help_lines());
                self.help_renderer.set_status_label(None);
                self.help_renderer.scroll_top();
                self.help_renderer.redraw(&mut io::stdout())?;
                Ok(KeyResult::Continue)
            }
            KeyAction::Refresh => Ok(KeyResult::Refresh),
            KeyAction::ContextLess | KeyAction::ContextMore => {
                let result = self.change_context(action)?;
                self.mode = Mode::DiffSelect;
                self.paint_diff_select(true)?;
                Ok(result)
            }
            KeyAction::ScrollDown => {
                let next = self.diff_mode_index.saturating_add(1);
                self.move_diff_mode_selection(next)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollUp => {
                let prev = self.diff_mode_index.saturating_sub(1);
                self.move_diff_mode_selection(prev)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollTop => {
                self.move_diff_mode_selection(0)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollBottom => {
                self.move_diff_mode_selection(n.saturating_sub(1))?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollPageDown => {
                let next = self.diff_mode_index.saturating_add(page);
                self.move_diff_mode_selection(next)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::ScrollPageUp => {
                let prev = self.diff_mode_index.saturating_sub(page);
                self.move_diff_mode_selection(prev)?;
                Ok(KeyResult::Continue)
            }
            KeyAction::Redraw => {
                self.help_renderer.redraw(&mut io::stdout())?;
                Ok(KeyResult::Continue)
            }
            KeyAction::FileList
            | KeyAction::Dash
            | KeyAction::SearchForward
            | KeyAction::SearchBackward
            | KeyAction::SearchNext
            | KeyAction::SearchPrev
            | KeyAction::SetMark
            | KeyAction::GotoMark => Ok(KeyResult::Continue),
        }
    }

    fn change_context(&mut self, action: KeyAction) -> anyhow::Result<KeyResult> {
        let increase = matches!(action, KeyAction::ContextMore);
        let current = self.context.load(Ordering::SeqCst);
        let next = step_context(current, increase);
        self.context.store(next, Ordering::SeqCst);
        self.renderer
            .set_status_label(Some(format!("context {next}")));
        self.renderer.redraw(&mut io::stdout())?;
        if next == current {
            Ok(KeyResult::Continue)
        } else {
            Ok(KeyResult::Refresh)
        }
    }

    fn on_help_key(&mut self, key: KeyEvent) -> anyhow::Result<KeyResult> {
        let Some(action) = key_action(&key) else {
            return Ok(KeyResult::Continue);
        };
        match action {
            KeyAction::Quit | KeyAction::Help => {
                self.close_help()?;
                Ok(KeyResult::Continue)
            }
            KeyAction::DismissMessage => {
                let Some(status) = self.renderer.status_label() else {
                    return Ok(KeyResult::Continue);
                };
                if is_dismissable_status(status) {
                    let label = self.search.match_label(self.renderer.lines());
                    self.renderer.set_status_label(label);
                }
                Ok(KeyResult::Continue)
            }
            KeyAction::Refresh => Ok(KeyResult::Refresh),
            KeyAction::ContextLess | KeyAction::ContextMore => {
                let back = match self.mode {
                    Mode::Help { back } => back,
                    _ => HelpBack::Output,
                };
                match back {
                    HelpBack::Files => {
                        // Same as u/U on the stat page: change context and
                        // stay on the list.
                        let result = self.change_context(action)?;
                        self.mode = Mode::Files;
                        self.paint_file_list(true)?;
                        Ok(result)
                    }
                    HelpBack::DiffSelect => {
                        let result = self.change_context(action)?;
                        self.mode = Mode::DiffSelect;
                        self.paint_diff_select(true)?;
                        Ok(result)
                    }
                    HelpBack::Output => {
                        self.mode = Mode::Output;
                        self.change_context(action)
                    }
                }
            }
            KeyAction::FileList | KeyAction::DiffMode => Ok(KeyResult::Continue),
            KeyAction::Dash => Ok(KeyResult::Continue),
            KeyAction::SearchForward
            | KeyAction::SearchBackward
            | KeyAction::SearchNext
            | KeyAction::SearchPrev
            | KeyAction::SetMark
            | KeyAction::GotoMark => Ok(KeyResult::Continue),
            action @ (KeyAction::ScrollDown
            | KeyAction::ScrollUp
            | KeyAction::Redraw
            | KeyAction::ScrollTop
            | KeyAction::ScrollBottom
            | KeyAction::ScrollPageDown
            | KeyAction::ScrollPageUp) => {
                apply_view_action(&mut self.help_renderer, action)?;
                Ok(KeyResult::Continue)
            }
        }
    }

    fn on_output_key(&mut self, key: KeyEvent) -> anyhow::Result<KeyResult> {
        let Some(action) = key_action(&key) else {
            return Ok(KeyResult::Continue);
        };
        match action {
            KeyAction::Dash => {
                self.mode = Mode::Dash;
                self.renderer.set_status_label(Some("-".to_owned()));
                self.renderer.redraw(&mut io::stdout())?;
                Ok(KeyResult::Continue)
            }
            KeyAction::DismissMessage => {
                let Some(status) = self.renderer.status_label() else {
                    return Ok(KeyResult::Continue);
                };
                if !is_dismissable_status(status) {
                    return Ok(KeyResult::Continue);
                }
                let label = self.search.match_label(self.renderer.lines());
                self.renderer.set_status_label(label);
                self.renderer.redraw(&mut io::stdout())?;
                Ok(KeyResult::Continue)
            }
            KeyAction::Quit => Ok(KeyResult::Quit),
            KeyAction::Help => {
                self.mode = Mode::Help {
                    back: HelpBack::Output,
                };
                self.help_renderer.set_lines(help_lines());
                self.help_renderer.scroll_top();
                self.help_renderer.redraw(&mut io::stdout())?;
                Ok(KeyResult::Continue)
            }
            KeyAction::Refresh => Ok(KeyResult::Refresh),
            KeyAction::ContextLess | KeyAction::ContextMore => self.change_context(action),
            KeyAction::FileList => self.open_file_list(),
            KeyAction::DiffMode => self.open_diff_select(),
            KeyAction::SearchForward | KeyAction::SearchBackward => {
                let forward = matches!(action, KeyAction::SearchForward);
                let prompt = Prompt::new(forward);
                self.renderer
                    .set_prompt_status(prompt.status_label(), prompt.caret_cols());
                self.mode = Mode::Prompt(prompt);
                self.renderer.draw(&mut io::stdout())?;
                Ok(KeyResult::Continue)
            }
            KeyAction::SetMark => self.begin_mark(MarkOp::Set),
            KeyAction::GotoMark => self.begin_mark(MarkOp::Goto),
            KeyAction::SearchNext | KeyAction::SearchPrev => {
                let reverse = matches!(action, KeyAction::SearchPrev);
                self.resume_search_pattern();
                let outcome = search_next(&mut self.renderer, &mut self.search, reverse);
                show_search_outcome(&mut self.renderer, &self.search, outcome)?;
                Ok(KeyResult::Continue)
            }
            action @ (KeyAction::ScrollDown
            | KeyAction::ScrollUp
            | KeyAction::Redraw
            | KeyAction::ScrollTop
            | KeyAction::ScrollBottom
            | KeyAction::ScrollPageDown
            | KeyAction::ScrollPageUp) => {
                if !matches!(action, KeyAction::Redraw) {
                    // Drop the selected hit so the next n/N searches from
                    // the screen edge, like a fresh / or ?.
                    self.search.clear_match();
                    self.search.apply_highlight(&mut self.renderer);
                    self.renderer.set_status_label(None);
                }
                apply_view_action(&mut self.renderer, action)?;
                Ok(KeyResult::Continue)
            }
        }
    }
}

fn show_search_outcome(
    renderer: &mut Renderer,
    search: &Search,
    outcome: SearchOutcome,
) -> anyhow::Result<()> {
    match outcome {
        SearchOutcome::Found => {
            renderer.set_status_label(search.match_label(renderer.lines()));
        }
        SearchOutcome::NotFound => match search.match_label(renderer.lines()) {
            Some(label) => {
                let mut msg = String::from(PATTERN_NOT_FOUND);
                msg.push(' ');
                msg.push_str(&label);
                renderer.set_status_label(Some(msg));
            }
            None => {
                renderer.set_status_label(Some(PATTERN_NOT_FOUND.to_owned()));
            }
        },
        SearchOutcome::NoPattern => {
            renderer.set_status_label(Some(NO_PREVIOUS_SEARCH.to_owned()));
        }
        SearchOutcome::InvalidPattern => {
            renderer.set_status_label(Some(INVALID_PATTERN.to_owned()));
        }
    }
    renderer.redraw(&mut io::stdout())?;
    Ok(())
}

/// Status-bar note for the diff view. Hidden when there are none.
fn untracked_status_note(count: usize) -> Option<String> {
    if count == 0 {
        None
    } else {
        Some(format!("{count} untracked"))
    }
}

/// Identity of each diff-stat row, so a refresh can keep the same
/// selection. Diff files and untracked paths use different prefixes
/// because a path can match a header label.
fn stat_list_labels(output: &DiffOutput) -> Vec<String> {
    let mut labels = Vec::new();
    for file in output.files() {
        labels.push(format!("f:{}", crate::diff::file_header_text(file)));
    }
    for path in output.untracked() {
        labels.push(format!("u:{path}"));
    }
    labels
}

fn apply_view_action(view: &mut Renderer, action: KeyAction) -> anyhow::Result<()> {
    match action {
        KeyAction::ScrollDown => {
            view.scroll_down();
            view.draw(&mut io::stdout())?;
        }
        KeyAction::ScrollUp => {
            view.scroll_up();
            view.draw(&mut io::stdout())?;
        }
        KeyAction::Redraw => {
            view.redraw(&mut io::stdout())?;
        }
        KeyAction::ScrollTop => {
            view.scroll_top();
            view.draw(&mut io::stdout())?;
        }
        KeyAction::ScrollBottom => {
            view.scroll_bottom();
            view.draw(&mut io::stdout())?;
        }
        KeyAction::ScrollPageDown => {
            view.scroll_page_down();
            view.draw(&mut io::stdout())?;
        }
        KeyAction::ScrollPageUp => {
            view.scroll_page_up();
            view.draw(&mut io::stdout())?;
        }
        KeyAction::Quit
        | KeyAction::Help
        | KeyAction::DismissMessage
        | KeyAction::Dash
        | KeyAction::Refresh
        | KeyAction::ContextLess
        | KeyAction::ContextMore
        | KeyAction::FileList
        | KeyAction::DiffMode
        | KeyAction::SearchForward
        | KeyAction::SearchBackward
        | KeyAction::SearchNext
        | KeyAction::SearchPrev
        | KeyAction::SetMark
        | KeyAction::GotoMark => {}
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum KeyAction {
    Quit,
    Help,
    DismissMessage,
    Dash,
    Refresh,
    SearchForward,
    SearchBackward,
    SearchNext,
    SearchPrev,
    ScrollDown,
    ScrollUp,
    ScrollTop,
    ScrollBottom,
    ScrollPageDown,
    ScrollPageUp,
    Redraw,
    SetMark,
    GotoMark,
    ContextLess,
    ContextMore,
    FileList,
    DiffMode,
}

fn diff_mode_lines(
    selected: usize,
    current: Option<DiffViewKind>,
    terminal_bg: TerminalBg,
) -> Vec<Line> {
    let mut lines = Vec::new();
    for (i, kind) in DIFF_VIEW_KINDS.iter().enumerate() {
        let selected_row = i == selected;
        let mut name_style = diff_file_header_style();
        let mut mark_style = diff_note_style();
        if selected_row {
            let bg = Some(terminal_bg.row_highlight());
            name_style.bg = bg;
            mark_style.bg = bg;
        }
        let mut line = Line::new();
        line.push_styled(if selected_row { "> " } else { "  " }, name_style);
        line.push_styled(kind.label(), name_style);
        if current == Some(*kind) {
            line.push_styled("  *", mark_style);
        }
        lines.push(line);
    }
    lines
}

fn list_page_size(size: Size) -> usize {
    let rows = size.height.0.saturating_sub(1).max(1);
    usize::try_from(rows).unwrap_or(1)
}

fn is_dismissable_status(status: &str) -> bool {
    status.starts_with(PATTERN_NOT_FOUND)
        || status == NO_PREVIOUS_SEARCH
        || status == INVALID_PATTERN
        || status == NOT_SET
        || status.starts_with("context ")
        || is_case_mode_status(status)
}

fn key_action(key: &KeyEvent) -> Option<KeyAction> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(
            key.code,
            KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Char('\u{c}')
        )
    {
        return Some(KeyAction::Redraw);
    }
    match key.code {
        KeyCode::Esc => Some(KeyAction::DismissMessage),
        KeyCode::Char('-') => Some(KeyAction::Dash),
        KeyCode::Char('q') => Some(KeyAction::Quit),
        KeyCode::Char('h') => Some(KeyAction::Help),
        KeyCode::Char('r') => Some(KeyAction::Refresh),
        KeyCode::Char('/') => Some(KeyAction::SearchForward),
        KeyCode::Char('?') => Some(KeyAction::SearchBackward),
        KeyCode::Char('n') => Some(KeyAction::SearchNext),
        KeyCode::Char('N') => Some(KeyAction::SearchPrev),
        KeyCode::Char('m') => Some(KeyAction::SetMark),
        KeyCode::Char('\'') => Some(KeyAction::GotoMark),
        KeyCode::Char('j') | KeyCode::Down | KeyCode::Enter => Some(KeyAction::ScrollDown),
        KeyCode::Char('k') | KeyCode::Up => Some(KeyAction::ScrollUp),
        KeyCode::Char('g') | KeyCode::Home => Some(KeyAction::ScrollTop),
        KeyCode::Char('G') | KeyCode::End => Some(KeyAction::ScrollBottom),
        KeyCode::Char(' ') | KeyCode::PageDown => Some(KeyAction::ScrollPageDown),
        KeyCode::PageUp => Some(KeyAction::ScrollPageUp),
        KeyCode::Char('s')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(KeyAction::FileList)
        }
        KeyCode::Char('d')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(KeyAction::DiffMode)
        }
        KeyCode::Char('u')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(KeyAction::ContextLess)
        }
        KeyCode::Char('U')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(KeyAction::ContextMore)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::capture::DiffViewKind;
    use crate::event_loop::diff_mode_lines;
    use crate::terminal_bg::TerminalBg;

    #[test]
    fn test_diff_mode_lines_mark_current_and_selection() {
        let lines = diff_mode_lines(1, Some(DiffViewKind::Default), TerminalBg::Dark);
        assert_eq!(3, lines.len());
        assert_eq!("  default diff  *", lines[0].plain_text());
        assert_eq!("> diff -c .", lines[1].plain_text());
        assert_eq!("  diff -r prev", lines[2].plain_text());
    }
}
