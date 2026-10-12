// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Window conversions and builder mapping: the pure half of `ops::window` plus
//! the `WindowConfig` → `QtWindowBuilder` field mapping from `lib.rs`.
//!
//! Qt reports geometry in device-independent ("logical") pixels while tauri asks
//! for physical ones, so every geometry value crosses the boundary through the
//! window's own device pixel ratio. Qt takes `int`s for sizes and positions and
//! sizes its hints with 24-bit values, hence the clamping rules here. Nothing in
//! this module knows about Qt objects: the runtime keeps the FFI calls, the
//! "platform cannot honour this" errors and the trace lines.

use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, PixelUnit, Position, Size};
use tauri_runtime::window::WindowSizeConstraints;
use tauri_runtime::ResizeDirection;
use tauri_utils::config::WindowConfig;
use tauri_utils::Theme;

/// Qt's largest usable window dimension (`QWIDGETSIZE_MAX`): the sentinel of
/// "no maximum size".
pub const QWIDGETSIZE_MAX: f64 = (1 << 24) as f64 - 1.0;

/// Qt's "no minimum size": `QSize(0, 0)`.
pub const NO_MINIMUM: (f64, f64) = (0.0, 0.0);

/// Qt's "no maximum size": both axes at [`QWIDGETSIZE_MAX`].
pub const NO_MAXIMUM: (f64, f64) = (QWIDGETSIZE_MAX, QWIDGETSIZE_MAX);

/// The 1..8 code the C++ side expects for `startSystemResize` (it maps the code
/// to `Qt::Edges`), in tauri's [`ResizeDirection`] order.
pub fn resize_direction_code(direction: ResizeDirection) -> i32 {
    match direction {
        ResizeDirection::East => 1,
        ResizeDirection::North => 2,
        ResizeDirection::NorthEast => 3,
        ResizeDirection::NorthWest => 4,
        ResizeDirection::South => 5,
        ResizeDirection::SouthEast => 6,
        ResizeDirection::SouthWest => 7,
        ResizeDirection::West => 8,
    }
}

/// Whether the Qt platform plugin lets clients place and stack their own
/// top-level windows (X11). A Wayland compositor owns placement and exposes no
/// stacking request, so `always_on_top`/`always_on_bottom` must refuse there.
///
/// This is *not* the gate for positions: Qt remembers the position an app asked
/// for and reports it back, so `set_position`/`inner_position`/`outer_position`
/// answer on every platform (see `docs/wayland-ci.md`).
pub fn placement_supported(platform: &str) -> bool {
    matches!(platform, "xcb" | "xlib")
}

/// Parses the `"WxH"` payload of a resize event. Sizes are unsigned (Qt reports
/// pixels), so a negative or unparsable part reads as 0 — the C++ side reports
/// best-effort values, and 0 is the only honest stand-in for "Qt did not say".
pub fn parse_resize_payload(payload: &str) -> (u32, u32) {
    let mut parts = payload.split('x');
    (
        parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    )
}

/// Parses the `"x,y"` payload of a move event; positions are signed and a part
/// that does not parse reads as 0, like [`parse_resize_payload`].
pub fn parse_move_payload(payload: &str) -> (i32, i32) {
    let mut parts = payload.split(',');
    (
        parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    )
}

/// The window's device pixel ratio (physical pixels per logical pixel). Qt
/// reports `0` before the window has a screen; a zero ratio would collapse every
/// value derived from it, so it reads as 1.
pub fn device_pixel_ratio(dpr: f64) -> f64 {
    if dpr > 0.0 {
        dpr
    } else {
        1.0
    }
}

/// Qt logical pixels → tauri physical pixels.
pub fn to_physical_size(dpr: f64, width: f64, height: f64) -> PhysicalSize<u32> {
    PhysicalSize::new((width * dpr).round() as u32, (height * dpr).round() as u32)
}

/// Qt logical pixels → tauri physical pixels.
pub fn to_physical_position(dpr: f64, x: f64, y: f64) -> PhysicalPosition<i32> {
    PhysicalPosition::new((x * dpr).round() as i32, (y * dpr).round() as i32)
}

/// Qt size hints are 24-bit: clamp so no `f64` can overflow the `int` Qt takes.
/// A non-finite value (e.g. the `PixelUnit::MAX` sentinel) means "no
/// constraint", i.e. the maximum.
pub fn qt_dimension(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, QWIDGETSIZE_MAX)
    }
}

/// A tauri size in Qt logical pixels.
pub fn qt_size(dpr: f64, size: Size) -> (f64, f64) {
    let (width, height) = match size {
        Size::Logical(size) => (size.width, size.height),
        Size::Physical(size) => (f64::from(size.width) / dpr, f64::from(size.height) / dpr),
    };
    (qt_dimension(width), qt_dimension(height))
}

/// A tauri size-constraint value in Qt logical pixels (`missing` when the
/// constraint is unspecified).
pub fn qt_pixel_unit(dpr: f64, unit: Option<PixelUnit>, missing: f64) -> f64 {
    match unit {
        None => missing,
        Some(PixelUnit::Logical(unit)) => qt_dimension(unit.0),
        Some(PixelUnit::Physical(unit)) => qt_dimension(f64::from(unit.0) / dpr),
    }
}

/// The four Qt size hints a [`WindowSizeConstraints`] maps to (logical pixels,
/// unspecified constraints at Qt's sentinels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeHints {
    pub min_width: f64,
    pub min_height: f64,
    pub max_width: f64,
    pub max_height: f64,
}

/// [`WindowSizeConstraints`] → Qt's `setMinimumSize`/`setMaximumSize` arguments.
pub fn size_hints(dpr: f64, constraints: WindowSizeConstraints) -> SizeHints {
    SizeHints {
        min_width: qt_pixel_unit(dpr, constraints.min_width, NO_MINIMUM.0),
        min_height: qt_pixel_unit(dpr, constraints.min_height, NO_MINIMUM.1),
        max_width: qt_pixel_unit(dpr, constraints.max_width, NO_MAXIMUM.0),
        max_height: qt_pixel_unit(dpr, constraints.max_height, NO_MAXIMUM.1),
    }
}

/// A tauri position in Qt logical pixels (Qt takes `int`s, so it is rounded).
pub fn window_position(dpr: f64, position: Position) -> (i32, i32) {
    let (x, y) = match position {
        Position::Logical(position) => (position.x, position.y),
        Position::Physical(position) => (f64::from(position.x) / dpr, f64::from(position.y) / dpr),
    };
    (x.round() as i32, y.round() as i32)
}

/// A tauri size as the pair of `int`s Qt takes. Both units are passed through
/// unchanged: callers of `set_size` speak pixels, not scale factors.
pub fn size_to_i32(size: Size) -> (i32, i32) {
    match size {
        Size::Physical(size) => (size.width as i32, size.height as i32),
        Size::Logical(size) => (size.width as i32, size.height as i32),
    }
}

/// The colour scheme code Qt reports → the runtime's theme. Anything else
/// (including 0 = "unknown") is no answer, not a theme.
pub fn theme_from_code(code: i32) -> Option<Theme> {
    match code {
        1 => Some(Theme::Light),
        2 => Some(Theme::Dark),
        _ => None,
    }
}

/// Number of RGBA8888 bytes an icon of these dimensions must carry.
pub fn icon_rgba_len(width: u32, height: u32) -> usize {
    (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4)
}

/// The exact message for an icon whose pixel buffer does not match its declared
/// dimensions (`None` when the icon is well-formed). A zero dimension is never a
/// valid icon, whatever the buffer length.
pub fn icon_buffer_error(width: u32, height: u32, len: usize) -> Option<String> {
    let expected = icon_rgba_len(width, height);
    if width == 0 || height == 0 || len != expected {
        return Some(format!(
            "icon is {}x{} RGBA8888 ({} bytes) but the buffer holds {} bytes",
            width, height, expected, len
        ));
    }
    None
}

/// The `WindowConfig` fields `QtWindowBuilder::with_config` copies. Everything
/// else the builder carries is a Qt default (`has_icon`) or not expressible here
/// (`min_inner_size`, `resizable`, …); `position` is always cleared because
/// Wayland cannot place toplevels.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowParams {
    pub theme: Option<Theme>,
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub visible: bool,
    pub decorations: bool,
    pub maximized: bool,
    pub center: bool,
}

/// `WindowConfig` → builder fields.
pub fn window_params(config: &WindowConfig) -> WindowParams {
    WindowParams {
        theme: config.theme,
        title: config.title.clone(),
        width: config.width,
        height: config.height,
        visible: config.visible,
        decorations: config.decorations,
        maximized: config.maximized,
        center: config.center,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_runtime::dpi::{LogicalPosition, LogicalSize, LogicalUnit, PhysicalUnit};

    fn window_config(json: &str) -> WindowConfig {
        serde_json::from_str(json).expect("valid window config")
    }

    #[test]
    fn resize_payloads_parse_two_unsigned_integers() {
        assert_eq!(parse_resize_payload("1280x1024"), (1280, 1024));
        assert_eq!(parse_resize_payload("0x0"), (0, 0));
        assert_eq!(parse_resize_payload("800x"), (800, 0));
        assert_eq!(parse_resize_payload(""), (0, 0));
        assert_eq!(parse_resize_payload("garbage"), (0, 0));
        // A negative component is not a size: it reads as 0, not as a huge u32.
        assert_eq!(parse_resize_payload("-5x10"), (0, 10));
        // Only the first two parts are read: extra ones are ignored.
        assert_eq!(parse_resize_payload("10x20x30"), (10, 20));
    }

    #[test]
    fn move_payloads_parse_two_integers() {
        assert_eq!(parse_move_payload("120,340"), (120, 340));
        assert_eq!(parse_move_payload("-5,-6"), (-5, -6));
        assert_eq!(parse_move_payload("12,"), (12, 0));
        assert_eq!(parse_move_payload("nonsense"), (0, 0));
    }

    #[test]
    fn only_x11_platforms_allow_placement() {
        assert!(placement_supported("xcb"));
        assert!(placement_supported("xlib"));
        assert!(!placement_supported("wayland"));
        assert!(!placement_supported("offscreen"));
        assert!(!placement_supported(""));
    }

    #[test]
    fn resize_directions_use_tauris_numbering() {
        assert_eq!(resize_direction_code(ResizeDirection::East), 1);
        assert_eq!(resize_direction_code(ResizeDirection::North), 2);
        assert_eq!(resize_direction_code(ResizeDirection::NorthEast), 3);
        assert_eq!(resize_direction_code(ResizeDirection::NorthWest), 4);
        assert_eq!(resize_direction_code(ResizeDirection::South), 5);
        assert_eq!(resize_direction_code(ResizeDirection::SouthEast), 6);
        assert_eq!(resize_direction_code(ResizeDirection::SouthWest), 7);
        assert_eq!(resize_direction_code(ResizeDirection::West), 8);
    }

    #[test]
    fn device_pixel_ratios_read_as_one_when_unset() {
        assert_eq!(device_pixel_ratio(1.5), 1.5);
        assert_eq!(device_pixel_ratio(0.0), 1.0);
        assert_eq!(device_pixel_ratio(-2.0), 1.0);
        assert_eq!(device_pixel_ratio(f64::NAN), 1.0);
    }

    #[test]
    fn qt_logical_values_convert_to_physical_pixels() {
        assert_eq!(
            to_physical_size(1.5, 10.0, 3.0),
            PhysicalSize::new(15u32, 5u32)
        );
        assert_eq!(
            to_physical_size(1.0, -3.0, 0.0),
            PhysicalSize::new(0u32, 0u32)
        );
        assert_eq!(
            to_physical_position(1.5, 5.0, -3.0),
            PhysicalPosition::new(8i32, -5i32)
        );
    }

    #[test]
    fn qt_dimensions_clamp_to_the_24_bit_sentinel() {
        assert_eq!(qt_dimension(-1.0), 0.0);
        assert_eq!(qt_dimension(10.5), 10.5);
        assert_eq!(qt_dimension(f64::MAX), QWIDGETSIZE_MAX);
        assert_eq!(qt_dimension(f64::INFINITY), QWIDGETSIZE_MAX);
        assert_eq!(qt_dimension(f64::NAN), 0.0);
    }

    #[test]
    fn qt_sizes_convert_physical_pixels_through_the_ratio() {
        assert_eq!(
            qt_size(2.0, Size::Logical(LogicalSize::new(100.0, 50.0))),
            (100.0, 50.0)
        );
        assert_eq!(
            qt_size(2.0, Size::Physical(PhysicalSize::new(100u32, 50u32))),
            (50.0, 25.0)
        );
        assert_eq!(
            qt_size(2.0, Size::Logical(LogicalSize::new(-5.0, 1.0))),
            (0.0, 1.0)
        );
    }

    #[test]
    fn qt_pixel_units_handle_both_units_and_the_missing_case() {
        assert_eq!(qt_pixel_unit(2.0, None, 7.0), 7.0);
        assert_eq!(
            qt_pixel_unit(2.0, Some(PixelUnit::Logical(LogicalUnit::new(120.0))), 7.0),
            120.0
        );
        assert_eq!(
            qt_pixel_unit(
                2.0,
                Some(PixelUnit::Physical(PhysicalUnit::new(120i32))),
                7.0
            ),
            60.0
        );
    }

    #[test]
    fn unspecified_constraints_use_qts_sentinels() {
        let hints = size_hints(1.0, WindowSizeConstraints::default());
        assert_eq!(
            hints,
            SizeHints {
                min_width: 0.0,
                min_height: 0.0,
                max_width: QWIDGETSIZE_MAX,
                max_height: QWIDGETSIZE_MAX,
            }
        );
        assert_eq!(hints.min_width, NO_MINIMUM.0);
        assert_eq!(hints.max_width, NO_MAXIMUM.0);
    }

    #[test]
    fn size_hints_scale_physical_constraints_through_the_ratio() {
        let constraints = WindowSizeConstraints {
            min_width: Some(PixelUnit::Physical(PhysicalUnit::new(200i32))),
            min_height: Some(PixelUnit::Logical(LogicalUnit::new(80.0))),
            max_width: Some(PixelUnit::Physical(PhysicalUnit::new(1000i32))),
            max_height: None,
        };
        assert_eq!(
            size_hints(2.0, constraints),
            SizeHints {
                min_width: 100.0,
                min_height: 80.0,
                max_width: 500.0,
                max_height: QWIDGETSIZE_MAX,
            }
        );
    }

    #[test]
    fn physical_constraints_are_clamped_like_logical_ones() {
        let constraints = WindowSizeConstraints {
            min_width: Some(PixelUnit::MAX),
            ..WindowSizeConstraints::default()
        };
        assert_eq!(size_hints(1.0, constraints).min_width, QWIDGETSIZE_MAX);
    }

    #[test]
    fn qt_sizes_are_integral() {
        assert_eq!(
            qt_size(1.0, Size::Logical(LogicalSize::new(10.5, 20.4))),
            (10.5, 20.4)
        );
        assert_eq!(
            qt_size(1.0, Size::Physical(PhysicalSize::new(10u32, 20u32))),
            (10.0, 20.0)
        );
    }

    #[test]
    fn window_positions_round_to_qt_logical_pixels() {
        assert_eq!(
            window_position(1.0, Position::Logical(LogicalPosition::new(10.5, 20.4))),
            (11, 20)
        );
        assert_eq!(
            window_position(2.0, Position::Physical(PhysicalPosition::new(30i32, 40i32))),
            (15, 20)
        );
        assert_eq!(
            window_position(2.0, Position::Physical(PhysicalPosition::new(-3i32, 5i32))),
            (-2, 3)
        );
    }

    #[test]
    fn size_to_i32_keeps_the_callers_unit() {
        assert_eq!(
            size_to_i32(Size::Logical(LogicalSize::new(10.5, 20.4))),
            (10, 20)
        );
        assert_eq!(
            size_to_i32(Size::Physical(PhysicalSize::new(30u32, 40u32))),
            (30, 40)
        );
    }

    #[test]
    fn theme_codes_map_to_light_and_dark() {
        assert_eq!(theme_from_code(1), Some(Theme::Light));
        assert_eq!(theme_from_code(2), Some(Theme::Dark));
        assert_eq!(theme_from_code(0), None);
        assert_eq!(theme_from_code(3), None);
        assert_eq!(theme_from_code(-1), None);
    }

    #[test]
    fn icon_buffers_are_checked_against_their_dimensions() {
        assert_eq!(icon_rgba_len(2, 2), 16);
        assert_eq!(icon_buffer_error(2, 2, 16), None);
        assert_eq!(
            icon_buffer_error(2, 2, 15),
            Some("icon is 2x2 RGBA8888 (16 bytes) but the buffer holds 15 bytes".to_string())
        );
        assert_eq!(
            icon_buffer_error(0, 2, 0),
            Some("icon is 0x2 RGBA8888 (0 bytes) but the buffer holds 0 bytes".to_string())
        );
    }

    #[test]
    fn icon_dimensions_cannot_overflow_the_expected_length() {
        // u32::MAX squared times four does not fit a usize; the length
        // saturates instead of wrapping to a length a buffer could "match".
        assert_eq!(icon_rgba_len(u32::MAX, u32::MAX), usize::MAX);
        assert!(icon_buffer_error(u32::MAX, u32::MAX, 0).is_some());
    }

    #[test]
    fn builder_params_copy_the_config_fields() {
        let config = window_config(
            r#"{"title":"demo","width":1024,"height":768,"visible":false,"decorations":false,"maximized":true,"center":true,"theme":"dark"}"#,
        );
        assert_eq!(
            window_params(&config),
            WindowParams {
                theme: Some(Theme::Dark),
                title: "demo".to_string(),
                width: 1024.0,
                height: 768.0,
                visible: false,
                decorations: false,
                maximized: true,
                center: true,
            }
        );
    }

    #[test]
    fn builder_params_keep_tauris_defaults_otherwise() {
        let params = window_params(&window_config("{}"));
        assert_eq!(params.theme, None);
        assert_eq!(params.width, 800.0);
        assert_eq!(params.height, 600.0);
        assert!(params.visible);
        assert!(params.decorations);
        assert!(!params.maximized);
        assert!(!params.center);
    }

    #[test]
    fn qt_size_sentinels_match_qwidget() {
        assert_eq!(QWIDGETSIZE_MAX, 16_777_215.0);
        assert_eq!(NO_MINIMUM, (0.0, 0.0));
        assert_eq!(NO_MAXIMUM, (QWIDGETSIZE_MAX, QWIDGETSIZE_MAX));
    }
}
