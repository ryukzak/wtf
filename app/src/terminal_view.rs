use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line as GridLine, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    ClipboardItem, Context, FocusHandle, FontWeight, HighlightStyle, Hsla, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollWheelEvent, StyledText, Task,
    Window, canvas, div, font, prelude::*, px, rgb,
};

use unicode_width::UnicodeWidthChar;

use crate::console_view::{Line, Tone};
use crate::theme::Theme;

/// The terminal uses the code font and size from Settings.
fn font_size() -> f32 {
    crate::preferences::code_size()
}

fn line_height() -> f32 {
    crate::preferences::terminal_line_height()
}
const PADDING: f32 = 8.;
const SCROLLBACK: usize = 10_000;

#[derive(Clone)]
struct Listener(UnboundedSender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.unbounded_send(event).ok();
    }
}

/// A running shell: the terminal state and the channel to its PTY.
struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Notifier,
    /// Writes the app's own lines into the terminal.
    parser: Processor,
    /// Lines held while a full-screen program owns the screen.
    pending: Vec<Line>,
    exited: bool,
    _events: Task<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.pty.0.send(Msg::Shutdown).ok();
    }
}

/// A terminal like Zed's: the user's login shell in a PTY, drawn cell by cell.
pub struct TerminalView {
    dir: String,
    session: Option<Session>,
    error: Option<String>,
    focus: FocusHandle,
    /// Fractions of a line left over from smooth scrolling.
    scroll_rest: f32,
    /// Where the grid starts on screen and how wide a cell is, from the last
    /// layout, to turn mouse positions into cells.
    grid: Rc<Cell<(gpui::Point<Pixels>, Pixels)>>,
    /// A drag selecting text is on.
    selecting: bool,
}

impl TerminalView {
    pub fn new(dir: String, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            dir,
            session: None,
            error: None,
            focus: cx.focus_handle(),
            scroll_rest: 0.,
            grid: Rc::new(Cell::new((gpui::Point::default(), px(7.)))),
            selecting: false,
        };
        view.start(cx);
        view
    }

    pub fn focus(&self, window: &mut Window) {
        self.focus.focus(window);
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        match spawn(&self.dir, cx) {
            Ok(session) => {
                self.session = Some(session);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn handle(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        match event {
            Event::PtyWrite(text) => session
                .pty
                .0
                .send(Msg::Input(text.into_bytes().into()))
                .ok(),
            Event::Exit | Event::ChildExit(_) => {
                session.exited = true;
                Some(())
            }
            _ => None,
        };
        // Lines held for a full-screen program show once it quits.
        self.announce(Vec::new(), cx);
        cx.notify();
    }

    /// Prints lines above the prompt without the shell noticing. Lines wait
    /// while a full-screen program owns the screen.
    pub fn announce(&mut self, lines: Vec<Line>, cx: &mut Context<Self>) {
        // A shell that exited shows nothing more.
        let Some(session) = self.session.as_mut().filter(|session| !session.exited) else {
            return;
        };
        session.pending.extend(lines);
        if session.pending.is_empty() {
            return;
        }
        let mut term = session.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let bytes = announcement(
            &session.pending,
            term.grid().cursor.point,
            term.screen_lines(),
            term.columns(),
        );
        session.parser.advance(&mut *term, &bytes);
        session.pending.clear();
        drop(term);
        cx.notify();
    }

    /// Types a command line into the shell and runs it: in a new shell when
    /// the last one exited, in place of anything half typed at the prompt.
    /// A full-screen program does not get it as keys: it is not run, and a
    /// note saying so shows once the program quits.
    pub fn run(&mut self, line: &str, cx: &mut Context<Self>) {
        if self.session.as_ref().is_none_or(|session| session.exited) {
            self.start(cx);
        }
        // A shell that failed to start shows its error instead.
        let Some(session) = self.session.as_mut().filter(|session| !session.exited) else {
            return;
        };
        if session.term.lock().mode().contains(TermMode::ALT_SCREEN) {
            session.pending.push(Line {
                text: format!("not run while a full-screen program was open: {line}"),
                tone: Tone::Error,
            });
            return;
        }
        // Ctrl-E and Ctrl-U clear what was typed at the prompt.
        self.write(format!("\x05\x15{line}\r").into_bytes());
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if let Some(session) = &self.session {
            let mut term = session.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
            drop(term);
            session.pty.0.send(Msg::Input(bytes.into())).ok();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if self.session.as_ref().is_none_or(|session| session.exited) {
            if keystroke.key == "enter" {
                self.start(cx);
                cx.stop_propagation();
            }
            return;
        }
        if modifiers.platform {
            // ⌘C copies the selection; without one it is the app's.
            if keystroke.key == "c"
                && let Some(text) = self
                    .session
                    .as_ref()
                    .and_then(|session| session.term.lock().selection_to_string())
            {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                cx.stop_propagation();
                return;
            }
            if keystroke.key == "v"
                && let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
            {
                self.paste(&text);
                cx.stop_propagation();
            }
            // ⌘A is the terminal's, though it has nothing to select all of;
            // it does not reach the review behind.
            if keystroke.key == "a" {
                cx.stop_propagation();
            }
            // Other Cmd shortcuts belong to the app.
            return;
        }
        let mode = match &self.session {
            Some(session) => *session.term.lock().mode(),
            None => return,
        };
        let Some(bytes) = key_bytes(keystroke, mode) else {
            return;
        };
        self.write(bytes);
        cx.stop_propagation();
    }

    /// The cell under a mouse position, and which half of it.
    fn cell_at(&self, position: gpui::Point<Pixels>, term: &Term<Listener>) -> (Point, Side) {
        let (origin, cell_width) = self.grid.get();
        let x = f32::from(position.x - origin.x) / f32::from(cell_width);
        let y = f32::from(position.y - origin.y) / line_height();
        let column = (x.max(0.) as usize).min(term.columns().saturating_sub(1));
        let row = (y.max(0.) as usize).min(term.screen_lines().saturating_sub(1));
        let line = row as i32 - term.grid().display_offset() as i32;
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        (Point::new(GridLine(line), Column(column)), side)
    }

    /// A click starts a selection: by characters, words on a double click or
    /// lines on a triple one.
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        let Some(session) = &self.session else {
            return;
        };
        let mut term = session.term.lock();
        let (point, side) = self.cell_at(event.position, &term);
        let kind = match event.click_count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        term.selection = Some(Selection::new(kind, point, side));
        drop(term);
        self.selecting = true;
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let mut term = session.term.lock();
        let (point, side) = self.cell_at(event.position, &term);
        if let Some(selection) = &mut term.selection {
            selection.update(point, side);
        }
        drop(term);
        cx.notify();
    }

    fn paste(&self, text: &str) {
        let Some(session) = &self.session else {
            return;
        };
        let bracketed = session
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            self.write(format!("\x1b[200~{text}\x1b[201~").into_bytes());
        } else {
            self.write(text.into_bytes());
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        self.scroll_rest += event.delta.pixel_delta(px(line_height())).y / px(line_height());
        let lines = self.scroll_rest.trunc() as i32;
        self.scroll_rest -= lines as f32;
        if lines == 0 {
            return;
        }
        let mut term = session.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            // Full-screen programs such as less scroll on arrow keys.
            let arrow = if lines > 0 { "A" } else { "B" };
            let prefix = if term.mode().contains(TermMode::APP_CURSOR) {
                "\x1bO"
            } else {
                "\x1b["
            };
            let keys = format!("{prefix}{arrow}").repeat(lines.unsigned_abs() as usize);
            drop(term);
            session
                .pty
                .0
                .send(Msg::Input(keys.into_bytes().into()))
                .ok();
        } else {
            term.scroll_display(Scroll::Delta(lines));
        }
        cx.notify();
    }
}

fn spawn(dir: &str, cx: &mut Context<TerminalView>) -> Result<Session, String> {
    let (sender, mut events) = unbounded();
    let listener = Listener(sender);
    let options = tty::Options {
        working_directory: Some(dir.into()),
        env: HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            // BSD tools such as ls color their output only when asked.
            ("CLICOLOR".to_string(), "1".to_string()),
        ]),
        ..Default::default()
    };
    let size = WindowSize {
        num_lines: 24,
        num_cols: 80,
        cell_width: 8,
        cell_height: line_height() as u16,
    };
    let pty = tty::new(&options, size, 0).map_err(|error| error.to_string())?;
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let term = Term::new(config, &TermSize::new(80, 24), listener.clone());
    let term = Arc::new(FairMutex::new(term));
    let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)
        .map_err(|error| error.to_string())?;
    let pty = Notifier(event_loop.channel());
    event_loop.spawn();

    let events = cx.spawn(async move |this, cx| {
        while let Some(event) = events.next().await {
            if this.update(cx, |this, cx| this.handle(event, cx)).is_err() {
                break;
            }
        }
    });
    Ok(Session {
        term,
        pty,
        parser: Processor::new(),
        pending: Vec::new(),
        exited: false,
        _events: events,
    })
}

/// Escape codes that make room above the cursor's line, write `lines` there and
/// put the cursor back, scrolling the screen up when there is no room below.
fn announcement(lines: &[Line], cursor: Point, screen_lines: usize, columns: usize) -> Vec<u8> {
    if screen_lines < 2 {
        return Vec::new();
    }
    let bottom = screen_lines - 1;
    let width = columns.saturating_sub(1).max(1);
    let mut row = usize::try_from(cursor.line.0).unwrap_or(0).min(bottom);
    let mut out = String::new();
    // At most a screen minus the cursor's line at a time.
    for chunk in lines.chunks(bottom) {
        let count = chunk.len();
        let scroll = (row + count).saturating_sub(bottom);
        if scroll > 0 {
            out.push_str(&format!("\x1b[{scroll}S"));
            row -= scroll;
        }
        // Inserting lines pushes the cursor's line down, colors and all.
        out.push_str(&format!("\x1b[{};1H\x1b[{count}L", row + 1));
        for (i, line) in chunk.iter().enumerate() {
            // Cut to the screen's width in cells, so a wide character never
            // wraps the line onto the next row.
            let mut cells = 0;
            let text: String = line
                .text
                .chars()
                .filter(|c| !c.is_control())
                .take_while(|&c| {
                    cells += UnicodeWidthChar::width(c).unwrap_or(0);
                    cells <= width
                })
                .collect();
            out.push_str(&format!(
                "\x1b[{};1H{}\x1b[0m",
                row + i + 1,
                colored(&text, line.tone)
            ));
        }
        row += count;
    }
    out.push_str(&format!("\x1b[{};{}H", row + 1, cursor.column.0 + 1));
    out.into_bytes()
}

/// An announced line in color, as a shell prompt would show it: a dim `$`,
/// the program in green, or red when it failed, `cd`'s directory in blue,
/// the time it took dim, and a failure's output in red.
fn colored(text: &str, tone: Tone) -> String {
    let Some(command) = text.strip_prefix("$ ") else {
        return match tone {
            Tone::Plain => format!("\x1b[2m{text}"),
            Tone::Error => format!("\x1b[31m{text}"),
        };
    };
    let (command, time) = match command.rfind("  (") {
        Some(at) if command.ends_with(" ms)") => command.split_at(at),
        _ => (command, ""),
    };
    let (program, rest) = command.split_once(' ').unwrap_or((command, ""));
    let rest = match (program, tone) {
        ("cd", _) => format!("\x1b[34m{rest}"),
        (_, Tone::Error) => format!("\x1b[31m{rest}"),
        _ => rest.to_string(),
    };
    let program = match tone {
        Tone::Plain => format!("\x1b[1;32m{program}"),
        Tone::Error => format!("\x1b[1;31m{program}"),
    };
    format!("\x1b[2m$\x1b[0m {program}\x1b[0m {rest}\x1b[0;2m{time}")
}

/// The bytes a terminal expects for a key, or `None` for keys it ignores.
fn key_bytes(keystroke: &gpui::Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let modifiers = keystroke.modifiers;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let arrow = |code: &str| {
        let prefix = if app_cursor && !modifiers.shift {
            "\x1bO"
        } else {
            "\x1b["
        };
        Some(format!("{prefix}{code}").into_bytes())
    };
    let named: Option<&[u8]> = match keystroke.key.as_str() {
        "enter" => Some(b"\r"),
        "backspace" if modifiers.alt => Some(b"\x1b\x7f"),
        "backspace" => Some(b"\x7f"),
        "tab" if modifiers.shift => Some(b"\x1b[Z"),
        "tab" => Some(b"\t"),
        "escape" => Some(b"\x1b"),
        "delete" => Some(b"\x1b[3~"),
        "pageup" => Some(b"\x1b[5~"),
        "pagedown" => Some(b"\x1b[6~"),
        "up" => return arrow("A"),
        "down" => return arrow("B"),
        "right" if modifiers.alt => Some(b"\x1bf"),
        "left" if modifiers.alt => Some(b"\x1bb"),
        "right" => return arrow("C"),
        "left" => return arrow("D"),
        "home" => return arrow("H"),
        "end" => return arrow("F"),
        _ => None,
    };
    if let Some(bytes) = named {
        return Some(bytes.to_vec());
    }
    if modifiers.control {
        // Ctrl-` toggles the dock.
        let key = keystroke
            .key
            .chars()
            .next()
            .filter(|_| keystroke.key.len() == 1)?;
        return match key {
            'a'..='z' => Some(vec![key as u8 - b'a' + 1]),
            '[' => Some(vec![0x1b]),
            '\\' => Some(vec![0x1c]),
            ']' => Some(vec![0x1d]),
            ' ' | '@' => Some(vec![0]),
            _ => None,
        };
    }
    let text = keystroke.key_char.as_ref()?;
    Some(text.as_bytes().to_vec())
}

/// A row of text with style runs over byte ranges.
struct Row {
    text: String,
    runs: Vec<(std::ops::Range<usize>, HighlightStyle)>,
}

fn rows(term: &Term<Listener>, palette: &Palette, focused: bool) -> Vec<Row> {
    let content = term.renderable_content();
    let lines = term.screen_lines();
    let offset = content.display_offset as i32;
    let cursor = content.cursor.point;
    let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR) && offset == 0;
    let color = |color: Color| palette.color(color, content.colors);
    let selection = content.selection;

    let mut rows: Vec<Row> = (0..lines)
        .map(|_| Row {
            text: String::new(),
            runs: Vec::new(),
        })
        .collect();
    for indexed in content.display_iter {
        let line = indexed.point.line.0 + offset;
        let Some(row) = usize::try_from(line)
            .ok()
            .and_then(|line| rows.get_mut(line))
        else {
            continue;
        };
        let cell = indexed.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let (mut fg, mut bg) = (color(cell.fg), color(cell.bg));
        if cell.flags.contains(Flags::INVERSE) {
            (fg, bg) = (bg, fg);
        }
        let at_cursor = show_cursor && indexed.point == cursor;
        if at_cursor && focused {
            (fg, bg) = (palette.background, palette.cursor);
        }
        if cell.flags.contains(Flags::DIM) {
            fg.a *= 0.7;
        }
        if cell.flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        if selection.is_some_and(|range| range.contains(indexed.point)) {
            bg = palette.selection;
        }
        let start = row.text.len();
        row.text.push(cell.c);
        // Combining marks ride on their cell's character.
        if let Some(marks) = cell.zerowidth() {
            row.text.extend(marks);
        }
        let style = HighlightStyle {
            color: Some(fg),
            background_color: (bg != palette.background).then_some(bg),
            font_weight: cell.flags.contains(Flags::BOLD).then_some(FontWeight::BOLD),
            underline: cell
                .flags
                .intersects(Flags::ALL_UNDERLINES)
                .then(|| gpui::UnderlineStyle {
                    thickness: px(1.),
                    color: Some(fg),
                    wavy: false,
                }),
            ..Default::default()
        };
        let end = row.text.len();
        match row.runs.last_mut() {
            Some((range, last)) if *last == style && range.end == start => range.end = end,
            _ => row.runs.push((start..end, style)),
        }
    }
    rows
}

/// ANSI colors that follow the app theme.
struct Palette {
    ansi: [Hsla; 16],
    foreground: Hsla,
    background: Hsla,
    cursor: Hsla,
    selection: Hsla,
}

impl Palette {
    fn new(theme: &Theme, dark: bool) -> Self {
        // Mariana in dark mode and Breakers in light mode, like the diffs.
        let ansi = if dark {
            [
                0x22282d, 0xec5f66, 0x99c794, 0xf9ae58, 0x6699cc, 0xc695c6, 0x5fb4b4, 0xd8dee9,
                0x65737e, 0xf97b58, 0xa9d8a3, 0xfac863, 0x8fb2d9, 0xd7aedb, 0x80cbc4, 0xffffff,
            ]
        } else {
            [
                0x333333, 0xc9454c, 0x5d9a50, 0xc77d14, 0x3f7bbf, 0x9a5f9a, 0x3d9a9a, 0xbbbbbb,
                0x666666, 0xe0564a, 0x6fb062, 0xd99a2e, 0x5f97d8, 0xb47bb4, 0x4fb2b2, 0xeeeeee,
            ]
        };
        Self {
            ansi: ansi.map(|hex| rgb(hex).into()),
            foreground: theme.foreground,
            background: theme.background,
            cursor: theme.accent,
            selection: theme.selection,
        }
    }

    fn color(&self, color: Color, overrides: &alacritty_terminal::term::color::Colors) -> Hsla {
        let spec =
            |rgb: Rgb| -> Hsla { gpui::rgb(u32::from_be_bytes([0, rgb.r, rgb.g, rgb.b])).into() };
        match color {
            Color::Spec(rgb) => spec(rgb),
            Color::Named(name) => {
                if let Some(rgb) = overrides[name as usize] {
                    return spec(rgb);
                }
                match name {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
                    NamedColor::Background => self.background,
                    NamedColor::Cursor => self.cursor,
                    NamedColor::DimForeground => self.foreground.opacity(0.7),
                    name if (name as usize) < 16 => self.ansi[name as usize],
                    // The dim colors follow the normal ones.
                    name => {
                        self.ansi[(name as usize - NamedColor::DimBlack as usize) % 8].opacity(0.7)
                    }
                }
            }
            Color::Indexed(index) => {
                if let Some(rgb) = overrides[index as usize] {
                    return spec(rgb);
                }
                match index {
                    0..16 => self.ansi[index as usize],
                    16..232 => {
                        let index = index - 16;
                        let level = |n: u8| if n == 0 { 0 } else { n * 40 + 55 };
                        spec(Rgb {
                            r: level(index / 36),
                            g: level(index / 6 % 6),
                            b: level(index % 6),
                        })
                    }
                    _ => {
                        let gray = (index - 232) * 10 + 8;
                        spec(Rgb {
                            r: gray,
                            g: gray,
                            b: gray,
                        })
                    }
                }
            }
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let dark = theme.dark;
        let palette = Palette::new(&theme, dark);
        let focused = self.focus.is_focused(window);

        let mut root = div()
            .id("terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.selecting = false;
                    // A click without a drag leaves nothing selected.
                    if let Some(session) = &this.session {
                        let mut term = session.term.lock();
                        if term.selection.as_ref().is_some_and(Selection::is_empty) {
                            term.selection = None;
                        }
                    }
                    cx.notify();
                }),
            )
            .size_full()
            .p(px(PADDING))
            .overflow_hidden()
            .font_family(crate::preferences::code_font())
            .font_weight(crate::preferences::code_weight())
            .text_size(px(font_size()))
            .line_height(px(line_height()))
            .text_color(palette.foreground);

        let Some(session) = &self.session else {
            let error = self.error.clone().unwrap_or_default();
            return root.child(format!("Cannot start a shell: {error}"));
        };

        // Fits the terminal to the panel once the panel's size is known.
        let term = session.term.clone();
        let mut pty = Notifier(session.pty.0.clone());
        let grid = self.grid.clone();
        let font_id = window
            .text_system()
            .resolve_font(&font(crate::preferences::code_font()));
        let cell_width = window
            .text_system()
            .advance(font_id, px(font_size()), 'm')
            .map_or(px(7.), |size| size.width);
        root = root.child(
            canvas(
                move |bounds, window, _| {
                    grid.set((bounds.origin, cell_width));
                    let cols = (bounds.size.width / cell_width).floor().max(2.) as usize;
                    let lines = (bounds.size.height / px(line_height())).floor().max(1.) as usize;
                    let mut term = term.lock();
                    if term.columns() != cols || term.screen_lines() != lines {
                        term.resize(TermSize::new(cols, lines));
                        pty.on_resize(WindowSize {
                            num_lines: lines as u16,
                            num_cols: cols as u16,
                            cell_width: f32::from(cell_width) as u16,
                            cell_height: line_height() as u16,
                        });
                        window.refresh();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top(px(PADDING))
            .left(px(PADDING))
            .right(px(PADDING))
            .bottom(px(PADDING)),
        );

        let rows = rows(&session.term.lock(), &palette, focused);
        // Past an exited shell, its top row makes room for the note below.
        let skip = usize::from(session.exited);
        root = root.children(rows.into_iter().skip(skip).map(|row| {
            div()
                .h(px(line_height()))
                .whitespace_nowrap()
                .child(StyledText::new(row.text).with_highlights(row.runs))
        }));
        if session.exited {
            root = root.child(
                div()
                    .text_color(theme.secondary)
                    .child("[Process exited. Press Enter to start a new shell.]"),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use gpui::Keystroke;

    use super::*;

    fn bytes(key: &str, mode: TermMode) -> Option<Vec<u8>> {
        key_bytes(&Keystroke::parse(key).unwrap(), mode)
    }

    #[test]
    fn maps_control_and_named_keys() {
        let mode = TermMode::empty();
        assert_eq!(bytes("ctrl-c", mode), Some(vec![3]));
        assert_eq!(bytes("enter", mode), Some(b"\r".to_vec()));
        assert_eq!(bytes("backspace", mode), Some(vec![0x7f]));
        assert_eq!(bytes("up", mode), Some(b"\x1b[A".to_vec()));
        assert_eq!(bytes("up", TermMode::APP_CURSOR), Some(b"\x1bOA".to_vec()));
        assert_eq!(bytes("shift-tab", mode), Some(b"\x1b[Z".to_vec()));
        assert_eq!(bytes("ctrl-`", mode), None, "left for the dock toggle");
        assert_eq!(Keystroke::parse("ctrl-`").unwrap().key, "`");
    }

    #[test]
    fn shell_output_reaches_the_grid() {
        let (sender, _events) = unbounded();
        let listener = Listener(sender);
        let options = tty::Options {
            shell: Some(tty::Shell::new("/bin/sh".to_string(), Vec::new())),
            working_directory: Some(std::env::temp_dir()),
            ..Default::default()
        };
        let size = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 8,
            cell_height: 16,
        };
        let pty = tty::new(&options, size, 0).unwrap();
        let term = Term::new(Config::default(), &TermSize::new(80, 24), listener.clone());
        let term = Arc::new(FairMutex::new(term));
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false).unwrap();
        let pty = event_loop.channel();
        event_loop.spawn();
        pty.send(Msg::Input(b"echo wtf-$((40 + 2))\r".to_vec().into()))
            .unwrap();

        let theme = Theme::for_appearance(gpui::WindowAppearance::Dark);
        let palette = Palette::new(&theme, true);
        let mut screen = String::new();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let lines = rows(&term.lock(), &palette, true);
            screen = lines
                .iter()
                .map(|row| row.text.trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            if screen.lines().any(|line| line.trim() == "wtf-42") {
                pty.send(Msg::Shutdown).ok();
                return;
            }
        }
        panic!("no output on the screen:\n{screen}");
    }

    fn screen(term: &Term<alacritty_terminal::event::VoidListener>) -> Vec<String> {
        use alacritty_terminal::index::{Column, Line as Row};
        (0..term.screen_lines())
            .map(|line| {
                let row = &term.grid()[Row(line as i32)];
                let text: String = (0..term.columns()).map(|col| row[Column(col)].c).collect();
                text.trim_end().to_string()
            })
            .collect()
    }

    fn announce_into(screen_lines: usize, before: &str, lines: &[&str]) -> (Vec<String>, Point) {
        let listener = alacritty_terminal::event::VoidListener;
        let mut term = Term::new(
            Config::default(),
            &TermSize::new(20, screen_lines),
            listener,
        );
        let mut parser = Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new();
        parser.advance(&mut term, before.as_bytes());
        let lines: Vec<Line> = lines
            .iter()
            .map(|text| Line {
                text: text.to_string(),
                tone: Tone::Plain,
            })
            .collect();
        let bytes = announcement(&lines, term.grid().cursor.point, screen_lines, 20);
        parser.advance(&mut term, &bytes);
        (screen(&term), term.grid().cursor.point)
    }

    #[test]
    fn announcements_go_above_the_prompt() {
        let (screen, cursor) = announce_into(5, "old\r\n$ ls", &["$ git log", "$ git diff"]);
        assert_eq!(screen, ["old", "$ git log", "$ git diff", "$ ls", ""]);
        assert_eq!((cursor.line.0, cursor.column.0), (3, 4));
    }

    #[test]
    fn wide_characters_are_cut_by_cells_not_wrapped() {
        let wide = "日本語".repeat(4);
        let (screen, cursor) = announce_into(3, "$ ", &[wide.as_str()]);
        assert_eq!(screen[1], "$", "the prompt stays on its row");
        assert_eq!((cursor.line.0, cursor.column.0), (1, 2));
    }

    #[test]
    fn announcements_scroll_when_the_prompt_is_at_the_bottom() {
        let (screen, cursor) = announce_into(3, "a\r\nb\r\n$ ", &["$ git log"]);
        assert_eq!(screen, ["b", "$ git log", "$"]);
        assert_eq!((cursor.line.0, cursor.column.0), (2, 2));
    }

    #[test]
    fn announced_commands_keep_their_text_in_color() {
        let plain = |text: &str| -> String {
            let mut out = String::new();
            let mut escape = false;
            for c in text.chars() {
                match c {
                    '\x1b' => escape = true,
                    'm' if escape => escape = false,
                    _ if !escape => out.push(c),
                    _ => {}
                }
            }
            out
        };
        let line = "$ git log --oneline  (12 ms)";
        assert_eq!(plain(&colored(line, Tone::Plain)), line);
        assert!(colored(line, Tone::Plain).contains("\x1b[1;32mgit"));
        assert!(colored(line, Tone::Error).contains("\x1b[1;31mgit"));
        assert!(colored("$ cd /repo", Tone::Plain).contains("\x1b[34m/repo"));
        assert_eq!(plain(&colored("fatal: no", Tone::Error)), "fatal: no");
    }

    #[test]
    fn indexed_colors_cover_the_cube_and_grays() {
        let theme = Theme::for_appearance(gpui::WindowAppearance::Dark);
        let palette = Palette::new(&theme, true);
        let colors = alacritty_terminal::term::color::Colors::default();
        let white: Hsla = rgb(0xffffff).into();
        assert_eq!(palette.color(Color::Indexed(231), &colors), white);
        assert_eq!(palette.color(Color::Indexed(1), &colors), palette.ansi[1]);
        let gray = palette.color(Color::Indexed(232), &colors);
        assert!(gray.l < 0.05);
    }
}
