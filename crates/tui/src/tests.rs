use super::*;
use crate::buffer::append_mark;
use crate::width::{COMBINING, WIDE};
use std::time::Duration;

fn area(width: u16, height: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width,
        height,
    }
}

fn glyph(cell: &Cell) -> &Glyph {
    &cell.glyph
}

#[test]
fn colors_use_the_sixteen_ansi_indexes() {
    for (index, color) in Color::ALL.iter().enumerate() {
        assert_eq!(color.ansi(), u8::try_from(index).unwrap());
        assert!(!format!("{color:?}").is_empty());
        assert_eq!(*color, *color);
    }
    let style = Style {
        fg: Some(Color::Red),
        bg: Some(Color::Blue),
        bold: true,
        underline: true,
        reverse: true,
    };
    assert!(format!("{style:?}").contains("Red"));
    assert_eq!(Style::default().fg, None);
}

#[test]
fn display_width_counts_wide_and_combining_marks() {
    assert_eq!(char_width('A'), 1);
    assert_eq!(char_width('\u{0301}'), 0);
    assert_eq!(char_width('\u{3099}'), 0);
    assert_eq!(char_width('\u{0400}'), 1);
    assert_eq!(char_width('\u{1100}'), 2);
    assert_eq!(char_width('\u{10FF}'), 1);
    assert_eq!(char_width('\u{3042}'), 2);
    assert_eq!(char_width('\u{FF21}'), 2);
    assert_eq!(char_width(char::from_u32(0x3FFFD).unwrap()), 2);
    assert_eq!(char_width(char::MAX), 1);
    assert_eq!(display_width(""), 0);
    assert_eq!(display_width("a\u{0301}あ"), 3);
    assert!(ranges_are_ordered(WIDE));
    assert!(ranges_are_ordered(COMBINING));
}

fn ranges_are_ordered(ranges: &[(u32, u32)]) -> bool {
    let mut previous = 0u32;
    for (start, end) in ranges {
        if *start < previous || *end < *start {
            return false;
        }
        previous = end.saturating_add(1);
    }
    true
}

#[test]
fn truncate_adds_an_ellipsis_when_text_is_wider() {
    assert_eq!(truncate("ab", 2), "ab");
    assert_eq!(truncate("abcd", 3), "ab\u{2026}");
    assert_eq!(truncate("abcd", 1), "\u{2026}");
    assert_eq!(truncate("abcd", 0), "");
    assert_eq!(truncate("あa", 3), "あa");
    assert_eq!(truncate("あab", 3), "あ\u{2026}");
    assert_eq!(truncate("あa", 2), "\u{2026}");
}

#[test]
fn buffer_writes_spans_and_clears_wide_pairs() {
    let mut buffer = Buffer::empty(4, 2);
    let style = Style {
        fg: Some(Color::Green),
        ..Style::default()
    };
    let used = buffer.set_span(
        0,
        0,
        &Span {
            style,
            content: "a\u{0301}あb",
        },
        4,
    );
    assert_eq!(used, 4);
    assert_eq!(
        glyph(buffer.get(0, 0).unwrap()),
        &Glyph::Single("a\u{0301}".into())
    );
    assert_eq!(buffer.get(0, 0).unwrap().style, style);
    assert_eq!(glyph(buffer.get(1, 0).unwrap()), &Glyph::Wide("あ".into()));
    assert_eq!(glyph(buffer.get(2, 0).unwrap()), &Glyph::Tail);
    assert_eq!(glyph(buffer.get(3, 0).unwrap()), &Glyph::Single("b".into()));
    assert_eq!(
        buffer.set_span(
            1,
            0,
            &Span {
                style,
                content: "Z"
            },
            1
        ),
        1
    );
    assert_eq!(glyph(buffer.get(1, 0).unwrap()), &Glyph::Single("Z".into()));
    assert_eq!(glyph(buffer.get(2, 0).unwrap()), &Glyph::Single(" ".into()));
    buffer.set_span(
        0,
        0,
        &Span {
            style,
            content: "あ",
        },
        4,
    );
    assert_eq!(
        buffer.set_span(
            1,
            0,
            &Span {
                style,
                content: "Q"
            },
            1
        ),
        1
    );
    assert_eq!(glyph(buffer.get(0, 0).unwrap()), &Glyph::Single(" ".into()));
    assert_eq!(glyph(buffer.get(1, 0).unwrap()), &Glyph::Single("Q".into()));
    assert_eq!(
        buffer.set_span(
            3,
            0,
            &Span {
                style,
                content: "あ"
            },
            1
        ),
        0
    );
    assert_eq!(
        buffer.set_span(
            0,
            5,
            &Span {
                style,
                content: "a"
            },
            1
        ),
        0
    );
    assert_eq!(
        buffer.set_span(
            0,
            0,
            &Span {
                style,
                content: "a"
            },
            0
        ),
        0
    );
    assert_eq!(buffer.get(9, 0), None);
    assert_eq!(buffer.get(0, 9), None);
    let mut mark = Glyph::Tail;
    append_mark(&mut mark, '\u{0301}');
    assert_eq!(mark, Glyph::Tail);
    let mut wide = Glyph::Wide("あ".into());
    append_mark(&mut wide, '\u{0301}');
    assert_eq!(wide, Glyph::Wide("あ\u{0301}".into()));
    buffer.set_span(
        0,
        0,
        &Span {
            style,
            content: "あ",
        },
        4,
    );
    assert_eq!(
        buffer.set_span(
            2,
            0,
            &Span {
                style,
                content: "\u{3099}"
            },
            1
        ),
        0
    );
    assert_eq!(
        glyph(buffer.get(0, 0).unwrap()),
        &Glyph::Wide("あ\u{3099}".into())
    );
    assert_eq!(
        buffer.set_span(
            1,
            0,
            &Span {
                style,
                content: "\u{0301}"
            },
            1
        ),
        0
    );
    assert_eq!(
        glyph(buffer.get(0, 0).unwrap()),
        &Glyph::Wide("あ\u{3099}\u{0301}".into())
    );
    assert_eq!(
        buffer.set_span(
            0,
            0,
            &Span {
                style,
                content: "\u{0301}c"
            },
            2
        ),
        1
    );
    assert_eq!(glyph(buffer.get(0, 0).unwrap()), &Glyph::Single("c".into()));
}

#[test]
fn set_line_keeps_each_span_style() {
    let mut buffer = Buffer::empty(4, 1);
    let left = Style {
        fg: Some(Color::Red),
        ..Style::default()
    };
    let right = Style {
        underline: true,
        ..Style::default()
    };
    let used = buffer.set_line(
        1,
        0,
        &Line {
            spans: vec![
                Span {
                    style: left,
                    content: "ab",
                },
                Span {
                    style: right,
                    content: "c",
                },
            ],
        },
        2,
    );
    assert_eq!(used, 2);
    assert_eq!(glyph(buffer.get(1, 0).unwrap()), &Glyph::Single("a".into()));
    assert_eq!(buffer.get(1, 0).unwrap().style, left);
    assert_eq!(glyph(buffer.get(2, 0).unwrap()), &Glyph::Single("b".into()));
    assert_eq!(buffer.get(2, 0).unwrap().style, left);
    assert_eq!(glyph(buffer.get(3, 0).unwrap()), &Glyph::Single(" ".into()));
    assert_eq!(buffer.set_line(0, 0, &Line { spans: Vec::new() }, 4), 0);
    assert_eq!(buffer.width(), 4);
    assert_eq!(buffer.height(), 1);
}

#[test]
fn split_places_fixed_percent_and_minimum() {
    let parent = Rect {
        x: 2,
        y: 1,
        width: 10,
        height: 4,
    };
    let rects = split(
        parent,
        Direction::Horizontal,
        &[
            Constraint::Fixed(3),
            Constraint::Percent(50),
            Constraint::Min(1),
        ],
    );
    assert_eq!(
        rects,
        vec![
            Rect {
                x: 2,
                y: 1,
                width: 3,
                height: 4
            },
            Rect {
                x: 5,
                y: 1,
                width: 5,
                height: 4
            },
            Rect {
                x: 10,
                y: 1,
                width: 2,
                height: 4
            },
        ]
    );
    let mins = split(
        area(10, 10),
        Direction::Vertical,
        &[Constraint::Min(2), Constraint::Min(2)],
    );
    assert_eq!(mins[0].height, 5);
    assert_eq!(mins[1].y, 5);
    assert_eq!(mins[1].height, 5);
    assert_eq!(mins[0].width, 10);
    let tight = split(
        area(5, 1),
        Direction::Horizontal,
        &[Constraint::Fixed(4), Constraint::Min(3)],
    );
    assert_eq!(tight[0].width, 4);
    assert_eq!(tight[1].width, 1);
    let capped = split(
        area(10, 1),
        Direction::Horizontal,
        &[Constraint::Percent(150)],
    );
    assert_eq!(capped[0].width, 10);
    let order = split(
        area(10, 1),
        Direction::Horizontal,
        &[Constraint::Percent(50), Constraint::Fixed(8)],
    );
    assert_eq!(order[0].width, 5);
    assert_eq!(order[1].width, 5);
    assert!(split(area(4, 1), Direction::Horizontal, &[]).is_empty());
    let zero = split(
        area(0, 2),
        Direction::Horizontal,
        &[Constraint::Fixed(3), Constraint::Percent(0)],
    );
    assert_eq!(zero[0].width, 0);
    assert_eq!(zero[1].width, 0);
}

#[test]
fn test_backend_draws_changed_cells_and_reads_events() {
    let mut backend = TestBackend::new(2, 1);
    let mut buffer = Buffer::empty(2, 1);
    Backend::draw(&mut backend, &buffer);
    assert_eq!(backend.writes(0, 0), Some(0));
    assert_eq!(backend.writes(3, 0), None);
    assert_eq!(backend.cell(3, 0), None);
    assert_eq!(
        glyph(backend.cell(0, 0).unwrap()),
        &Glyph::Single(" ".into())
    );
    buffer.set_span(
        0,
        0,
        &Span {
            style: Style::default(),
            content: "Z",
        },
        1,
    );
    backend.draw(&buffer);
    assert_eq!(backend.writes(0, 0), Some(1));
    assert_eq!(backend.writes(1, 0), Some(0));
    assert_eq!(
        glyph(backend.cell(0, 0).unwrap()),
        &Glyph::Single("Z".into())
    );
    let wide = Buffer::empty(3, 1);
    backend.draw(&wide);
    assert_eq!(backend.size().width, 3);
    assert_eq!(backend.writes(0, 0), Some(0));
    let empty = Buffer::empty(0, 0);
    backend.draw(&empty);
    assert_eq!(backend.size(), area(0, 0));
    backend.push(Event::Key(KeyCode::Enter));
    backend.push(Event::Resize {
        width: 4,
        height: 5,
    });
    assert_eq!(
        backend.poll(Duration::from_millis(1)),
        Some(Event::Key(KeyCode::Enter))
    );
    assert_eq!(
        backend.poll(Duration::from_millis(1)),
        Some(Event::Resize {
            width: 4,
            height: 5
        })
    );
    assert_eq!(backend.poll(Duration::from_millis(1)), None);
}

#[test]
fn debug_names_each_event_variant() {
    let keys = [
        KeyCode::Char('a'),
        KeyCode::Enter,
        KeyCode::Backspace,
        KeyCode::Esc,
        KeyCode::Tab,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
    ];
    for key in keys {
        assert_eq!(key, key);
        assert!(!format!("{key:?}").is_empty());
    }
    let buttons = [MouseButton::Left, MouseButton::Middle, MouseButton::Right];
    for button in buttons {
        assert_eq!(button, button);
        assert!(!format!("{button:?}").is_empty());
    }
    for direction in [Wheel::Up, Wheel::Down] {
        assert_eq!(direction, direction);
        assert!(!format!("{direction:?}").is_empty());
    }
    let events = [
        Event::Key(KeyCode::Tab),
        Event::Resize {
            width: 1,
            height: 2,
        },
        Event::Press {
            button: MouseButton::Left,
            column: 1,
            row: 2,
        },
        Event::Release {
            button: MouseButton::Right,
            column: 0,
            row: 0,
        },
        Event::Wheel {
            direction: Wheel::Down,
            column: 3,
            row: 4,
        },
    ];
    for event in events {
        assert_eq!(event, event);
        assert!(!format!("{event:?}").is_empty());
    }
    for direction in [Direction::Horizontal, Direction::Vertical] {
        assert_eq!(direction, direction);
        assert!(!format!("{direction:?}").is_empty());
    }
    for constraint in [
        Constraint::Fixed(1),
        Constraint::Min(2),
        Constraint::Percent(3),
    ] {
        assert_eq!(constraint, constraint);
        assert!(!format!("{constraint:?}").is_empty());
    }
    for glyph in [
        Glyph::Single("a".into()),
        Glyph::Wide("あ".into()),
        Glyph::Tail,
    ] {
        assert_eq!(glyph, glyph.clone());
        assert!(!format!("{glyph:?}").is_empty());
    }
    let cell = Cell::blank();
    assert_eq!(cell, cell.clone());
    let buffer = Buffer::empty(1, 1);
    assert_eq!(buffer, buffer.clone());
    let span = Span {
        style: Style::default(),
        content: "a",
    };
    assert_eq!(span, span.clone());
    let line = Line { spans: vec![span] };
    assert_eq!(line, line.clone());
    let backend = TestBackend::new(1, 1);
    assert_eq!(backend, backend.clone());
    assert!(!format!("{backend:?}").is_empty());
}
