#![cfg_attr(not(test), allow(dead_code))]

//! The inbox screen.
//!
//! One view: tabs, the pull-request table, the detail pane, and a footer.
//! At 100 columns the table and the detail sit side by side. Below that the
//! table is above the detail.

use bistill_lib::{Build, Enrichment, ReviewStatus, Row, Section, Snapshot};
use tui::{
    Buffer, Constraint, Direction, Event, Input, KeyCode, ListState, Rect, Style, Wheel,
    draw_block, draw_input, draw_paragraph, draw_table, draw_tabs, hit_row, inner, split,
};

const HELP: &str = "\
j / k    Move the selection
Enter    Open the pull request
r        Refresh
/        Filter title, repo, and author
Tab      Switch section
?        List the keys
q        Quit";

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

/// Who is drawing.
#[derive(Eq, PartialEq)]
pub(crate) enum Role {
    /// This process holds the lock.
    Holder(Phase),
    /// Another live pid holds the lock.
    Viewer {
        /// The holder's pid.
        pid: u32,
    },
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
    table_len: usize,
    table_window: usize,
    detail_window: usize,
    last_click: Option<(usize, u64)>,
    frame: Frame,
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
            table_len: 0,
            table_window: 0,
            detail_window: 0,
            last_click: None,
            frame: Frame::default(),
        }
    }
}

/// Clock values the screen formats. `offset_secs` is the OS zone offset.
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
    let marked = Style {
        bold: true,
        ..Style::default()
    };
    let reverse = Style {
        reverse: true,
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
            plain,
            marked,
        );
    }
    let rows = visible(snapshot, screen.section, &screen.filter);
    screen.table_len = rows.len();
    select_existing(screen, rows.len());
    screen.table_window = usize::from(frame.table_inner.height.saturating_sub(1));
    screen.detail_window = usize::from(frame.detail_inner.height);
    draw_block(
        buffer,
        frame.table,
        section_name(screen.section),
        screen.focus == Pane::Table,
        plain,
        marked,
    );
    if rows.is_empty() && screen.filter.is_empty() && snapshot.is_some() {
        draw_paragraph(
            buffer,
            frame.table_inner,
            "Nothing needs your attention.",
            plain,
        );
    } else {
        let owned = table_cells(&rows, clock.now_ms);
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
        draw_table(
            buffer,
            frame.table_inner,
            &refs,
            &column_widths(frame.table_inner.width),
            state,
            plain,
            reverse,
        );
    }
    let detail = chosen(&rows, active(screen)).map(|row| detail_text(row, clock.offset_secs));
    let mut detail = detail.unwrap_or_default();
    if let Some(snapshot) = snapshot.filter(|snapshot| snapshot.truncated > 0) {
        detail.insert_str(0, &format!("and {} more\n", snapshot.truncated));
    }
    screen.detail_lines = detail.lines().count();
    screen.detail_offset = screen.detail_offset.min(
        screen
            .detail_lines
            .saturating_sub(screen.detail_window.max(1)),
    );
    let shown = skip_lines(&detail, screen.detail_offset);
    draw_block(
        buffer,
        frame.detail,
        "Detail",
        screen.focus == Pane::Detail,
        plain,
        marked,
    );
    draw_paragraph(buffer, frame.detail_inner, &shown, plain);
    let status = footer(role, snapshot.is_some(), clock.offset_secs);
    draw_line(buffer, frame.footer, &status, plain);
    if screen.editing {
        draw_line(buffer, frame.footer, "/", plain);
        let field = Rect {
            x: frame.footer.x.saturating_add(1),
            y: frame.footer.y,
            width: frame.footer.width.saturating_sub(1),
            height: frame.footer.height,
        };
        draw_input(buffer, field, &screen.draft, plain);
    }
    if screen.help {
        draw_block(buffer, full(buffer), "Keys", true, plain, marked);
        draw_paragraph(buffer, inner(full(buffer)), HELP, plain);
    }
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
        let len = visible(snapshot, section, &screen.filter).len();
        let state = match section {
            Section::NeedsReview => &mut screen.needs,
            Section::Waiting => &mut screen.waiting,
        };
        state.selected = match state.selected {
            Some(index) if index < len => Some(index),
            Some(_) if len > 0 => Some(len - 1),
            _ => None,
        };
        state.offset = state.offset.min(len);
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
                active_mut(screen).selected = Some(index);
                let open = match screen.last_click {
                    Some((previous, at))
                        if previous == index && now_ms.saturating_sub(at) <= 400 =>
                    {
                        true
                    }
                    Some((previous, _)) if previous == index => false,
                    Some(_) => false,
                    None => false,
                };
                screen.last_click = Some((index, now_ms));
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

fn select_existing(screen: &mut Screen, len: usize) {
    let state = active_mut(screen);
    match (len, state.selected) {
        (0, _) => state.selected = None,
        (_, None) => state.selected = Some(0),
        (_, Some(index)) if index >= len => state.selected = Some(len - 1),
        (_, Some(_)) => {}
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

fn table_cells(rows: &[&Row], now_ms: u64) -> Vec<[String; 5]> {
    rows.iter()
        .map(|row| {
            [
                format!("{}/{}#{}", row.project, row.repo, row.number),
                row.title.clone(),
                row.author.clone(),
                relative(row.updated_ms, now_ms),
                badges(row),
            ]
        })
        .collect()
}

fn column_widths(room: u16) -> [u16; 5] {
    let budget = room.saturating_sub(4);
    let id = 16.min(budget);
    let rest = budget.saturating_sub(id);
    let author = 12.min(rest);
    let rest = rest.saturating_sub(author);
    let time = 4.min(rest);
    let rest = rest.saturating_sub(time);
    let badges = 14.min(rest);
    let title = rest.saturating_sub(badges);
    [id, title, author, time, badges]
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

fn badges(row: &Row) -> String {
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
    parts.join(" ")
}

fn detail_text(row: &Row, offset_secs: i32) -> String {
    let mut lines = Vec::new();
    lines.push(row.title.clone());
    for reviewer in &row.reviewers {
        lines.push(format!(
            "{} {}",
            reviewer.name,
            status_word(reviewer.status)
        ));
    }
    lines.push(format!("{} -> {}", row.from_branch, row.to_branch));
    if row.enrichment == Enrichment::Ready {
        lines.push(format!("unanswered as author {}", row.unanswered_as_author));
        lines.push(format!(
            "unanswered as reviewer {}",
            row.unanswered_as_reviewer
        ));
        lines.push(format!("open tasks {}", row.open_tasks));
        if let Some(word) = build_word(row.build) {
            lines.push(format!("build {word}"));
        }
        if row.conflicted {
            lines.push("conflicted".to_owned());
        }
        if row.can_merge {
            lines.push("mergeable".to_owned());
        }
    }
    lines.push(absolute(row.updated_ms, offset_secs));
    lines.push(row.html_url.clone());
    lines.join("\n")
}

fn status_word(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Unapproved => "UNAPPROVED",
        ReviewStatus::NeedsWork => "NEEDS_WORK",
        ReviewStatus::Approved => "APPROVED",
    }
}

fn build_word(build: Build) -> Option<&'static str> {
    match build {
        Build::None => None,
        Build::Successful => Some("successful"),
        Build::InProgress => Some("in progress"),
        Build::Failed => Some("failed"),
    }
}

fn footer(role: &Role, loaded: bool, offset_secs: i32) -> String {
    match role {
        Role::Viewer { .. } if !loaded => "Fetching from Bitbucket...".to_owned(),
        Role::Viewer { pid } => format!("Holder {pid}."),
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

fn skip_lines(text: &str, offset: usize) -> String {
    text.lines().skip(offset).collect::<Vec<_>>().join("\n")
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

fn full(buffer: &Buffer) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: buffer.width(),
        height: buffer.height(),
    }
}
