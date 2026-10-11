// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Webview geometry conversions for `ops::webview`.
//!
//! Qt and tauri disagree on pixel units: `QWidget::geometry()`,
//! `QWidget::resize()` and `QWidget::move()` are in device-independent
//! ("logical") pixels, while tauri's `Physical*` values are device pixels. The
//! setters therefore tell the C++ side *which* unit the caller used (it applies
//! the widget's `devicePixelRatioF()` in exactly one place), and the getters
//! report live widget geometry as physical values with the position taken as-is
//! and the size clamped to the unsigned range.
//!
//! Qt widget geometry is integral logical pixels, so the physical setter →
//! physical getter round trip can differ by up to half a logical pixel; that is
//! Qt's granularity, not a value invented here.

use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size};

/// The `(width, height, physical)` triple the bridge's `qt_webview_set_size`
/// takes: `physical` tells the C++ side whether to divide by the dpr.
pub fn size_params(size: Size) -> (i32, i32, bool) {
    match size {
        Size::Physical(size) => (size.width as i32, size.height as i32, true),
        Size::Logical(size) => (size.width as i32, size.height as i32, false),
    }
}

/// The `(x, y, physical)` triple the bridge's `qt_webview_set_position` takes.
pub fn position_params(position: Position) -> (i32, i32, bool) {
    match position {
        Position::Physical(position) => (position.x, position.y, true),
        Position::Logical(position) => (position.x as i32, position.y as i32, false),
    }
}

/// Live widget size (logical pixels) → physical size; Qt reports `-1` for a
/// widget that has no size yet, which is not a size but "unknown".
pub fn geometry_size(width: i32, height: i32) -> PhysicalSize<u32> {
    PhysicalSize::new(width.max(0) as u32, height.max(0) as u32)
}

/// Live widget geometry (logical pixels) → the trait's `Rect`.
pub fn geometry_rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect {
        position: Position::Physical(PhysicalPosition::new(x, y)),
        size: Size::Physical(geometry_size(width, height)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_runtime::dpi::{LogicalPosition, LogicalSize};

    #[test]
    fn size_params_flag_physical_values() {
        assert_eq!(
            size_params(Size::Physical(PhysicalSize::new(10u32, 20u32))),
            (10, 20, true)
        );
        assert_eq!(
            size_params(Size::Logical(LogicalSize::new(10.5, 20.0))),
            (10, 20, false)
        );
    }

    #[test]
    fn position_params_flag_physical_values() {
        assert_eq!(
            position_params(Position::Physical(PhysicalPosition::new(-5i32, 20i32))),
            (-5, 20, true)
        );
        assert_eq!(
            position_params(Position::Logical(LogicalPosition::new(-5.5, 20.0))),
            (-5, 20, false)
        );
    }

    #[test]
    fn geometry_size_clamps_negative_components() {
        assert_eq!(geometry_size(30, 40), PhysicalSize::new(30u32, 40u32));
        assert_eq!(geometry_size(-3, 40), PhysicalSize::new(0u32, 40u32));
        assert_eq!(geometry_size(-1, -1), PhysicalSize::new(0u32, 0u32));
    }

    #[test]
    fn geometry_rect_is_physical_and_clamped() {
        let rect = geometry_rect(-5, 6, -3, 40);
        assert_eq!(
            rect.position,
            Position::Physical(PhysicalPosition::new(-5i32, 6i32))
        );
        assert_eq!(rect.size, Size::Physical(PhysicalSize::new(0u32, 40u32)));
    }
}
