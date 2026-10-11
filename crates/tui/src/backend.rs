//! Draw a buffer and read one input event.
//!
//! [`TestBackend`] keeps the grid in memory and returns events you queued,
//! without sleeping. `timeout` is how long the caller is willing to wait.

use crate::buffer::{Buffer, Cell};
use crate::layout::Rect;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A key the terminal reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyCode {
    /// A Unicode scalar.
    Char(char),
    /// Enter.
    Enter,
    /// Backspace.
    Backspace,
    /// Escape.
    Esc,
    /// Tab.
    Tab,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButton {
    /// The primary button.
    Left,
    /// The middle button.
    Middle,
    /// The secondary button.
    Right,
}

/// Wheel direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wheel {
    /// Toward the top of the screen.
    Up,
    /// Toward the bottom of the screen.
    Down,
}

/// One input event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    /// A key press.
    Key(KeyCode),
    /// The terminal size, in cells.
    Resize {
        /// Columns.
        width: u16,
        /// Rows.
        height: u16,
    },
    /// A button press at a cell.
    Press {
        /// Which button.
        button: MouseButton,
        /// Column.
        column: u16,
        /// Row.
        row: u16,
    },
    /// A button release at a cell.
    Release {
        /// Which button.
        button: MouseButton,
        /// Column.
        column: u16,
        /// Row.
        row: u16,
    },
    /// A wheel step at a cell.
    Wheel {
        /// Direction of the step.
        direction: Wheel,
        /// Column.
        column: u16,
        /// Row.
        row: u16,
    },
}

/// Blocks until [`Park::unpark`] or until a descriptor this park owns is readable.
pub trait Park: Send + Sync {
    /// Waits until the generation is no longer `seen`. Returns the generation now.
    fn park(&self, seen: u64) -> u64;
    /// Wakes every thread blocked in [`Park::park`].
    fn unpark(&self);
}

/// A generation counter. [`Park::park`] waits until [`Park::unpark`] moves it.
pub struct Gate {
    state: Mutex<u64>,
    cv: Condvar,
}

impl Gate {
    /// A gate whose generation is zero.
    pub fn new() -> Arc<Self> {
        Arc::new(Gate {
            state: Mutex::new(0),
            cv: Condvar::new(),
        })
    }

    /// [`Park::park`] with a deadline. `None` means `limit` elapsed on `seen`.
    pub fn park_for(&self, seen: u64, limit: Duration) -> Option<u64> {
        let guard = self.state.lock().unwrap();
        let (guard, status) = self
            .cv
            .wait_timeout_while(guard, limit, |generation| *generation == seen)
            .unwrap();
        if status.timed_out() && *guard == seen {
            None
        } else {
            Some(*guard)
        }
    }
}

impl Park for Gate {
    fn park(&self, seen: u64) -> u64 {
        let guard = self.state.lock().unwrap();
        let guard = self
            .cv
            .wait_while(guard, |generation| *generation == seen)
            .unwrap();
        *guard
    }

    fn unpark(&self) {
        let mut guard = self.state.lock().unwrap();
        *guard = guard.wrapping_add(1);
        self.cv.notify_all();
    }
}

/// A surface that presents a buffer and yields events.
pub trait Backend: Send {
    /// The grid size in cells.
    fn size(&self) -> Rect;
    /// Present `buffer`. Cells equal to the previous frame stay as they were.
    fn draw(&mut self, buffer: &Buffer);
    /// The next event, or nothing when `timeout` elapses with the queue empty.
    fn poll(&mut self, timeout: Duration) -> Option<Event>;
    /// A park the input thread can block on without holding this backend.
    fn parking(&self) -> Arc<dyn Park>;
}

/// An in-memory grid. Tests push events and read cells.
#[derive(Clone)]
pub struct TestBackend {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
    writes: Vec<u32>,
    events: VecDeque<Event>,
    gate: Arc<Gate>,
}

impl TestBackend {
    /// A `width` by `height` grid of blank cells and an empty event queue.
    pub fn new(width: u16, height: u16) -> Self {
        let count = usize::from(width) * usize::from(height);
        TestBackend {
            width,
            height,
            cells: vec![Cell::blank(); count],
            writes: vec![0; count],
            events: VecDeque::new(),
            gate: Gate::new(),
        }
    }

    /// Queue `event` for a later [`Backend::poll`].
    pub fn push(&mut self, event: Event) {
        self.events.push_back(event);
    }

    /// The presented cell at `x`, `y`.
    pub fn cell(&self, x: u16, y: u16) -> Option<&Cell> {
        self.position(x, y).map(|index| &self.cells[index])
    }

    /// How many times `draw` has replaced this cell.
    pub fn writes(&self, x: u16, y: u16) -> Option<u32> {
        self.position(x, y).map(|index| self.writes[index])
    }

    fn position(&self, x: u16, y: u16) -> Option<usize> {
        if x < self.width && y < self.height {
            Some(usize::from(y) * usize::from(self.width) + usize::from(x))
        } else {
            None
        }
    }
}

impl PartialEq for TestBackend {
    fn eq(&self, other: &Self) -> bool {
        (
            self.width,
            self.height,
            &self.cells,
            &self.writes,
            &self.events,
        ) == (
            other.width,
            other.height,
            &other.cells,
            &other.writes,
            &other.events,
        )
    }
}

impl Eq for TestBackend {}

impl std::fmt::Debug for TestBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestBackend")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("cells", &self.cells)
            .field("writes", &self.writes)
            .field("events", &self.events)
            .finish()
    }
}

impl Backend for TestBackend {
    fn size(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }

    fn draw(&mut self, buffer: &Buffer) {
        if self.width != buffer.width() || self.height != buffer.height() {
            self.width = buffer.width();
            self.height = buffer.height();
            let count = usize::from(self.width) * usize::from(self.height);
            self.cells = vec![Cell::blank(); count];
            self.writes = vec![0; count];
        }
        for (index, cell) in buffer.cells().iter().enumerate() {
            if self.cells[index] != *cell {
                self.cells[index] = cell.clone();
                self.writes[index] += 1;
            }
        }
        self.gate.unpark();
    }

    fn poll(&mut self, timeout: Duration) -> Option<Event> {
        let _ = timeout;
        self.events.pop_front()
    }

    fn parking(&self) -> Arc<dyn Park> {
        self.gate.clone()
    }
}
