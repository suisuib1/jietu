use screenshots::Screen;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

pub(crate) fn screen_rect(screen: &Screen) -> Rect {
    let info = screen.display_info;
    Rect {
        x: info.x as f64,
        y: info.y as f64,
        width: info.width as f64,
        height: info.height as f64,
    }
}

pub(crate) fn rect_distance_squared(rect: Rect, point: (f64, f64)) -> f64 {
    let right = rect.x + rect.width;
    let bottom = rect.y + rect.height;
    let dx = if point.0 < rect.x {
        rect.x - point.0
    } else if point.0 > right {
        point.0 - right
    } else {
        0.0
    };
    let dy = if point.1 < rect.y {
        rect.y - point.1
    } else if point.1 > bottom {
        point.1 - bottom
    } else {
        0.0
    };
    dx * dx + dy * dy
}

pub(crate) fn select_screen_containing_point(
    screens: &[Screen],
    point: (f64, f64),
) -> Option<Screen> {
    screens
        .iter()
        .filter(|screen| {
            let rect = screen_rect(screen);
            point.0 >= rect.x
                && point.0 < rect.x + rect.width
                && point.1 >= rect.y
                && point.1 < rect.y + rect.height
        })
        .min_by_key(|screen| {
            let rect = screen_rect(screen);
            (rect.width * rect.height) as u64
        })
        .copied()
}

pub(crate) fn nearest_screen(screens: &[Screen], point: (f64, f64)) -> Option<Screen> {
    screens
        .iter()
        .min_by(|left, right| {
            rect_distance_squared(screen_rect(left), point)
                .total_cmp(&rect_distance_squared(screen_rect(right), point))
        })
        .copied()
}

pub(crate) fn screen_geometry_score(screen: &Screen, logical: &Rect, physical: &Rect) -> f64 {
    let info = screen_rect(screen);
    let score = |expected: &Rect| {
        let position = (info.x - expected.x).abs() + (info.y - expected.y).abs();
        let size = (info.width - expected.width).abs() + (info.height - expected.height).abs();
        position + size
    };
    score(logical).min(score(physical))
}

pub(crate) fn logical_region(rect: Rect, bounds: Rect) -> (i32, i32, u32, u32) {
    let x = rect.x.floor().max(0.0) as i32;
    let y = rect.y.floor().max(0.0) as i32;
    let right = (rect.x + rect.width)
        .ceil()
        .min(bounds.width)
        .max(x as f64 + 1.0) as u32;
    let bottom = (rect.y + rect.height)
        .ceil()
        .min(bounds.height)
        .max(y as f64 + 1.0) as u32;
    (
        x,
        y,
        right.saturating_sub(x as u32).max(1),
        bottom.saturating_sub(y as u32).max(1),
    )
}
