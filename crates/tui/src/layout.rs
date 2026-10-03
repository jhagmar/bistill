//! Rectangles and splits.

/// A region of cells. `x` and `y` are the top-left column and row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    /// Left column.
    pub x: u16,
    /// Top row.
    pub y: u16,
    /// Width in columns.
    pub width: u16,
    /// Height in rows.
    pub height: u16,
}

/// Which axis [`split`] divides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// Divide `width`, keeping `height`.
    Horizontal,
    /// Divide `height`, keeping `width`.
    Vertical,
}

/// One piece of a [`split`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Constraint {
    /// This many cells.
    Fixed(u16),
    /// At least this many cells, then a share of any surplus.
    Min(u16),
    /// This percent of the parent length. Values above 100 count as 100.
    Percent(u16),
}

/// Divide `area` into one rectangle per constraint.
pub fn split(area: Rect, direction: Direction, constraints: &[Constraint]) -> Vec<Rect> {
    let total = axis_length(area, direction);
    let mut lengths = vec![0u32; constraints.len()];
    let mut remaining = total;
    let mut mins = Vec::new();
    for (index, constraint) in constraints.iter().enumerate() {
        match constraint {
            Constraint::Fixed(value) => {
                let take = remaining.min(u32::from(*value));
                lengths[index] = take;
                remaining -= take;
            }
            Constraint::Percent(value) => {
                let claim = total * u32::from((*value).min(100)) / 100;
                let take = remaining.min(claim);
                lengths[index] = take;
                remaining -= take;
            }
            Constraint::Min(value) => mins.push((index, u32::from(*value))),
        }
    }
    for (index, min) in &mins {
        let take = remaining.min(*min);
        lengths[*index] = take;
        remaining -= take;
    }
    let mut cursor = 0;
    while remaining > 0 && !mins.is_empty() {
        let (index, _) = mins[cursor % mins.len()];
        lengths[index] += 1;
        remaining -= 1;
        cursor += 1;
    }
    let mut origin = axis_origin(area, direction);
    let mut rects = Vec::with_capacity(lengths.len());
    for length in lengths {
        rects.push(piece(area, direction, origin, length));
        origin += length;
    }
    rects
}

fn axis_length(area: Rect, direction: Direction) -> u32 {
    match direction {
        Direction::Horizontal => u32::from(area.width),
        Direction::Vertical => u32::from(area.height),
    }
}

fn axis_origin(area: Rect, direction: Direction) -> u32 {
    match direction {
        Direction::Horizontal => u32::from(area.x),
        Direction::Vertical => u32::from(area.y),
    }
}

fn piece(area: Rect, direction: Direction, origin: u32, length: u32) -> Rect {
    let origin = origin as u16;
    let length = length as u16;
    match direction {
        Direction::Horizontal => Rect {
            x: origin,
            y: area.y,
            width: length,
            height: area.height,
        },
        Direction::Vertical => Rect {
            x: area.x,
            y: origin,
            width: area.width,
            height: length,
        },
    }
}
