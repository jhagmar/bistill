//! The process terminal.
//!
//! [`Terminal`] enters the alternate screen and raw mode. Drop restores the
//! previous screen and console mode. A panic runs that restore before
//! unwinding.

use crate::backend::{Backend, Event, KeyCode, MouseButton, Wheel};
use crate::buffer::{Buffer, Glyph};
use crate::layout::Rect;
use crate::style::{Color, Style};
use crate::terminal_os;
use std::io;
use std::panic::{PanicHookInfo, set_hook, take_hook};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

#[cfg(all(unix, test))]
use std::fs::File;

pub(crate) const ENTER: &[u8] = b"\x1b[?1049h\x1b[?1000h\x1b[?1006h";
pub(crate) const LEAVE: &[u8] = b"\x1b[?1006l\x1b[?1000l\x1b[?1049l";

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>;

static PREVIOUS: Mutex<Option<Hook>> = Mutex::new(None);

/// The process terminal.
///
/// One value owns the console. [`Terminal::stdio`] claims standard input and
/// standard output.
pub struct Terminal {
    _hook: HookGuard,
    pending: Vec<u8>,
    previous: Buffer,
    screen: (u16, u16),
    console: terminal_os::Console,
}

struct HookGuard;

impl Drop for HookGuard {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            remove_hook();
        }
    }
}

impl Terminal {
    /// Claims standard input and standard output.
    pub fn stdio() -> io::Result<Self> {
        Ok(Self::new(terminal_os::Console::stdio()?))
    }

    #[cfg(all(unix, test))]
    pub(crate) fn pair(input: File, output: File) -> io::Result<Self> {
        Ok(Self::new(terminal_os::from_files(input, output)?))
    }

    fn new(mut console: terminal_os::Console) -> Self {
        install_hook();
        console.write(ENTER);
        let screen = console.size();
        Terminal {
            _hook: HookGuard,
            pending: Vec::new(),
            previous: Buffer::empty(screen.0, screen.1),
            screen,
            console,
        }
    }
}

impl Backend for Terminal {
    fn size(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: self.screen.0,
            height: self.screen.1,
        }
    }

    fn draw(&mut self, buffer: &Buffer) {
        let bytes = paint(&self.previous, buffer);
        self.console.write(&bytes);
        self.previous = buffer.clone();
    }

    fn poll(&mut self, timeout: Duration) -> Option<Event> {
        let screen = self.console.size();
        if screen != self.screen {
            self.screen = screen;
            Some(Event::Resize {
                width: screen.0,
                height: screen.1,
            })
        } else {
            self.next_event(timeout)
        }
    }
}

impl Terminal {
    fn next_event(&mut self, timeout: Duration) -> Option<Event> {
        match decode(&mut self.pending, false) {
            Some(event) => Some(event),
            None => self.read_event(timeout),
        }
    }

    fn read_event(&mut self, timeout: Duration) -> Option<Event> {
        if self.console.wait(timeout) {
            let mut buf = [0; 128];
            let count = self.console.read(&mut buf);
            self.pending.extend_from_slice(&buf[..count]);
            decode(&mut self.pending, false)
        } else {
            decode(&mut self.pending, true)
        }
    }
}

fn install_hook() {
    let previous = take_hook();
    *lock_previous() = Some(previous);
    set_hook(Box::new(panic_hook));
}

pub(crate) fn remove_hook() {
    let previous = lock_previous().take();
    if let Some(previous) = previous {
        let _installed = take_hook();
        set_hook(previous);
    }
}

fn panic_hook(info: &PanicHookInfo<'_>) {
    terminal_os::disarm();
    let hook = lock_previous().take();
    if let Some(hook) = hook {
        hook(info);
        *lock_previous() = Some(hook);
    }
}

fn lock_previous() -> MutexGuard<'static, Option<Hook>> {
    PREVIOUS.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn paint(previous: &Buffer, next: &Buffer) -> Vec<u8> {
    let mut out = Vec::new();
    let same = previous.width() == next.width() && previous.height() == next.height();
    if !same {
        out.extend_from_slice(b"\x1b[2J");
    }
    let blank = Buffer::empty(next.width(), next.height());
    let prior = if same { previous } else { &blank };
    if next.width() > 0 {
        paint_cells(&mut out, prior, next);
    }
    out
}

struct Pen {
    cursor: Option<(u16, u16)>,
    style: Option<Style>,
}

fn paint_cells(out: &mut Vec<u8>, prior: &Buffer, next: &Buffer) {
    let width = usize::from(next.width());
    let mut pen = Pen {
        cursor: None,
        style: None,
    };
    for (index, cell) in next.cells().iter().enumerate() {
        let x = (index % width) as u16;
        let y = (index / width) as u16;
        match &cell.glyph {
            Glyph::Tail => {}
            Glyph::Single(text) => {
                if &prior.cells()[index] != cell {
                    write_cell(out, &mut pen, x, y, text, cell.style, 1);
                }
            }
            Glyph::Wide(text) => {
                let tail = index + 1;
                let tail_changed =
                    tail < next.cells().len() && prior.cells()[tail] != next.cells()[tail];
                if &prior.cells()[index] != cell || tail_changed {
                    write_cell(out, &mut pen, x, y, text, cell.style, 2);
                }
            }
        }
    }
}

fn write_cell(
    out: &mut Vec<u8>,
    pen: &mut Pen,
    x: u16,
    y: u16,
    text: &str,
    style: Style,
    advance: u16,
) {
    if pen.cursor != Some((x, y)) {
        out.extend(format!("\x1b[{};{}H", u32::from(y) + 1, u32::from(x) + 1).bytes());
    }
    if pen.style != Some(style) {
        write_sgr(out, style);
        pen.style = Some(style);
    }
    out.extend(text.bytes());
    pen.cursor = Some((x.saturating_add(advance), y));
}

fn write_sgr(out: &mut Vec<u8>, style: Style) {
    out.extend_from_slice(b"\x1b[0");
    if style.bold {
        out.extend_from_slice(b";1");
    }
    if style.underline {
        out.extend_from_slice(b";4");
    }
    if style.reverse {
        out.extend_from_slice(b";7");
    }
    out.push(b';');
    push_code(out, color_code(style.fg, 30, 90, 39));
    out.push(b';');
    push_code(out, color_code(style.bg, 40, 100, 49));
    out.push(b'm');
}

fn color_code(color: Option<Color>, base: u8, bright: u8, default_code: u8) -> u8 {
    match color {
        None => default_code,
        Some(color) => {
            let index = color.ansi();
            if index < 8 {
                base + index
            } else {
                bright + index - 8
            }
        }
    }
}

fn push_code(out: &mut Vec<u8>, code: u8) {
    out.extend(code.to_string().bytes());
}

pub(crate) fn decode(pending: &mut Vec<u8>, flushed: bool) -> Option<Event> {
    if pending.is_empty() {
        None
    } else if pending[0] == 0x1b {
        decode_escape(pending, flushed)
    } else {
        decode_plain(pending, flushed)
    }
}

fn decode_escape(pending: &mut Vec<u8>, flushed: bool) -> Option<Event> {
    if pending.len() == 1 {
        unfinished(pending, flushed)
    } else if pending[1] == b'[' {
        decode_csi(pending, flushed)
    } else if pending[1] == b'O' {
        decode_ss3(pending, flushed)
    } else {
        pending.remove(0);
        Some(Event::Key(KeyCode::Esc))
    }
}

fn decode_csi(pending: &mut Vec<u8>, flushed: bool) -> Option<Event> {
    if pending.len() >= 3 && pending[2] == b'<' {
        decode_mouse(pending, flushed)
    } else if let Some(at) = csi_final(pending) {
        let event = if at == 2 { arrow(pending[at]) } else { None };
        pending.drain(..=at);
        event
    } else {
        unfinished(pending, flushed)
    }
}

fn decode_ss3(pending: &mut Vec<u8>, flushed: bool) -> Option<Event> {
    if pending.len() >= 3 {
        let event = arrow(pending[2]);
        pending.drain(..3);
        event
    } else {
        unfinished(pending, flushed)
    }
}

fn decode_mouse(pending: &mut Vec<u8>, flushed: bool) -> Option<Event> {
    let end = pending
        .iter()
        .skip(3)
        .position(|byte| *byte == b'M' || *byte == b'm');
    match end {
        Some(end) => {
            let at = end + 3;
            let body = pending[3..at].to_vec();
            let kind = pending[at];
            pending.drain(..=at);
            parse_mouse(&body, kind)
        }
        None => unfinished(pending, flushed),
    }
}

fn csi_final(pending: &[u8]) -> Option<usize> {
    pending
        .iter()
        .skip(2)
        .position(|byte| (0x40..=0x7e).contains(byte))
        .map(|at| at + 2)
}

fn unfinished(pending: &mut Vec<u8>, flushed: bool) -> Option<Event> {
    if flushed {
        pending.remove(0);
        Some(Event::Key(KeyCode::Esc))
    } else {
        None
    }
}

fn arrow(byte: u8) -> Option<Event> {
    let code = match byte {
        b'A' => Some(KeyCode::Up),
        b'B' => Some(KeyCode::Down),
        b'C' => Some(KeyCode::Right),
        b'D' => Some(KeyCode::Left),
        _ => None,
    };
    code.map(Event::Key)
}

fn parse_mouse(body: &[u8], kind: u8) -> Option<Event> {
    let text = std::str::from_utf8(body).ok()?;
    let mut parts = text.split(';');
    let button = parts.next()?.parse::<u16>().ok()?;
    let column = parts.next()?.parse::<u16>().ok()?.saturating_sub(1);
    let row = parts.next()?.parse::<u16>().ok()?.saturating_sub(1);
    match parts.next() {
        Some(_) => None,
        None => mouse_event(button, kind, column, row),
    }
}

fn mouse_event(button: u16, kind: u8, column: u16, row: u16) -> Option<Event> {
    match (button & 0x7f, kind) {
        (0, b'M') => Some(Event::Press {
            button: MouseButton::Left,
            column,
            row,
        }),
        (1, b'M') => Some(Event::Press {
            button: MouseButton::Middle,
            column,
            row,
        }),
        (2, b'M') => Some(Event::Press {
            button: MouseButton::Right,
            column,
            row,
        }),
        (0, b'm') => Some(Event::Release {
            button: MouseButton::Left,
            column,
            row,
        }),
        (1, b'm') => Some(Event::Release {
            button: MouseButton::Middle,
            column,
            row,
        }),
        (2, b'm') => Some(Event::Release {
            button: MouseButton::Right,
            column,
            row,
        }),
        (64, b'M') => Some(Event::Wheel {
            direction: Wheel::Up,
            column,
            row,
        }),
        (65, b'M') => Some(Event::Wheel {
            direction: Wheel::Down,
            column,
            row,
        }),
        _ => None,
    }
}

fn decode_plain(pending: &mut Vec<u8>, flushed: bool) -> Option<Event> {
    match pending[0] {
        b'\r' | b'\n' => {
            pending.remove(0);
            Some(Event::Key(KeyCode::Enter))
        }
        b'\t' => {
            pending.remove(0);
            Some(Event::Key(KeyCode::Tab))
        }
        0x08 | 0x7f => {
            pending.remove(0);
            Some(Event::Key(KeyCode::Backspace))
        }
        byte if byte < 0x20 => {
            pending.remove(0);
            None
        }
        _ => decode_text(pending, flushed),
    }
}

fn decode_text(pending: &mut Vec<u8>, flushed: bool) -> Option<Event> {
    let width = utf8_width(pending[0]);
    if pending.len() < width {
        if flushed {
            pending.remove(0);
        }
        None
    } else {
        take_char(pending, width)
    }
}

fn take_char(pending: &mut Vec<u8>, width: usize) -> Option<Event> {
    match std::str::from_utf8(&pending[..width]) {
        Ok(text) => {
            let event = text.chars().next().map(KeyCode::Char).map(Event::Key);
            let len = text.len();
            pending.drain(..len);
            event
        }
        Err(_) => {
            pending.remove(0);
            None
        }
    }
}

fn utf8_width(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte < 0xE0 {
        2
    } else if byte < 0xF0 {
        3
    } else {
        4
    }
}
