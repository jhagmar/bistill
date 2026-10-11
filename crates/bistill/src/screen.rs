#![cfg_attr(not(test), allow(dead_code))]

//! The inbox screen.
//!
//! The view is the tabs, the pull-request table, the detail pane, and a
//! footer. At 100 columns or more, the table and the detail sit side by side.
//! Narrower than that, the table is above the detail.

use bistill_lib::{Build, Enrichment, EventKind, Row, Section, Snapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tui::{
    Buffer, Color, Constraint, Direction, Event, Input, KeyCode, ListState, Rect, Span, Style,
    Wheel, draw_block, draw_input, draw_paragraph, draw_table, draw_tabs, fill_rect, hit_row,
    inner, split,
};

const HELP: &str = "\
j / k    Move the selection
Enter    Open the pull request
m        Mark this pull request read
i        Ignore or watch a review
r        Refresh
/        Filter title, repo, and author
Tab      Switch section
?        List the keys
q        Quit";

const LIST_KEYS: &str =
    "j/k move  Enter open  m read  i ignore  r refresh  / filter  Tab section  ? help  q quit";
const EDIT_KEYS: &str = "Enter apply  Esc cancel";
const HELP_KEYS: &str = "? or Esc close  q quit";

/// What one key or click did.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Action {
    /// The view changed and the process keeps running.
    None,
    /// `q`.
    Quit,
    /// `r`.
    Refresh,
    /// Enter or a double-click. The string is `html_url`.
    Open(String),
}

/// Poll phase drawn in the footer.
#[derive(Eq, PartialEq)]
pub(crate) enum Phase {
    /// A fetch is in flight.
    Fetching,
    /// The latest rows are on screen.
    Ready,
    /// HTTP 401.
    Auth,
    /// TLS failed.
    Tls,
    /// Bitbucket could not be reached.
    Unreachable {
        /// When the outage started, epoch milliseconds.
        since_ms: u64,
    },
    /// HTTP 429.
    RateLimited,
    /// Another poll failure. `message` is that error's display line.
    Failed {
        /// The line shown in the footer.
        message: String,
    },
}

/// A key asked the watermark file to change.
#[derive(Clone, Eq, PartialEq)]
pub(crate) enum Pending {
    /// Move this pull request's watermark to its newest event.
    Read(String),
    /// Toggle ignored on a Needs review row.
    Ignore(String),
}

/// Who is drawing.
#[derive(Eq, PartialEq)]
pub(crate) enum Role {
    /// This process holds the lock.
    Holder(Phase),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Pane {
    Table,
    Detail,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Frame {
    tabs: Rect,
    table: Rect,
    detail: Rect,
    table_inner: Rect,
    detail_inner: Rect,
    footer: Rect,
}

/// Selection, filter, and overlay state.
pub(crate) struct Screen {
    section: Section,
    needs: ListState,
    waiting: ListState,
    filter: String,
    editing: bool,
    draft: Input,
    help: bool,
    focus: Pane,
    detail_offset: usize,
    detail_lines: usize,
    /// Pull request the detail scroll belongs to.
    detail_for: Option<String>,
    table_len: usize,
    table_window: usize,
    detail_window: usize,
    last_click: Option<(String, u64)>,
    /// Selected pull request on Needs review.
    needs_id: Option<String>,
    /// Selected pull request on Waiting.
    waiting_id: Option<String>,
    /// Last row drawn in the detail, kept while a fetch omits that id.
    held: Option<Row>,
    frame: Frame,
    /// Activity id this pull request is caught up to. Missing means nothing is `new`.
    floors: BTreeMap<String, u64>,
    /// Reviewer rows left out of the tray count.
    ignored: BTreeSet<String>,
    /// Set by a key. The caller writes `watermarks.json` and clears it.
    pending: Option<Pending>,
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen {
    /// Needs review, nothing selected, no filter.
    pub(crate) fn new() -> Self {
        Screen {
            section: Section::NeedsReview,
            needs: ListState::new(),
            waiting: ListState::new(),
            filter: String::new(),
            editing: false,
            draft: Input::new(),
            help: false,
            focus: Pane::Table,
            detail_offset: 0,
            detail_lines: 0,
            detail_for: None,
            table_len: 0,
            table_window: 0,
            detail_window: 0,
            last_click: None,
            needs_id: None,
            waiting_id: None,
            held: None,
            frame: Frame::default(),
            floors: BTreeMap::new(),
            ignored: BTreeSet::new(),
            pending: None,
        }
    }

    /// The watermark edit from the last key, if there was one.
    pub(crate) fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    /// Copy the watermark cursors the detail pane and the ignored badge use.
    pub(crate) fn set_marks(&mut self, floors: BTreeMap<String, u64>, ignored: BTreeSet<String>) {
        self.floors = floors;
        self.ignored = ignored;
    }
}

/// Clock values the screen formats. `offset_secs` is the OS zone offset.
#[derive(Clone, Copy)]
pub(crate) struct Clock {
    /// Epoch milliseconds.
    pub now_ms: u64,
    /// Seconds east of UTC.
    pub offset_secs: i32,
}

/// Paint `snapshot` into `buffer`. `snapshot` absent means the file is not there yet.
pub(crate) fn draw(
    screen: &mut Screen,
    buffer: &mut Buffer,
    snapshot: Option<&Snapshot>,
    role: &Role,
    clock: Clock,
) {
    if buffer.width() == 0 || buffer.height() == 0 {
        screen.frame = Frame::default();
    } else {
        paint(screen, buffer, snapshot, role, clock);
    }
}

fn paint(
    screen: &mut Screen,
    buffer: &mut Buffer,
    snapshot: Option<&Snapshot>,
    role: &Role,
    clock: Clock,
) {
    let frame = layout(buffer.width(), buffer.height());
    screen.frame = frame;
    let plain = Style::default();
    let quiet = tone(Color::BrightBlack, false);
    let focus = tone(Color::Cyan, true);
    let selected = Style {
        fg: Some(Color::BrightWhite),
        bg: Some(Color::Blue),
        bold: true,
        ..Style::default()
    };
    if frame.tabs.height > 0 {
        let selected = match screen.section {
            Section::NeedsReview => 0,
            Section::Waiting => 1,
        };
        draw_tabs(
            buffer,
            frame.tabs,
            &["Needs review", "Waiting"],
            selected,
            quiet,
            focus,
        );
    }
    let fetching = matches!(role, Role::Holder(Phase::Fetching));
    screen.table_window = usize::from(frame.table_inner.height.saturating_sub(1));
    screen.detail_window = usize::from(frame.detail_inner.height);
    sync_lists(screen, snapshot, fetching);
    let rows = visible(snapshot, screen.section, &screen.filter);
    screen.table_len = rows.len();
    draw_block(
        buffer,
        frame.table,
        section_name(screen.section),
        screen.focus == Pane::Table,
        quiet,
        focus,
    );
    if rows.is_empty() && screen.filter.is_empty() && snapshot.is_some() {
        draw_paragraph(
            buffer,
            frame.table_inner,
            "Nothing needs your attention.",
            tone(Color::Green, false),
        );
    } else {
        let owned = table_cells(&rows, clock.now_ms, &screen.ignored);
        let views: Vec<[&str; 5]> = owned
            .iter()
            .map(|cell| {
                [
                    cell[0].as_str(),
                    cell[1].as_str(),
                    cell[2].as_str(),
                    cell[3].as_str(),
                    cell[4].as_str(),
                ]
            })
            .collect();
        let refs: Vec<&[&str]> = views.iter().map(|cell| cell.as_slice()).collect();
        let state = active(screen);
        let widths = column_widths(frame.table_inner.width);
        draw_table(
            buffer,
            frame.table_inner,
            &refs,
            &widths,
            state,
            plain,
            selected,
        );
        paint_metadata(buffer, frame.table_inner, rows.len(), state, &widths);
        paint_badges(
            buffer,
            frame.table_inner,
            &rows,
            &screen.ignored,
            active(screen),
            &widths,
        );
        paint_position(buffer, frame.table_inner, quiet);
    }
    let user_slug = snapshot.map(|item| item.user_slug.as_str()).unwrap_or("");
    let detail_row = match chosen(&rows, active(screen)).cloned() {
        Some(row) => {
            screen.held = Some(row.clone());
            Some(row)
        }
        None => held_row(screen, fetching),
    };
    let title = detail_row
        .as_ref()
        .map(|row| format!("{}#{}", row.repo, row.number))
        .unwrap_or_else(|| "Detail".to_owned());
    let shown_id = detail_row.as_ref().map(|row| row.id.clone());
    if screen.detail_for != shown_id {
        screen.detail_offset = 0;
        screen.detail_for = shown_id;
    }
    let mut feed = detail_row
        .as_ref()
        .map(|row| {
            feed_lines(
                row,
                user_slug,
                screen.floors.get(&row.id).copied(),
                clock,
                usize::from(frame.detail_inner.width),
            )
        })
        .unwrap_or_default();
    if let Some(snapshot) = snapshot.filter(|snapshot| snapshot.truncated > 0) {
        feed.insert(0, more_line(&format!("and {} more", snapshot.truncated)));
    }
    screen.detail_lines = feed.len();
    screen.detail_offset = screen.detail_offset.min(
        screen
            .detail_lines
            .saturating_sub(screen.detail_window.max(1)),
    );
    let start = screen.detail_offset;
    let end = start.saturating_add(screen.detail_window.max(1));
    let window = if start < feed.len() {
        &feed[start..end.min(feed.len())]
    } else {
        &[]
    };
    draw_block(
        buffer,
        frame.detail,
        &title,
        screen.focus == Pane::Detail,
        quiet,
        focus,
    );
    paint_feed(buffer, frame.detail_inner, window);
    let status = footer(role, clock.offset_secs);
    let keys = if screen.help {
        HELP_KEYS
    } else if screen.editing {
        EDIT_KEYS
    } else {
        LIST_KEYS
    };
    paint_footer(buffer, frame.footer, &status, footer_style(role), keys);
    if screen.editing {
        let hint = u16::try_from(EDIT_KEYS.chars().count()).unwrap_or(0);
        let input_width = frame.footer.width.saturating_sub(hint.saturating_add(2));
        draw_line(buffer, frame.footer, "/", focus);
        let field = Rect {
            x: frame.footer.x.saturating_add(1),
            y: frame.footer.y,
            width: input_width,
            height: frame.footer.height,
        };
        draw_input(buffer, field, &screen.draft, plain);
        draw_line(
            buffer,
            Rect {
                x: field.x.saturating_add(input_width).saturating_add(1),
                y: frame.footer.y,
                width: hint,
                height: frame.footer.height,
            },
            EDIT_KEYS,
            keys_style(),
        );
    }
    if screen.help {
        draw_help(buffer, plain, focus);
    }
}

fn draw_help(buffer: &mut Buffer, plain: Style, marked: Style) {
    let lines: Vec<&str> = HELP.lines().collect();
    let inner_w = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let width = u16::try_from(inner_w)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(buffer.width());
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(buffer.height());
    let area = Rect {
        x: buffer.width().saturating_sub(width) / 2,
        y: buffer.height().saturating_sub(height) / 2,
        width,
        height,
    };
    fill_rect(buffer, area, plain);
    draw_block(buffer, area, "Keys", true, plain, marked);
    draw_paragraph(buffer, inner(area), HELP, plain);
}

/// Apply one event. `now_ms` is the click clock for a double-click.
pub(crate) fn handle(
    screen: &mut Screen,
    event: Event,
    snapshot: Option<&Snapshot>,
    now_ms: u64,
) -> Action {
    if screen.help && closes_help(&event) {
        screen.help = false;
        Action::None
    } else if screen.editing {
        edit_event(screen, event, snapshot)
    } else {
        command(screen, event, snapshot, now_ms)
    }
}

fn closes_help(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(KeyCode::Esc | KeyCode::Char('?')) | Event::Press { .. }
    )
}

fn edit_event(screen: &mut Screen, event: Event, snapshot: Option<&Snapshot>) -> Action {
    match event {
        Event::Key(KeyCode::Esc) => {
            screen.editing = false;
            Action::None
        }
        Event::Key(KeyCode::Enter) => {
            let value = screen.draft.value.clone();
            screen.editing = false;
            apply_filter(screen, value, snapshot);
            Action::None
        }
        Event::Key(KeyCode::Backspace) => {
            screen.draft.backspace();
            Action::None
        }
        Event::Key(KeyCode::Char(ch)) => {
            screen.draft.insert(ch);
            Action::None
        }
        _ => Action::None,
    }
}

fn command(screen: &mut Screen, event: Event, snapshot: Option<&Snapshot>, now_ms: u64) -> Action {
    match event {
        Event::Key(KeyCode::Char('q')) => Action::Quit,
        Event::Key(KeyCode::Char('r')) => Action::Refresh,
        Event::Key(KeyCode::Char('m')) => {
            note(screen, snapshot, true);
            Action::None
        }
        Event::Key(KeyCode::Char('i')) => {
            if screen.section == Section::NeedsReview {
                note(screen, snapshot, false);
            }
            Action::None
        }
        Event::Key(KeyCode::Char('?')) => {
            screen.help = true;
            Action::None
        }
        Event::Key(KeyCode::Char('/')) => {
            screen.draft = Input {
                value: screen.filter.clone(),
                cursor: screen.filter.chars().count(),
            };
            screen.editing = true;
            Action::None
        }
        Event::Key(KeyCode::Tab) => {
            screen.section = match screen.section {
                Section::NeedsReview => Section::Waiting,
                Section::Waiting => Section::NeedsReview,
            };
            screen.detail_offset = 0;
            screen.last_click = None;
            Action::None
        }
        Event::Key(KeyCode::Char('j') | KeyCode::Down) => {
            step(screen, snapshot, true);
            Action::None
        }
        Event::Key(KeyCode::Char('k') | KeyCode::Up) => {
            step(screen, snapshot, false);
            Action::None
        }
        Event::Key(KeyCode::Enter) => open_selected(screen, snapshot),
        Event::Press { column, row, .. } => press(screen, snapshot, column, row, now_ms),
        Event::Wheel { direction, .. } => {
            wheel(screen, direction);
            Action::None
        }
        _ => Action::None,
    }
}

fn apply_filter(screen: &mut Screen, value: String, snapshot: Option<&Snapshot>) {
    screen.filter = value;
    screen.detail_offset = 0;
    screen.last_click = None;
    for section in [Section::NeedsReview, Section::Waiting] {
        let rows = visible(snapshot, section, &screen.filter);
        let len = rows.len();
        let selected = {
            let state = state_mut(screen, section);
            state.selected = match state.selected {
                Some(index) if index < len => Some(index),
                Some(_) if len > 0 => Some(len - 1),
                _ => None,
            };
            state.offset = state.offset.min(len);
            state.selected
        };
        let id = selected.and_then(|index| rows.get(index).map(|row| row.id.clone()));
        remember(screen, section, id);
    }
}

fn step(screen: &mut Screen, snapshot: Option<&Snapshot>, down: bool) {
    let len = visible(snapshot, screen.section, &screen.filter).len();
    let window = screen.table_window;
    let state = active_mut(screen);
    if len == 0 {
        state.selected = None;
        state.offset = 0;
    } else {
        let next = match state.selected {
            None => 0,
            Some(index) if down => index.saturating_add(1).min(len - 1),
            Some(index) => index.saturating_sub(1),
        };
        state.selected = Some(next);
        state.reveal(len, window);
    }
    let id = {
        let rows = visible(snapshot, screen.section, &screen.filter);
        chosen(&rows, active(screen)).map(|row| row.id.clone())
    };
    if remembered(screen, screen.section) != id {
        screen.detail_offset = 0;
    }
    remember(screen, screen.section, id);
    note(screen, snapshot, true);
}

fn note(screen: &mut Screen, snapshot: Option<&Snapshot>, read: bool) {
    let rows = visible(snapshot, screen.section, &screen.filter);
    if let Some(row) = chosen(&rows, active(screen)) {
        let id = row.id.clone();
        let pending = if read {
            Pending::Read(id)
        } else {
            Pending::Ignore(id)
        };
        screen.pending = Some(pending);
    }
    let _ = screen.pending.is_some();
}

fn open_selected(screen: &Screen, snapshot: Option<&Snapshot>) -> Action {
    let rows = visible(snapshot, screen.section, &screen.filter);
    match chosen(&rows, active(screen)) {
        Some(row) => Action::Open(row.html_url.clone()),
        None => Action::None,
    }
}

fn press(
    screen: &mut Screen,
    snapshot: Option<&Snapshot>,
    column: u16,
    row: u16,
    now_ms: u64,
) -> Action {
    if let Some(section) = tab_at(&screen.frame, column, row) {
        screen.section = section;
        screen.focus = Pane::Table;
        screen.detail_offset = 0;
        screen.last_click = None;
        Action::None
    } else if contains(screen.frame.table, column, row) {
        screen.focus = Pane::Table;
        let rows = visible(snapshot, screen.section, &screen.filter);
        match hit_row(
            screen.frame.table_inner,
            active(screen).offset,
            rows.len(),
            column,
            row,
        ) {
            Some(index) => {
                let id = rows[index].id.clone();
                active_mut(screen).selected = Some(index);
                if remembered(screen, screen.section).as_deref() != Some(id.as_str()) {
                    screen.detail_offset = 0;
                }
                remember(screen, screen.section, Some(id.clone()));
                note(screen, snapshot, true);
                let open = match &screen.last_click {
                    Some((previous, at))
                        if previous == &id && now_ms.saturating_sub(*at) <= 400 =>
                    {
                        true
                    }
                    Some((previous, _)) if previous == &id => false,
                    Some(_) => false,
                    None => false,
                };
                screen.last_click = Some((id, now_ms));
                if open {
                    Action::Open(rows[index].html_url.clone())
                } else {
                    Action::None
                }
            }
            None => Action::None,
        }
    } else if contains(screen.frame.detail, column, row) {
        screen.focus = Pane::Detail;
        Action::None
    } else {
        Action::None
    }
}

fn wheel(screen: &mut Screen, direction: Wheel) {
    let down = direction == Wheel::Down;
    if screen.focus == Pane::Table {
        let max = screen.table_len.saturating_sub(screen.table_window.max(1));
        let state = active_mut(screen);
        if down {
            state.offset = state.offset.saturating_add(1).min(max);
        } else {
            state.offset = state.offset.saturating_sub(1);
        }
    } else {
        let max = screen
            .detail_lines
            .saturating_sub(screen.detail_window.max(1));
        if down {
            screen.detail_offset = screen.detail_offset.saturating_add(1).min(max);
        } else {
            screen.detail_offset = screen.detail_offset.saturating_sub(1);
        }
    }
}

fn tab_at(frame: &Frame, column: u16, row: u16) -> Option<Section> {
    if frame.tabs.height > 0 && row == frame.tabs.y {
        let local = column.saturating_sub(frame.tabs.x);
        if local < 12 {
            Some(Section::NeedsReview)
        } else {
            Some(Section::Waiting)
        }
    } else {
        None
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    let col = u32::from(column).wrapping_sub(u32::from(area.x));
    let line = u32::from(row).wrapping_sub(u32::from(area.y));
    col < u32::from(area.width) && line < u32::from(area.height)
}

fn layout(width: u16, height: u16) -> Frame {
    let footer = Rect {
        x: 0,
        y: height - 1,
        width,
        height: 1,
    };
    let tabs = if height > 2 {
        Rect {
            x: 0,
            y: 0,
            width,
            height: 1,
        }
    } else {
        blank()
    };
    let body = Rect {
        x: 0,
        y: tabs.height,
        width,
        height: height.saturating_sub(u16::from(tabs.height > 0) + 1),
    };
    let (table, detail) = if width >= 100 {
        let panes = split(
            body,
            Direction::Horizontal,
            &[Constraint::Fixed(width / 2), Constraint::Min(0)],
        );
        (panes[0], panes[1])
    } else {
        let panes = split(
            body,
            Direction::Vertical,
            &[Constraint::Fixed(body.height / 2), Constraint::Min(0)],
        );
        (panes[0], panes[1])
    };
    Frame {
        tabs,
        table,
        detail,
        table_inner: inner(table),
        detail_inner: inner(detail),
        footer,
    }
}

fn visible<'a>(snapshot: Option<&'a Snapshot>, section: Section, filter: &str) -> Vec<&'a Row> {
    let rows = match (snapshot, section) {
        (Some(snapshot), Section::NeedsReview) => snapshot.needs_review.as_slice(),
        (Some(snapshot), Section::Waiting) => snapshot.waiting.as_slice(),
        (None, _) => &[],
    };
    rows.iter()
        .filter(|row| matches_filter(row, filter))
        .collect()
}

fn matches_filter(row: &Row, filter: &str) -> bool {
    if filter.is_empty() {
        true
    } else {
        let needle = filter.to_ascii_lowercase();
        row.title.to_ascii_lowercase().contains(&needle)
            || row.repo.to_ascii_lowercase().contains(&needle)
            || row.author.to_ascii_lowercase().contains(&needle)
    }
}

fn sync_lists(screen: &mut Screen, snapshot: Option<&Snapshot>, fetching: bool) {
    let window = screen.table_window;
    for section in [Section::NeedsReview, Section::Waiting] {
        let rows = visible(snapshot, section, &screen.filter);
        place_section(screen, section, &rows, fetching, window);
    }
}

fn place_section(
    screen: &mut Screen,
    section: Section,
    rows: &[&Row],
    fetching: bool,
    window: usize,
) {
    let previous = state_mut(screen, section).selected;
    let remembered_id = remembered(screen, section);
    let found = remembered_id
        .as_ref()
        .and_then(|id| rows.iter().position(|row| row.id == *id));
    let next = match (rows.is_empty(), found, remembered_id.is_some(), fetching) {
        (true, _, _, _) => None,
        (false, Some(index), _, _) => Some(index),
        (false, None, true, true) => None,
        (false, None, _, _) => Some(previous.unwrap_or(0).min(rows.len() - 1)),
    };
    {
        let state = state_mut(screen, section);
        let moved = next != previous;
        state.selected = next;
        if moved && next.is_some() {
            state.reveal(rows.len(), window);
        } else {
            let visible = window.max(1);
            let max_offset = rows.len().saturating_sub(visible);
            state.offset = state.offset.min(max_offset);
        }
    }
    if fetching && remembered_id.is_some() && found.is_none() {
        return;
    }
    let id = next.and_then(|index| rows.get(index).map(|row| row.id.clone()));
    remember(screen, section, id);
}

fn remembered(screen: &Screen, section: Section) -> Option<String> {
    match section {
        Section::NeedsReview => screen.needs_id.clone(),
        Section::Waiting => screen.waiting_id.clone(),
    }
}

fn remember(screen: &mut Screen, section: Section, id: Option<String>) {
    match section {
        Section::NeedsReview => screen.needs_id = id,
        Section::Waiting => screen.waiting_id = id,
    }
}

fn held_row(screen: &Screen, fetching: bool) -> Option<Row> {
    if fetching {
        let id = remembered(screen, screen.section)?;
        screen.held.clone().filter(|row| row.id == id)
    } else {
        None
    }
}

fn state_mut(screen: &mut Screen, section: Section) -> &mut ListState {
    match section {
        Section::NeedsReview => &mut screen.needs,
        Section::Waiting => &mut screen.waiting,
    }
}

fn chosen<'a>(rows: &[&'a Row], state: &ListState) -> Option<&'a Row> {
    state.selected.and_then(|index| rows.get(index).copied())
}

fn active(screen: &Screen) -> &ListState {
    match screen.section {
        Section::NeedsReview => &screen.needs,
        Section::Waiting => &screen.waiting,
    }
}

fn active_mut(screen: &mut Screen) -> &mut ListState {
    match screen.section {
        Section::NeedsReview => &mut screen.needs,
        Section::Waiting => &mut screen.waiting,
    }
}

fn section_name(section: Section) -> &'static str {
    match section {
        Section::NeedsReview => "Needs review",
        Section::Waiting => "Waiting",
    }
}

fn table_cells(rows: &[&Row], now_ms: u64, ignored: &BTreeSet<String>) -> Vec<[String; 5]> {
    rows.iter()
        .map(|row| {
            [
                format!("{}/{}#{}", row.project, row.repo, row.number),
                row.title.clone(),
                row.author.clone(),
                relative(row.updated_ms, now_ms),
                badges(row, ignored.contains(&row.id)),
            ]
        })
        .collect()
}

fn column_widths(room: u16) -> [u16; 5] {
    let budget = room.saturating_sub(4);
    let id = 11.min(budget);
    let title = 12.min(budget.saturating_sub(id));
    let author = 4.min(budget.saturating_sub(id.saturating_add(title)));
    let time = 4.min(budget.saturating_sub(id.saturating_add(title).saturating_add(author)));
    let badges = 13.min(
        budget.saturating_sub(
            id.saturating_add(title)
                .saturating_add(author)
                .saturating_add(time),
        ),
    );
    let used = id
        .saturating_add(title)
        .saturating_add(author)
        .saturating_add(time)
        .saturating_add(badges);
    let extra = budget.saturating_sub(used);
    let id_grow = 6.min(extra);
    let extra = extra.saturating_sub(id_grow);
    let author_grow = 8.min(extra);
    let extra = extra.saturating_sub(author_grow);
    let badge_grow = 4.min(extra);
    let title_grow = extra.saturating_sub(badge_grow);
    [
        id.saturating_add(id_grow),
        title.saturating_add(title_grow),
        author.saturating_add(author_grow),
        time,
        badges.saturating_add(badge_grow),
    ]
}

pub(crate) fn relative(updated_ms: u64, now_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(updated_ms) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

pub(crate) fn absolute(ms: u64, offset_secs: i32) -> String {
    let shifted = (ms / 1000) as i64 + i64::from(offset_secs);
    let shifted = shifted.max(0) as u64;
    let days = shifted / 86_400;
    let time = shifted % 86_400;
    let (year, month, day) = ymd(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        time / 3600,
        (time % 3600) / 60
    )
}

fn ymd(days: u64) -> (i32, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

fn badges(row: &Row, ignored: bool) -> String {
    badge_parts(row, ignored).join(" ")
}

fn badge_parts(row: &Row, ignored: bool) -> Vec<&'static str> {
    let mut parts = Vec::new();
    if row.draft {
        parts.push("draft");
    }
    if row.stale {
        parts.push("stale");
    }
    if row.needs_work {
        parts.push("needs work");
    }
    if row.enrichment == Enrichment::Ready {
        match row.build {
            Build::None => {}
            Build::Successful => parts.push("successful"),
            Build::InProgress => parts.push("in progress"),
            Build::Failed => parts.push("failed"),
        }
        if row.conflicted {
            parts.push("conflicted");
        }
    }
    if ignored {
        parts.push("ignored");
    }
    parts
}

fn badge_style(word: &str) -> Style {
    if word == "draft" {
        tone(Color::Magenta, false)
    } else if word == "stale" || word == "ignored" {
        tone(Color::BrightYellow, true)
    } else if word == "needs work" || word == "failed" || word == "conflicted" {
        tone(Color::BrightRed, true)
    } else if word == "in progress" {
        tone(Color::Cyan, false)
    } else {
        tone(Color::Green, false)
    }
}

pub(crate) struct FeedSpan {
    pub(crate) text: String,
    pub(crate) style: Style,
}

fn more_line(text: &str) -> Vec<FeedSpan> {
    vec![FeedSpan {
        text: text.to_owned(),
        style: tone(Color::BrightYellow, true),
    }]
}

fn feed_lines(
    row: &Row,
    user_slug: &str,
    floor: Option<u64>,
    clock: Clock,
    width: usize,
) -> Vec<Vec<FeedSpan>> {
    if width == 0 {
        return Vec::new();
    }
    let mut events = row.events.clone();
    events.sort_by(|left, right| {
        right
            .id
            .cmp(&left.id)
            .then(right.created_ms.cmp(&left.created_ms))
    });
    let mut lines = Vec::new();
    let mut shown = false;
    for event in &events {
        if keep_event(event.kind) {
            shown = true;
            lines.extend(event_rows(event, user_slug, floor, clock.now_ms, width));
        }
    }
    if shown {
        lines
    } else {
        vec![vec![FeedSpan {
            text: "No activity yet.".to_owned(),
            style: Style::default(),
        }]]
    }
}

fn keep_event(kind: EventKind) -> bool {
    match kind {
        EventKind::Commented => true,
        EventKind::Approved => true,
        EventKind::Pushed => true,
        EventKind::Reopened => true,
        EventKind::Added => true,
        EventKind::Other => false,
    }
}

fn event_rows(
    event: &bistill_lib::Event,
    user_slug: &str,
    floor: Option<u64>,
    now_ms: u64,
    width: usize,
) -> Vec<Vec<FeedSpan>> {
    let unread = floor.is_some_and(|floor| event.id > floor)
        && !event.actor_slug.eq_ignore_ascii_case(user_slug);
    let mut rows = vec![rule_spans(unread, &ago(event.created_ms, now_ms), width)];
    rows.push(vec![FeedSpan {
        text: verb_line(event, user_slug),
        style: Style::default(),
    }]);
    if event.text.is_empty() {
        rows
    } else {
        for line in wrap_text(&event.text, width) {
            rows.push(vec![FeedSpan {
                text: line,
                style: Style::default(),
            }]);
        }
        rows
    }
}

pub(crate) fn verb_line(event: &bistill_lib::Event, user_slug: &str) -> String {
    let you = event.actor_slug.eq_ignore_ascii_case(user_slug);
    match event.kind {
        EventKind::Commented if you => "You commented".to_owned(),
        EventKind::Commented => format!("Comment from {}", event.actor_name),
        EventKind::Approved if you => "You approved".to_owned(),
        EventKind::Approved => format!("Approved by {}", event.actor_name),
        EventKind::Pushed if you => "You pushed".to_owned(),
        EventKind::Pushed => format!("Pushed by {}", event.actor_name),
        EventKind::Reopened if you => "You reopened".to_owned(),
        EventKind::Reopened => format!("Reopened by {}", event.actor_name),
        EventKind::Added => format!("{} added you", event.actor_name),
        EventKind::Other => String::new(),
    }
}

pub(crate) fn rule_spans(unread: bool, time: &str, width: usize) -> Vec<FeedSpan> {
    let dim = tone(Color::BrightBlack, false);
    let mut spans = Vec::new();
    if width == 0 {
        return spans;
    }
    let mut used = 0usize;
    if unread && width >= 4 {
        spans.push(FeedSpan {
            text: "new".to_owned(),
            style: tone(Color::Green, true),
        });
        spans.push(FeedSpan {
            text: " ".to_owned(),
            style: dim,
        });
        used = 4;
    } else if unread && width >= 3 {
        spans.push(FeedSpan {
            text: "new".to_owned(),
            style: tone(Color::Green, true),
        });
        used = 3;
    }
    let time = tui::truncate(time, width - used);
    let time_w = tui::display_width(&time);
    let gap = width - used - time_w;
    if gap > 0 && time_w > 0 {
        spans.push(FeedSpan {
            text: format!("{} ", "─".repeat(gap - 1)),
            style: dim,
        });
    } else if gap > 0 {
        spans.push(FeedSpan {
            text: "─".repeat(gap),
            style: dim,
        });
    }
    if time_w > 0 {
        spans.push(FeedSpan {
            text: time,
            style: dim,
        });
    }
    spans
}

pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if width == 0 {
        return lines;
    }
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
        } else {
            let mut rest = paragraph;
            while !rest.is_empty() {
                let (line, next) = take_width(rest, width);
                lines.push(line);
                rest = next;
            }
        }
    }
    lines
}

fn take_width(text: &str, width: usize) -> (String, &str) {
    let mut used = 0usize;
    let mut end = 0usize;
    for (index, ch) in text.char_indices() {
        let mut buf = [0u8; 4];
        let piece = ch.encode_utf8(&mut buf);
        let wide = tui::display_width(piece);
        if used > 0 && used + wide > width {
            break;
        }
        used += wide;
        end = index + ch.len_utf8();
        if used >= width {
            break;
        }
    }
    let (line, rest) = text.split_at(end);
    (line.to_owned(), rest)
}

/// How long until a visible relative time can change. `None` when nothing on
/// screen has a timestamp to advance.
pub(crate) fn next_age(snapshot: Option<&Snapshot>, now_ms: u64) -> Option<Duration> {
    let snapshot = snapshot?;
    let mut wait = None;
    for row in snapshot.needs_review.iter().chain(snapshot.waiting.iter()) {
        consider(&mut wait, row.updated_ms, now_ms, false);
        for event in &row.events {
            consider(&mut wait, event.created_ms, now_ms, true);
        }
    }
    wait.map(Duration::from_millis)
}

fn consider(wait: &mut Option<u64>, then_ms: u64, now_ms: u64, ago_text: bool) {
    if then_ms > now_ms {
        return;
    }
    let elapsed = now_ms - then_ms;
    let unit = if ago_text {
        ago_unit(elapsed)
    } else {
        relative_unit(elapsed)
    };
    let rem = elapsed % unit;
    let step = if rem == 0 { unit } else { unit - rem };
    *wait = Some(match *wait {
        Some(best) => best.min(step),
        None => step,
    });
}

fn relative_unit(elapsed: u64) -> u64 {
    if elapsed < 60_000 {
        1_000
    } else if elapsed < 3_600_000 {
        60_000
    } else if elapsed < 86_400_000 {
        3_600_000
    } else {
        86_400_000
    }
}

fn ago_unit(elapsed: u64) -> u64 {
    if elapsed < 3_600_000 {
        60_000
    } else if elapsed < 86_400_000 {
        3_600_000
    } else {
        86_400_000
    }
}

/// Relative time for an event rule. Under a minute is `just now`.
pub(crate) fn ago(then_ms: u64, now_ms: u64) -> String {
    let elapsed = now_ms.saturating_sub(then_ms);
    if elapsed < 60_000 {
        "just now".to_owned()
    } else if elapsed < 3_600_000 {
        format!("{}m ago", elapsed / 60_000)
    } else if elapsed < 86_400_000 {
        format!("{}h ago", elapsed / 3_600_000)
    } else {
        format!("{}d ago", elapsed / 86_400_000)
    }
}

fn tone(fg: Color, bold: bool) -> Style {
    Style {
        fg: Some(fg),
        bold,
        ..Style::default()
    }
}

fn keys_style() -> Style {
    Style {
        bold: true,
        ..Style::default()
    }
}

fn footer_style(role: &Role) -> Style {
    match role {
        Role::Holder(Phase::Fetching) => tone(Color::Cyan, true),
        Role::Holder(Phase::RateLimited) => tone(Color::BrightYellow, true),
        Role::Holder(Phase::Auth) => tone(Color::BrightRed, true),
        Role::Holder(Phase::Tls) => tone(Color::BrightRed, true),
        Role::Holder(Phase::Unreachable { .. }) => tone(Color::BrightRed, true),
        Role::Holder(Phase::Failed { .. }) => tone(Color::BrightRed, true),
        Role::Holder(Phase::Ready) => Style::default(),
    }
}

fn paint_footer(buffer: &mut Buffer, area: Rect, status: &str, status_style: Style, keys: &str) {
    let keys_style = keys_style();
    let line = if status.is_empty() {
        tui::Line {
            spans: vec![Span {
                style: keys_style,
                content: keys,
            }],
        }
    } else {
        tui::Line {
            spans: vec![
                Span {
                    style: status_style,
                    content: status,
                },
                Span {
                    style: keys_style,
                    content: "  ",
                },
                Span {
                    style: keys_style,
                    content: keys,
                },
            ],
        }
    };
    buffer.set_line(area.x, area.y, &line, area.width);
}

pub(crate) fn paint_feed(buffer: &mut Buffer, area: Rect, rows: &[Vec<FeedSpan>]) {
    if area.width == 0 {
        return;
    }
    if area.height == 0 {
        return;
    }
    let limit = usize::from(area.height);
    for (row, spans) in rows.iter().take(limit).enumerate() {
        let y = area.y.saturating_add(row as u16);
        let mut x = area.x;
        let mut left = area.width;
        for span in spans {
            if left == 0 {
                break;
            }
            let used = buffer.set_span(
                x,
                y,
                &tui::Span {
                    style: span.style,
                    content: &span.text,
                },
                left,
            );
            x = x.saturating_add(used);
            left = left.saturating_sub(used);
        }
    }
}

fn paint_metadata(
    buffer: &mut Buffer,
    area: Rect,
    len: usize,
    state: &ListState,
    widths: &[u16; 5],
) {
    let time_x = area
        .x
        .saturating_add(widths[0])
        .saturating_add(1)
        .saturating_add(widths[1])
        .saturating_add(1)
        .saturating_add(widths[2])
        .saturating_add(1);
    let quiet = tone(Color::BrightBlack, false);
    let shown = usize::from(area.height.saturating_sub(1));
    for row in 0..shown {
        let index = state.offset + row;
        if index < len && state.selected != Some(index) {
            let y = area
                .y
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            paint_slot(buffer, area.x, y, widths[0], quiet);
            paint_slot(buffer, time_x, y, widths[3], quiet);
        }
    }
}

fn paint_slot(buffer: &mut Buffer, x: u16, y: u16, width: u16, style: Style) {
    let mut column = x;
    for _ in 0..width {
        buffer.set_style(column, y, style);
        column = column.saturating_add(1);
    }
}

fn paint_position(buffer: &mut Buffer, area: Rect, style: Style) {
    if area.height > 0 {
        let y = area.y.saturating_add(area.height - 1);
        paint_slot(buffer, area.x, y, area.width, style);
    }
}

fn paint_badges(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[&Row],
    ignored: &BTreeSet<String>,
    state: &ListState,
    widths: &[u16],
) {
    let mut origin = area.x;
    for slot in widths.iter().take(4) {
        origin = origin.saturating_add(*slot).saturating_add(1);
    }
    let slot = widths.get(4).copied().unwrap_or(0);
    let shown = usize::from(area.height.saturating_sub(1));
    for row in 0..shown {
        let index = state.offset + row;
        if index >= rows.len() || state.selected == Some(index) {
            continue;
        }
        let y = area
            .y
            .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
        recolor_badges(
            buffer,
            origin,
            y,
            slot,
            &badge_parts(rows[index], ignored.contains(&rows[index].id)),
        );
    }
}

fn recolor_badges(buffer: &mut Buffer, x: u16, y: u16, slot: u16, parts: &[&str]) {
    let mut column = x;
    let end = x.saturating_add(slot);
    let mut drawing = true;
    for (index, part) in parts.iter().enumerate() {
        if drawing && index > 0 {
            drawing = advance_badge(buffer, &mut column, end, y, ' ', Style::default());
        }
        let look = badge_style(part);
        for ch in part.chars() {
            if drawing {
                drawing = advance_badge(buffer, &mut column, end, y, ch, look);
            }
        }
    }
}

fn advance_badge(
    buffer: &mut Buffer,
    column: &mut u16,
    end: u16,
    y: u16,
    ch: char,
    style: Style,
) -> bool {
    let fits = *column < end && same_char(buffer, *column, y, ch);
    if fits {
        buffer.set_style(*column, y, style);
        *column = column.saturating_add(1);
    }
    fits
}

#[rustfmt::skip]
fn same_char(buffer: &Buffer, x: u16, y: u16, ch: char) -> bool {
    buffer.get(x, y).is_some_and(|cell| matches!(&cell.glyph, tui::Glyph::Single(text) | tui::Glyph::Wide(text) if text.starts_with(ch))) }

fn footer(role: &Role, offset_secs: i32) -> String {
    match role {
        Role::Holder(Phase::Fetching) => "Fetching from Bitbucket...".to_owned(),
        Role::Holder(Phase::Ready) => String::new(),
        Role::Holder(Phase::Auth) => "Token rejected.".to_owned(),
        Role::Holder(Phase::Tls) => "curl failed TLS.".to_owned(),
        Role::Holder(Phase::Unreachable { since_ms }) => {
            format!(
                "Bitbucket unreachable (since {}).",
                absolute(*since_ms, offset_secs)
            )
        }
        Role::Holder(Phase::RateLimited) => "Rate limited.".to_owned(),
        Role::Holder(Phase::Failed { message }) => message.clone(),
    }
}

fn draw_line(buffer: &mut Buffer, area: Rect, text: &str, style: Style) {
    buffer.set_span(
        area.x,
        area.y,
        &tui::Span {
            style,
            content: text,
        },
        area.width,
    );
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            tabs: blank(),
            table: blank(),
            detail: blank(),
            table_inner: blank(),
            detail_inner: blank(),
            footer: blank(),
        }
    }
}

fn blank() -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    }
}
