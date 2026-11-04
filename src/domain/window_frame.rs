//! Monitor-safe window placement in logical pixels.
//!
//! The GPUI window also receives `WindowOptions.window_min_size` of 560×420.
//! This clamp is the testable restore path: saved bounds that miss every
//! injected work area move onto the primary one. A work area smaller than the
//! minimum target wins, so the rectangle still fits on that monitor.

pub(crate) const MIN_WINDOW_WIDTH: i32 = 560;
pub(crate) const MIN_WINDOW_HEIGHT: i32 = 420;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LogicalRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl LogicalRect {
    pub(crate) fn new(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub(crate) fn x(self) -> i32 {
        self.x
    }

    pub(crate) fn y(self) -> i32 {
        self.y
    }

    pub(crate) fn width(self) -> i32 {
        self.width
    }

    pub(crate) fn height(self) -> i32 {
        self.height
    }
}

/// Keep `saved` inside `primary` when it misses every work area.
/// When it hits one, fit it to that area, growing toward the minimum target.
pub(crate) fn restore_window_bounds(
    saved: LogicalRect,
    primary: LogicalRect,
    others: &[LogicalRect],
) -> LogicalRect {
    let host = std::iter::once(primary)
        .chain(others.iter().copied())
        .find(|area| intersects(saved, *area))
        .unwrap_or(primary);
    clamp_to_work_area(saved, host)
}

fn intersects(window: LogicalRect, area: LogicalRect) -> bool {
    window.x < area.x + area.width
        && window.x + window.width > area.x
        && window.y < area.y + area.height
        && window.y + window.height > area.y
}

fn clamp_to_work_area(window: LogicalRect, area: LogicalRect) -> LogicalRect {
    let width = fit_span(window.width, MIN_WINDOW_WIDTH, area.width);
    let height = fit_span(window.height, MIN_WINDOW_HEIGHT, area.height);
    let max_x = area.x + area.width - width;
    let max_y = area.y + area.height - height;
    LogicalRect {
        x: window.x.clamp(area.x.min(max_x), max_x.max(area.x)),
        y: window.y.clamp(area.y.min(max_y), max_y.max(area.y)),
        width,
        height,
    }
}

fn fit_span(saved: i32, minimum: i32, area: i32) -> i32 {
    let area = area.max(1);
    saved.max(minimum).min(area)
}

#[cfg(test)]
mod tests {
    use super::{restore_window_bounds, LogicalRect, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH};

    fn rect(x: i32, y: i32, width: i32, height: i32) -> LogicalRect {
        LogicalRect::new(x, y, width, height).expect("rect")
    }

    #[test]
    fn off_screen_bounds_move_into_the_primary_work_area() {
        // Arrange
        let saved = rect(8_000, -4_000, 800, 600);
        let primary = rect(0, 0, 1920, 1080);

        // Act
        let restored = restore_window_bounds(saved, primary, &[]);

        // Assert
        assert!(restored.x() >= primary.x());
        assert!(restored.y() >= primary.y());
        assert!(restored.x() + restored.width() <= primary.x() + primary.width());
        assert!(restored.y() + restored.height() <= primary.y() + primary.height());
        assert!(restored.width() >= MIN_WINDOW_WIDTH);
        assert!(restored.height() >= MIN_WINDOW_HEIGHT);
    }

    #[test]
    fn tiny_bounds_grow_to_the_minimum_target() {
        // Arrange
        let saved = rect(40, 50, 100, 80);
        let primary = rect(0, 0, 1920, 1080);

        // Act
        let restored = restore_window_bounds(saved, primary, &[]);

        // Assert
        assert_eq!(restored.width(), MIN_WINDOW_WIDTH);
        assert_eq!(restored.height(), MIN_WINDOW_HEIGHT);
        assert_eq!(restored.x(), 40);
        assert_eq!(restored.y(), 50);
    }

    #[test]
    fn bounds_on_another_monitor_stay_on_that_monitor() {
        // Arrange
        let saved = rect(-1_600, 80, 900, 700);
        let primary = rect(0, 0, 1920, 1080);
        let secondary = rect(-1_920, 0, 1920, 1080);

        // Act
        let restored = restore_window_bounds(saved, primary, &[secondary]);

        // Assert
        assert!(restored.x() < 0);
        assert!(restored.x() >= secondary.x());
        assert!(restored.x() + restored.width() <= secondary.x() + secondary.width());
    }

    #[test]
    fn work_area_smaller_than_the_minimum_still_fits() {
        // Arrange
        let saved = rect(10, 10, 800, 600);
        let primary = rect(0, 0, 400, 300);

        // Act
        let restored = restore_window_bounds(saved, primary, &[]);

        // Assert
        assert_eq!(restored.width(), 400);
        assert_eq!(restored.height(), 300);
        assert_eq!(restored.x(), 0);
        assert_eq!(restored.y(), 0);
    }

    #[test]
    fn in_bounds_window_keeps_its_origin() {
        // Arrange
        let saved = rect(100, 120, 720, 480);
        let primary = rect(0, 0, 1920, 1080);

        // Act
        let restored = restore_window_bounds(saved, primary, &[]);

        // Assert
        assert_eq!(restored, saved);
    }
}
