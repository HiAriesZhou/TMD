//! Static Markdown in the primary terminal screen. Only visible lines are
//! painted; the complete layout is kept in memory for wheel/keyboard scrolling.
use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crossterm::{cursor, event, terminal};
use event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use tmd_core::{Document, Line, LineSize, RenderOptions, Theme, truncate};

use crate::cli::{Cli, InputFormat, ReaderMode};
use crate::clipboard::{self, CopyError};
use crate::reader_copy::CodePicker;
use crate::reader_outline::Outline;
use crate::reader_search::Search;
use crate::terminal_io::{RESET, direct_kaku, write_all_cancelled};
use crate::{ansi, large_headings, reader_native};

pub fn eligible(cli: &Cli) -> bool {
    cfg!(unix)
        && cli.file.is_some()
        && cli.from == InputFormat::Text
        && !cli.ansi
        && io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
        && terminal::size().is_ok_and(|(cols, rows)| cols >= 4 && rows >= 4)
}

fn layout(cli: &Cli, document: &Document, size: (u16, u16)) -> Vec<Line> {
    let width = cli.width.unwrap_or(size.0).min(size.0).max(1);
    let large = size.1 >= 4 && large_headings::enabled(cli, true, Some(size), width);
    document.layout(
        width,
        RenderOptions {
            large_headings: large,
        },
    )
}

fn header() -> String {
    std::env::args_os()
        .enumerate()
        .map(|(i, arg)| {
            let arg = if i == 0 {
                std::ffi::OsString::from("tmd")
            } else {
                arg
            };
            let text: String = arg
                .to_string_lossy()
                .chars()
                .filter(|ch| !ch.is_control())
                .collect();
            if text.contains(char::is_whitespace) {
                format!("{text:?}")
            } else {
                text
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn visible_end(lines: &[Line], start: usize, height: usize) -> usize {
    let mut used = 0;
    let mut end = start;
    for line in lines.iter().skip(start) {
        if used + line.physical_height() > height {
            break;
        }
        used += line.physical_height();
        end += 1;
    }
    end
}

fn last_start(lines: &[Line], height: usize) -> usize {
    let mut start = lines.len();
    let mut used = 0;
    while start > 0 && used + lines[start - 1].physical_height() <= height {
        start -= 1;
        used += lines[start].physical_height();
    }
    // When no row fits, retain a valid logical anchor for the next resize.
    start.min(lines.len().saturating_sub(1))
}

fn paint(
    lines: &[Line],
    start: usize,
    size: (u16, u16),
    title: &str,
    notice: &str,
    highlight: Option<usize>,
) -> (String, usize) {
    let (cols, rows) = size;
    let mut out = String::from("\x1b[?2026h");
    // Erasing alone does not reset DEC line attributes from an earlier frame.
    for row in 0..rows {
        out.push_str(&format!("\x1b[{};1H\x1b#5\x1b[2K", row + 1));
    }
    let body = rows.saturating_sub(2) as usize;
    let end = visible_end(lines, start, body);
    if rows >= 3 {
        out.push_str("\x1b[1;1H");
        out.push_str(truncate(title, cols.saturating_sub(1) as usize));
        let mut row = 2;
        for (index, line) in lines[start..end].iter().enumerate() {
            let mut text = String::new();
            if highlight == Some(start + index) {
                let mut marked = line.clone();
                for span in &mut marked.spans {
                    span.style.bold = true;
                    span.style.underline = true;
                }
                ansi::write_line(&mut text, &marked, true);
            } else {
                ansi::write_line(&mut text, line, true);
            }
            match line.size {
                LineSize::Normal => {
                    out.push_str(&format!("\x1b[{row};1H{text}"));
                    row += 1;
                }
                LineSize::Double => {
                    out.push_str(&format!(
                        "\x1b[{row};1H\x1b#3{text}\x1b[{};1H\x1b#4{text}",
                        row + 1
                    ));
                    row += 2;
                }
            }
        }
    }
    let footer = if rows < 3 || cols < 4 {
        "q 退出 · 窗口过小".into()
    } else {
        let position = if lines.is_empty() {
            "空文档"
        } else if start == 0 {
            "开头"
        } else if end == lines.len() {
            "末尾"
        } else {
            "阅读中"
        };
        format!(
            "{position}{notice} · / 搜索 · o 目录 · c 复制 · ? 帮助 · q/Esc 返回 · 滚轮/↑↓ · Home/End"
        )
    };
    if rows > 0 {
        out.push_str(&format!(
            "\x1b[{rows};1H\x1b#5\x1b[0m{}",
            truncate(&footer, cols.saturating_sub(1) as usize)
        ));
    }
    out.push_str("\x1b[?2026l");
    (out, end)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Exit,
    Interrupt,
    Help,
    Up,
    Down,
    Top,
    Bottom,
    PageUp,
    PageDown,
    Search,
    Next,
    Previous,
    Copy,
    Outline,
    Confirm,
}

fn key_action(key: KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
        return Some(Action::Interrupt);
    }
    if key.modifiers.intersects(
        KeyModifiers::CONTROL
            | KeyModifiers::ALT
            | KeyModifiers::SUPER
            | KeyModifiers::HYPER
            | KeyModifiers::META,
    ) {
        return None;
    }
    Some(match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Exit,
        KeyCode::Char('?') => Action::Help,
        KeyCode::Char('/') => Action::Search,
        KeyCode::Char('c') => Action::Copy,
        KeyCode::Char('o') => Action::Outline,
        KeyCode::Enter => Action::Confirm,
        KeyCode::Char('n') => Action::Next,
        KeyCode::Char('N') => Action::Previous,
        KeyCode::Up | KeyCode::Char('k') => Action::Up,
        KeyCode::Down | KeyCode::Char('j') => Action::Down,
        KeyCode::Home => Action::Top,
        KeyCode::End => Action::Bottom,
        KeyCode::PageDown | KeyCode::Char(' ') => Action::PageDown,
        KeyCode::PageUp | KeyCode::Char('b') => Action::PageUp,
        _ => return None,
    })
}

fn help_lines(width: u16) -> Vec<Line> {
    use tmd_core::{Span, Style};
    [
        "阅读帮助",
        "",
        "滚轮 / ↑↓ / j k：上下阅读",
        "空格 / PageDown：下一页",
        "b / PageUp：上一页",
        "Home / End：文章首尾",
        "Shift + 拖选：终端原生选择（Kaku）",
        "?：打开或关闭帮助",
        "/：搜索正文（区分大小写，Enter 确认）",
        "n / N：下一处 / 上一处匹配",
        "c：选择代码块，Enter 复制内容，Esc 取消",
        "o：打开标题目录，Enter 跳转，Esc 取消",
        "搜索中 Esc：取消；Ctrl-U：清空查询",
        "q / Esc：关闭帮助；阅读时返回命令行",
        "Ctrl-C：中断并返回命令行",
    ]
    .into_iter()
    .map(|text| Line {
        spans: vec![Span::new(
            truncate(text, width.saturating_sub(1) as usize),
            Style::default(),
        )],
        src: 0..0,
        size: LineSize::Normal,
    })
    .collect()
}

fn checked_write(out: &mut impl Write, bytes: &[u8], cancelled: &AtomicUsize) -> io::Result<()> {
    write_all_cancelled(out, bytes, cancelled)?;
    out.flush()
}

#[cfg(unix)]
struct Session {
    cancelled: Arc<AtomicUsize>,
    hooks: Vec<signal_hook::SigId>,
    original: nix::sys::termios::Termios,
    raw: bool,
    painted: bool,
    incomplete: bool,
    rows: u16,
    exit_row: u16,
}

#[cfg(unix)]
impl Session {
    fn start(rows: u16) -> io::Result<Self> {
        let mut session = Self {
            cancelled: Arc::new(AtomicUsize::new(0)),
            hooks: Vec::new(),
            original: nix::sys::termios::tcgetattr(io::stdin())?,
            raw: false,
            painted: false,
            incomplete: false,
            rows,
            exit_row: rows.saturating_sub(1),
        };
        for signal in [
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
            signal_hook::consts::SIGQUIT,
        ] {
            session.hooks.push(signal_hook::flag::register_usize(
                signal,
                session.cancelled.clone(),
                signal as usize,
            )?);
        }
        terminal::enable_raw_mode()?;
        session.raw = true;
        Ok(session)
    }

    fn restore_input(&mut self) -> io::Result<()> {
        if self.raw {
            let result = terminal::disable_raw_mode();
            nix::sys::termios::tcsetattr(
                io::stdin(),
                nix::sys::termios::SetArg::TCSANOW,
                &self.original,
            )?;
            self.raw = false;
            result?;
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for Session {
    fn drop(&mut self) {
        let mut out = io::stdout().lock();
        let _ = write!(out, "{RESET}\x1b[?2026l");
        if self.painted {
            if self.incomplete {
                for row in 1..=self.rows {
                    let _ = write!(out, "\x1b[{row};1H\x1b#5\x1b[2K");
                }
            }
            let _ = write!(out, "\x1b[{};1H\x1b#5\x1b[2K", self.rows.max(1));
            let _ = write!(out, "\x1b[{};1H\x1b#5\r", self.exit_row.saturating_add(1));
            let _ = crossterm::execute!(
                out,
                event::DisableMouseCapture,
                event::DisableBracketedPaste,
                cursor::Show
            );
        }
        let _ = out.flush();
        drop(out);
        let _ = self.restore_input();
        for hook in self.hooks.drain(..) {
            signal_hook::low_level::unregister(hook);
        }
    }
}

#[cfg(unix)]
pub fn run(cli: &Cli, md: &str, theme: &Theme, mode: ReaderMode) -> io::Result<u8> {
    run_with_copy(cli, md, theme, mode, clipboard::copy)
}

#[cfg(unix)]
fn run_with_copy(
    cli: &Cli,
    md: &str,
    theme: &Theme,
    mode: ReaderMode,
    mut copy: impl FnMut(&str) -> Result<(), CopyError>,
) -> io::Result<u8> {
    let mut size = terminal::size()?;
    let document = Document::new(md, theme.clone());
    let mut lines = layout(cli, &document, size);
    let content_height: usize = lines.iter().map(Line::physical_height).sum();
    let title = header();
    let mut session = Session::start(size.1)?;
    // This is screen scrolling, not an alternate buffer or a history purge.
    // Using the live cursor row also handles commands started near the bottom.
    let (_, row) = cursor::position()?;
    if row > 0 {
        crossterm::execute!(io::stdout(), terminal::ScrollUp(row))?;
    }
    crossterm::execute!(io::stdout(), cursor::MoveTo(0, 0))?;
    let mut queued = VecDeque::new();
    let token = reader_native::token();
    let mut notice = String::new();
    if mode == ReaderMode::Native {
        let capacity = if direct_kaku() {
            reader_native::probe(&mut io::stdout().lock(), &token, &mut queued)?
        } else {
            None
        };
        let required = content_height.saturating_add(2);
        if capacity.is_some_and(|capacity| capacity >= required) && queued.is_empty() {
            session.restore_input()?;
            let mut out = io::stdout().lock();
            writeln!(
                out,
                "{}",
                truncate(&title, size.0.saturating_sub(1) as usize)
            )?;
            out.flush()?;
            drop(out);
            let result = if lines.iter().any(|line| line.size == LineSize::Double) {
                large_headings::write(&lines)
            } else {
                checked_write(
                    &mut io::stdout().lock(),
                    ansi::to_ansi(&lines, true).as_bytes(),
                    &session.cancelled,
                )
            };
            let signal = session.cancelled.load(Ordering::SeqCst);
            if signal != 0 {
                return Ok((128 + signal) as u8);
            }
            result?;
            reader_native::notify(&mut io::stdout().lock(), "show", &token)?;
            return Ok(0);
        }
        notice = " · 原生定位不可用，使用内置阅读".into();
    }
    let result: io::Result<()> = (|| {
        session.painted = true;
        crossterm::execute!(
            io::stdout(),
            cursor::MoveTo(0, 0),
            cursor::Hide,
            event::EnableMouseCapture,
            event::EnableBracketedPaste
        )?;
        let mut start = 0;
        let mut help = false;
        let mut help_start = 0;
        let mut search = Search::default();
        let mut picker = CodePicker::default();
        let mut outline = Outline::default();
        loop {
            if session.cancelled.load(Ordering::SeqCst) != 0 {
                break;
            }
            session.rows = size.1;
            session.exit_row = size.1.saturating_sub(1);
            session.incomplete = true;
            let help_rows = help_lines(size.0);
            let code_rows = if picker.open {
                picker.rows(size.0)
            } else {
                Vec::new()
            };
            let outline_rows = if outline.open {
                outline.rows(document.headings(), size.0)
            } else {
                Vec::new()
            };
            let (view, position, heading) = if outline.open {
                (
                    &outline_rows,
                    outline.top(size.1.saturating_sub(2) as usize),
                    "tmd · 标题目录（Enter 跳转，q/Esc 返回）",
                )
            } else if picker.open {
                (
                    &code_rows,
                    picker.top(size.1.saturating_sub(2) as usize),
                    "tmd · 代码块（Enter 复制，q/Esc 返回）",
                )
            } else if help {
                (&help_rows, help_start, "tmd · 帮助（q/Esc 返回阅读）")
            } else {
                (&lines, start, title.as_str())
            };
            let highlight = if outline.open {
                Some(outline.selected)
            } else if picker.open {
                Some(picker.selected)
            } else if !help && search.input.is_none() {
                search.current().and_then(|m| Document::match_line(view, m))
            } else {
                None
            };
            let combined_notice = if outline.open {
                format!(
                    " · {}/{} · Enter 跳转 · Esc 取消",
                    outline.selected + 1,
                    document.headings().len()
                )
            } else if picker.open {
                format!(
                    " · {}/{} · Enter 复制 · Esc 取消{}",
                    picker.selected + 1,
                    document.code_blocks().len(),
                    picker.notice
                )
            } else {
                format!(
                    "{}{}{notice}{}",
                    outline.notice,
                    picker.notice,
                    search.notice()
                )
            };
            let (mut frame, end) =
                paint(view, position, size, heading, &combined_notice, highlight);
            if let Some(query) = &search.input {
                // Place the actual cursor at the query so the system IME can
                // anchor its candidate window. One final column stays free.
                frame.truncate(frame.len() - "\x1b[?2026l".len());
                if size.1 > 0 {
                    let cols = size.0.saturating_sub(2) as usize;
                    let (text, col) = query.visible(cols);
                    frame.push_str(&format!(
                        "\x1b[{};1H\x1b#5\x1b[0m\x1b[2K/{}\x1b[{};{}H\x1b[?25h",
                        size.1,
                        text,
                        size.1,
                        (col + 2).min(size.0.max(1) as usize)
                    ));
                }
                frame.push_str("\x1b[?2026l");
            } else {
                frame.truncate(frame.len() - "\x1b[?2026l".len());
                frame.push_str("\x1b[?25l\x1b[?2026l");
            }
            checked_write(
                &mut io::stdout().lock(),
                frame.as_bytes(),
                &session.cancelled,
            )?;
            session.incomplete = false;
            // Empty files retain the quick-return behavior. Any rendered content
            // stays interactive even when a resize makes the whole file fit.
            if lines.is_empty() && queued.is_empty() && event::poll(Duration::ZERO)? {
                queued.push_back(event::read()?);
            }
            if lines.is_empty() && queued.is_empty() {
                session.exit_row = 1.min(size.1.saturating_sub(1));
                break;
            }
            let input = loop {
                if let Some(event) = queued.pop_front() {
                    break event;
                }
                if session.cancelled.load(Ordering::SeqCst) != 0 {
                    return Ok(());
                }
                if event::poll(Duration::from_millis(100))? {
                    break event::read()?;
                }
            };
            if let Event::Key(key) = input
                && key_action(key) == Some(Action::Interrupt)
            {
                session.cancelled.store(2, Ordering::SeqCst);
                break;
            }
            if outline.open {
                let count = document.headings().len();
                match &input {
                    Event::Key(key) => match key_action(*key) {
                        Some(Action::Exit) => outline.open = false,
                        Some(Action::Confirm) => {
                            if let Some(row) = document
                                .headings()
                                .get(outline.selected)
                                .and_then(|heading| Document::heading_line(&lines, heading))
                            {
                                start =
                                    row.min(last_start(&lines, size.1.saturating_sub(2) as usize));
                                outline.open = false;
                            }
                        }
                        Some(Action::Up) => outline.move_by(-1, count),
                        Some(Action::Down) => outline.move_by(1, count),
                        Some(Action::Top) => outline.selected = 0,
                        Some(Action::Bottom) => outline.move_by(isize::MAX, count),
                        Some(Action::PageUp) => {
                            outline.move_by(-(size.1.saturating_sub(2).max(1) as isize), count)
                        }
                        Some(Action::PageDown) => {
                            outline.move_by(size.1.saturating_sub(2).max(1) as isize, count)
                        }
                        _ => {}
                    },
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => outline.move_by(-3, count),
                        MouseEventKind::ScrollDown => outline.move_by(3, count),
                        _ => {}
                    },
                    _ => {}
                }
                if !matches!(input, Event::Resize(..)) {
                    continue;
                }
            }
            if picker.open {
                match &input {
                    Event::Key(key) => match key_action(*key) {
                        Some(Action::Exit) => picker.open = false,
                        Some(Action::Confirm) => picker.confirm(document.code_blocks(), &mut copy),
                        Some(Action::Up) => picker.move_by(-1),
                        Some(Action::Down) => picker.move_by(1),
                        Some(Action::Top) => picker.selected = 0,
                        Some(Action::Bottom) => picker.move_by(isize::MAX),
                        Some(Action::PageUp) => {
                            picker.move_by(-(size.1.saturating_sub(2).max(1) as isize))
                        }
                        Some(Action::PageDown) => {
                            picker.move_by(size.1.saturating_sub(2).max(1) as isize)
                        }
                        _ => {}
                    },
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => picker.move_by(-3),
                        MouseEventKind::ScrollDown => picker.move_by(3),
                        _ => {}
                    },
                    _ => {}
                }
                if !matches!(input, Event::Resize(..)) {
                    continue;
                }
            }
            if search.input.is_some() {
                match &input {
                    Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                        KeyCode::Esc => {
                            search.input = None;
                            start = search.saved_start;
                        }
                        KeyCode::Enter => {
                            if search.confirm(&document)
                                && let Some(row) = search
                                    .current()
                                    .and_then(|m| Document::match_line(&lines, m))
                            {
                                start =
                                    row.min(last_start(&lines, size.1.saturating_sub(2) as usize));
                            }
                        }
                        _ => search.input.as_mut().unwrap().key(*key),
                    },
                    Event::Paste(text) => search.input.as_mut().unwrap().insert(text),
                    Event::Resize(..) => {}
                    _ => continue,
                }
                if !matches!(input, Event::Resize(..)) {
                    continue;
                }
            }
            let max = last_start(view, size.1.saturating_sub(2) as usize);
            let position = if help { &mut help_start } else { &mut start };
            match input {
                Event::Key(key) => match key_action(key) {
                    Some(Action::Interrupt) => {
                        session.cancelled.store(2, Ordering::SeqCst);
                        break;
                    }
                    Some(Action::Exit) if help => help = false,
                    Some(Action::Exit) if key.code == KeyCode::Esc && !search.query.is_empty() => {
                        search.clear()
                    }
                    Some(Action::Exit) => break,
                    Some(Action::Search) if !help => search.open(start),
                    Some(Action::Copy) if !help => {
                        picker.show(document.code_blocks(), &lines[start..]);
                    }
                    Some(Action::Outline) if !help => {
                        outline.show(
                            document.headings(),
                            lines.get(start).map_or(0, |line| line.src.start),
                        );
                    }
                    Some(Action::Copy | Action::Outline | Action::Confirm) => continue,
                    Some(Action::Next | Action::Previous) if !help => {
                        search.advance(key.code == KeyCode::Char('N'));
                        if let Some(row) = search
                            .current()
                            .and_then(|m| Document::match_line(&lines, m))
                        {
                            start = row.min(max);
                        }
                    }
                    Some(Action::Search | Action::Next | Action::Previous) => continue,
                    Some(Action::Help) => {
                        help = !help;
                        help_start = 0;
                    }
                    Some(Action::Up) => *position = position.saturating_sub(1),
                    Some(Action::Down) => *position = position.saturating_add(1).min(max),
                    Some(Action::Top) => *position = 0,
                    Some(Action::Bottom) => *position = max,
                    Some(Action::PageDown) => {
                        *position = end.saturating_sub(1).max(*position + 1).min(max)
                    }
                    Some(Action::PageUp) => {
                        *position = position.saturating_sub(size.1.saturating_sub(3) as usize)
                    }
                    None => continue,
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => *position = position.saturating_sub(3),
                    MouseEventKind::ScrollDown => *position = position.saturating_add(3).min(max),
                    _ => continue,
                },
                Event::Resize(cols, rows) => {
                    let anchor = lines.get(start).and_then(|line| {
                        line.spans
                            .iter()
                            .find_map(|s| s.text_ranges.first().cloned())
                    });
                    let source_anchor = lines.get(start).map_or(0, |line| line.src.start);
                    let saved_anchor = lines.get(search.saved_start).and_then(|line| {
                        line.spans
                            .iter()
                            .find_map(|s| s.text_ranges.first().cloned())
                    });
                    size = (cols, rows);
                    help_start = help_start.min(last_start(
                        &help_lines(cols),
                        rows.saturating_sub(2) as usize,
                    ));
                    lines = layout(cli, &document, size);
                    let max = last_start(&lines, rows.saturating_sub(2) as usize);
                    start = anchor
                        .as_ref()
                        .and_then(|range| {
                            Document::match_line(&lines, &(range.start..range.start + 1))
                        })
                        .or_else(|| {
                            lines
                                .iter()
                                .position(|line| line.src.start >= source_anchor)
                        })
                        .unwrap_or(0)
                        .min(max);
                    search.saved_start = saved_anchor
                        .as_ref()
                        .and_then(|range| {
                            Document::match_line(&lines, &(range.start..range.start + 1))
                        })
                        .unwrap_or(search.saved_start.min(max))
                        .min(max);
                }
                _ => continue,
            }
        }
        Ok(())
    })();
    let signal = session.cancelled.load(Ordering::SeqCst);
    if signal != 0 {
        return Ok((128 + signal) as u8);
    }
    result?;
    Ok(0)
}

#[cfg(not(unix))]
pub fn run(_: &Cli, _: &str, _: &Theme, _: ReaderMode) -> io::Result<u8> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmd_core::{Span, Style};
    fn row(size: LineSize) -> Line {
        Line {
            spans: vec![Span::new("中 é 👩‍💻", Style::default())],
            src: 0..20,
            size,
        }
    }
    #[test]
    fn modified_keys_cannot_trigger_reader_commands() {
        for code in [KeyCode::Char('q'), KeyCode::Char('?'), KeyCode::Down] {
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::ALT,
                KeyModifiers::SUPER,
            ] {
                assert_eq!(key_action(KeyEvent::new(code, modifiers)), None);
            }
        }
        assert_eq!(
            key_action(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Interrupt)
        );
        assert_eq!(
            key_action(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            Some(Action::Help)
        );
    }
    #[test]
    fn double_rows_are_never_split_by_a_viewport() {
        let lines = vec![
            row(LineSize::Normal),
            row(LineSize::Double),
            row(LineSize::Normal),
        ];
        assert_eq!(visible_end(&lines, 0, 2), 1);
        assert_eq!(visible_end(&lines, 1, 2), 2);
        assert_eq!(last_start(&lines, 2), 2);
        let (frame, end) = paint(&lines, 0, (40, 4), "tmd test.md", "", None);
        assert_eq!(end, 1);
        assert!(!frame.contains("\x1b#3"));
        let (frame, end) = paint(&lines, 1, (40, 4), "tmd test.md", "", None);
        assert_eq!(end, 2);
        assert_eq!(frame.matches("\x1b#3").count(), 1);
        assert_eq!(frame.matches("\x1b#4").count(), 1);
        assert!(!frame.contains("?1049"));
        assert!(!frame.contains("\x1b[3J"));
    }
    #[test]
    fn empty_viewport_keeps_a_valid_last_logical_anchor() {
        let lines = [
            row(LineSize::Normal),
            row(LineSize::Double),
            row(LineSize::Normal),
        ];
        assert_eq!(last_start(&lines, 0), 2);
        assert_eq!(last_start(&[row(LineSize::Double)], 1), 0);
        assert_eq!(last_start(&[], 0), 0);
    }
    #[test]
    fn resize_can_show_a_tiny_window_without_leaking_body_rows() {
        let (frame, end) = paint(&[row(LineSize::Double)], 0, (4, 1), "header", "", None);
        assert_eq!(end, 0);
        assert!(!frame.contains("\x1b#3"));
        assert!(!frame.contains("header"));
    }
}
