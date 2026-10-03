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

fn row_text(buffer: &Buffer, x: u16, y: u16, width: u16) -> String {
    let mut text = String::new();
    for column in x..x + width {
        text.push_str(&symbol(buffer, column, y));
    }
    text
}

fn symbol(buffer: &Buffer, x: u16, y: u16) -> String {
    match &buffer.get(x, y).unwrap().glyph {
        Glyph::Single(text) | Glyph::Wide(text) => text.clone(),
        Glyph::Tail => String::new(),
    }
}

fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn block_draws_a_focused_border_and_title() {
    let mut buffer = Buffer::empty(10, 4);
    let plain = Style::default();
    let focused = Style {
        bold: true,
        ..Style::default()
    };
    draw_block(
        &mut buffer,
        rect(0, 0, 10, 4),
        "Needs review",
        false,
        plain,
        focused,
    );
    assert_eq!(symbol(&buffer, 0, 0), "+");
    assert_eq!(row_text(&buffer, 1, 0, 8), "Needs r…");
    assert!(!buffer.get(0, 1).unwrap().style.bold);
    assert_eq!(symbol(&buffer, 0, 1), "|");
    draw_block(
        &mut buffer,
        rect(0, 0, 10, 4),
        "Title",
        true,
        plain,
        focused,
    );
    assert!(buffer.get(0, 0).unwrap().style.bold);
    assert_eq!(row_text(&buffer, 1, 0, 5), "Title");
    draw_block(
        &mut buffer,
        rect(0, 0, 8, 2),
        "Too long title",
        false,
        plain,
        focused,
    );
    assert_eq!(row_text(&buffer, 1, 0, 6), "Too l…");
    assert_eq!(symbol(&buffer, 0, 1), "+");
    draw_block(&mut buffer, rect(0, 0, 1, 4), "x", false, plain, focused);
    let inside = inner(rect(2, 3, 6, 4));
    assert_eq!(inside, rect(3, 4, 4, 2));
    assert_eq!(inner(rect(0, 0, 1, 4)).width, 0);
}

#[test]
fn table_selects_a_row_and_hit_testing_names_it() {
    let mut buffer = Buffer::empty(24, 8);
    let parts = split(
        rect(0, 0, 24, 8),
        Direction::Vertical,
        &[Constraint::Min(4), Constraint::Fixed(1)],
    );
    let body = parts[0];
    let mut state = ListState::new();
    state.selected = Some(1);
    state.offset = 0;
    state.reveal(3, usize::from(body.height.saturating_sub(1)));
    let rows = [
        ["PRJ/repo#12", "Fix"].as_slice(),
        ["PRJ/repo#13", "Draft"].as_slice(),
        ["PRJ/pipe#21", "Wait"].as_slice(),
    ];
    let plain = Style::default();
    let selected = Style {
        reverse: true,
        ..Style::default()
    };
    draw_table(&mut buffer, body, &rows, &[12, 6], &state, plain, selected);
    assert_eq!(symbol(&buffer, body.x, body.y + 1), "P");
    assert!(buffer.get(body.x, body.y + 1).unwrap().style.reverse);
    assert!(!buffer.get(body.x, body.y).unwrap().style.reverse);
    assert_eq!(symbol(&buffer, body.x + 13, body.y), "F");
    let label_y = body.y + body.height - 1;
    assert_eq!(symbol(&buffer, body.x, label_y), "2");
    assert_eq!(
        hit_row(body, state.offset, rows.len(), body.x, body.y + 1),
        Some(1)
    );
    assert_eq!(
        hit_row(body, state.offset, rows.len(), body.x, label_y),
        None
    );
    assert_eq!(hit_row(body, state.offset, rows.len(), 0, 100), None);
    assert_eq!(hit_row(body, 0, rows.len(), body.x, body.y + 4), None);
    assert_eq!(position(Some(1), 3), "2/3");
    assert_eq!(position(None, 3), "0/3");
    assert_eq!(position(Some(9), 3), "0/3");
    let mut scrolled = ListState {
        selected: Some(2),
        offset: 0,
    };
    scrolled.reveal(3, 1);
    assert_eq!(scrolled.offset, 2);
    assert_eq!(ensure_visible(Some(0), 10, 3, 4), 0);
    assert_eq!(ensure_visible(Some(5), 10, 3, 0), 3);
    assert_eq!(ensure_visible(Some(1), 10, 3, 0), 0);
    assert_eq!(ensure_visible(None, 10, 3, 2), 2);
    assert_eq!(ensure_visible(Some(4), 3, 0, 9), 3);
    let mut list_buffer = Buffer::empty(8, 3);
    let mut list = ListState {
        selected: Some(1),
        offset: 1,
    };
    draw_list(
        &mut list_buffer,
        rect(0, 0, 8, 3),
        &["one", "two", "three"],
        &list,
        plain,
        selected,
    );
    assert_eq!(symbol(&list_buffer, 0, 0), "t");
    assert!(list_buffer.get(0, 0).unwrap().style.reverse);
    assert_eq!(symbol(&list_buffer, 0, 2), "2");
    list.offset = 5;
    draw_list(
        &mut list_buffer,
        rect(0, 0, 4, 1),
        &["abcdef"],
        &list,
        plain,
        selected,
    );
    assert_eq!(symbol(&list_buffer, 0, 0), "0");
    draw_list(
        &mut list_buffer,
        rect(0, 0, 0, 0),
        &["a"],
        &ListState::new(),
        plain,
        selected,
    );
    let mut wide = Buffer::empty(6, 3);
    draw_table(
        &mut wide,
        rect(0, 0, 3, 3),
        &[&["abcdef", "z"]],
        &[4],
        &ListState {
            selected: None,
            offset: 0,
        },
        plain,
        selected,
    );
    assert_eq!(symbol(&wide, 0, 0), "a");
    draw_table(
        &mut wide,
        rect(0, 0, 6, 3),
        &[&["ab", "cd", "ef"]],
        &[2, 2],
        &ListState::new(),
        plain,
        selected,
    );
    assert_eq!(symbol(&wide, 3, 0), "c");
    draw_table(
        &mut wide,
        rect(0, 0, 6, 3),
        &[&["ab", "cd"]],
        &[2],
        &ListState::new(),
        plain,
        selected,
    );
}

#[test]
fn paragraph_tabs_and_input_edit_the_line() {
    let mut buffer = Buffer::empty(12, 6);
    let plain = Style::default();
    draw_paragraph(&mut buffer, rect(0, 0, 5, 3), "hello world", plain);
    assert_eq!(symbol(&buffer, 0, 0), "h");
    assert_eq!(symbol(&buffer, 0, 1), "w");
    draw_paragraph(&mut buffer, rect(0, 0, 3, 2), "abcdef", plain);
    assert_eq!(symbol(&buffer, 0, 0), "a");
    assert_eq!(symbol(&buffer, 0, 1), "d");
    draw_paragraph(&mut buffer, rect(0, 0, 4, 3), "hi\nthere", plain);
    assert_eq!(symbol(&buffer, 0, 1), "t");
    draw_paragraph(&mut buffer, rect(0, 0, 1, 2), "あa", plain);
    draw_paragraph(&mut buffer, rect(0, 0, 5, 1), "one two three", plain);
    draw_paragraph(&mut buffer, rect(0, 0, 0, 2), "x", plain);
    draw_paragraph(&mut buffer, rect(0, 0, 4, 2), "\n", plain);
    let selected = Style {
        bold: true,
        ..Style::default()
    };
    draw_tabs(
        &mut buffer,
        rect(0, 3, 12, 1),
        &["One", "Two"],
        1,
        plain,
        selected,
    );
    assert_eq!(symbol(&buffer, 0, 3), "O");
    assert!(!buffer.get(0, 3).unwrap().style.bold);
    assert_eq!(symbol(&buffer, 3, 3), "|");
    assert_eq!(symbol(&buffer, 4, 3), "T");
    assert!(buffer.get(4, 3).unwrap().style.bold);
    draw_tabs(
        &mut buffer,
        rect(0, 3, 1, 1),
        &["ab", "cd"],
        0,
        plain,
        selected,
    );
    draw_tabs(&mut buffer, rect(0, 3, 4, 0), &["ab"], 0, plain, selected);
    draw_tabs(&mut buffer, rect(0, 3, 4, 1), &[], 0, plain, selected);
    let mut input = Input::default();
    assert_eq!(input, Input::new());
    assert!(format!("{input:?}").contains("cursor"));
    input.insert('a');
    input.insert('あ');
    input.insert('b');
    assert_eq!(input.value, "aあb");
    assert_eq!(input.cursor, 3);
    input.backspace();
    assert_eq!(input.value, "aあ");
    assert_eq!(input.cursor, 2);
    input.cursor = 0;
    input.backspace();
    assert_eq!(input.cursor, 0);
    input.click(rect(0, 4, 6, 1), 1, 4);
    assert_eq!(input.cursor, 1);
    input.click(rect(0, 4, 6, 1), 5, 4);
    assert_eq!(input.cursor, 2);
    input.click(rect(0, 4, 6, 1), 0, 3);
    assert_eq!(input.cursor, 2);
    input.click(rect(0, 4, 0, 1), 0, 4);
    draw_input(&mut buffer, rect(0, 4, 6, 1), &input, plain);
    assert!(buffer.get(3, 4).unwrap().style.reverse);
    input.value = "abcdef".to_owned();
    input.cursor = 6;
    draw_input(&mut buffer, rect(0, 5, 3, 1), &input, plain);
    assert_eq!(symbol(&buffer, 0, 5), "e");
    assert!(buffer.get(2, 5).unwrap().style.reverse);
    buffer.set_style(20, 20, plain);
    draw_input(&mut buffer, rect(0, 0, 0, 1), &input, plain);
    let state = ListState::default();
    assert_eq!(state, ListState::new());
    assert!(format!("{state:?}").contains("offset"));
    assert_eq!(state.selected, None);
}
