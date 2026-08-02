//! Renderer-native window effects: theme-switch transitions and the
//! configuration shared with the backdrop-blur scene primitive.
//!
//! Theme transitions are driven by a state machine on [`crate::Window`]
//! (`Idle -> ReadyToStart -> Active -> Idle`) and composited by the platform
//! renderers from a snapshot of the pre-change frame.

use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

/// The visual style of a theme transition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransitionStyle {
    /// Rectangular reveal of the incoming frame.
    #[default]
    Rectangle,
    /// Circular reveal of the sharp incoming frame.
    Circle,
    /// Circular reveal whose incoming frame sharpens from a blur.
    CircleBlur,
}

impl TransitionStyle {
    /// GPU-facing encoding of the style.
    pub fn as_shader_value(self) -> f32 {
        match self {
            TransitionStyle::Rectangle => 0.0,
            TransitionStyle::Circle => 1.0,
            TransitionStyle::CircleBlur => 2.0,
        }
    }
}

/// The point from which a reveal expands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RevealOrigin {
    /// Reveal expands from the top-left corner.
    TopLeft,
    /// Reveal expands from the top-right corner.
    TopRight,
    /// Reveal expands from the bottom-left corner.
    BottomLeft,
    /// Reveal expands from the bottom-right corner.
    BottomRight,
    /// Reveal expands from the center.
    Center,
    /// Reveal expands upward from the bottom-center edge.
    #[default]
    BottomUp,
}

/// Configuration for one native theme transition.
#[derive(Clone, Copy, Debug)]
pub struct TransitionOptions {
    /// Visual style of the transition.
    pub style: TransitionStyle,
    /// Where the reveal expands from.
    pub origin: RevealOrigin,
    /// Total animation duration.
    pub duration: Duration,
    /// Maximum incoming-frame blur radius in logical pixels. Only used by
    /// [`TransitionStyle::CircleBlur`].
    pub blur_radius: f32,
}

impl Default for TransitionOptions {
    fn default() -> Self {
        Self {
            style: TransitionStyle::Rectangle,
            origin: RevealOrigin::BottomUp,
            duration: Duration::from_millis(400),
            blur_radius: 8.0,
        }
    }
}

/// Validate transition options before arming a transition.
pub fn validate_options(options: TransitionOptions) -> Result<(), &'static str> {
    if options.duration.is_zero() {
        return Err("duration must be non-zero");
    }
    if !options.blur_radius.is_finite() || options.blur_radius < 0.0 {
        return Err("blur radius must be finite and non-negative");
    }
    Ok(())
}

/// Whether renderer-native window effects (theme transitions, modal backdrop
/// blur) are enabled. Controlled by the `ZED_WINDOW_EFFECTS` environment
/// variable: `0`/`off`/`false` disables, anything else (or unset) enables.
pub fn window_effects_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("ZED_WINDOW_EFFECTS").as_deref(),
            Ok("0") | Ok("off") | Ok("false")
        )
    })
}

pub(crate) fn trace(message: &str) {
    static TRACE: OnceLock<bool> = OnceLock::new();
    if *TRACE.get_or_init(|| std::env::var_os("GPUI_WINDOW_EFFECTS_TRACE").is_some()) {
        eprintln!("[window-effects] {message}");
    }
}

/// Compute the reveal origin point and CSS `circle(150%)` radius in physical
/// pixels. CSS resolves a circle percentage against the normalized diagonal.
pub(crate) fn reveal_geometry(origin: RevealOrigin, width: f32, height: f32) -> ([f32; 2], f32) {
    let origin = match origin {
        RevealOrigin::TopLeft => [0.0, 0.0],
        RevealOrigin::TopRight => [width, 0.0],
        RevealOrigin::BottomLeft => [0.0, height],
        RevealOrigin::BottomRight => [width, height],
        RevealOrigin::Center => [width * 0.5, height * 0.5],
        RevealOrigin::BottomUp => [width * 0.5, height],
    };
    let radius = 1.5 * width.hypot(height) / std::f32::consts::SQRT_2;
    (origin, radius)
}

/// Initial CSS `inset()` values for the rectangular reveal, expressed as
/// normalized `[top, right, bottom, left]` fractions.
pub(crate) fn rectangle_insets(origin: RevealOrigin) -> [f32; 4] {
    match origin {
        RevealOrigin::TopLeft => [0.0, 1.0, 1.0, 0.0],
        RevealOrigin::TopRight => [0.0, 0.0, 1.0, 1.0],
        RevealOrigin::BottomLeft => [1.0, 1.0, 0.0, 0.0],
        RevealOrigin::BottomRight => [1.0, 0.0, 0.0, 1.0],
        RevealOrigin::Center => [0.5, 0.5, 0.5, 0.5],
        RevealOrigin::BottomUp => [1.0, 0.0, 0.0, 0.0],
    }
}

/// CSS easing used by the selected reference transition.
pub(crate) fn transition_ease(style: TransitionStyle, progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    if progress == 0.0 || progress == 1.0 {
        return progress;
    }

    let (first_x, second_x) = match style {
        TransitionStyle::Rectangle => (0.0, 0.58),
        TransitionStyle::Circle | TransitionStyle::CircleBlur => (0.4, 0.2),
    };

    let mut low = 0.0_f32;
    let mut high = 1.0_f32;
    for _ in 0..18 {
        let parameter = (low + high) * 0.5;
        if cubic_bezier_axis(parameter, first_x, second_x) < progress {
            low = parameter;
        } else {
            high = parameter;
        }
    }
    cubic_bezier_axis((low + high) * 0.5, 0.0, 1.0)
}

fn cubic_bezier_axis(parameter: f32, first: f32, second: f32) -> f32 {
    let inverse = 1.0 - parameter;
    3.0 * inverse * inverse * parameter * first
        + 3.0 * inverse * parameter * parameter * second
        + parameter * parameter * parameter
}

/// Per-frame theme-transition state on [`crate::Window`].
#[derive(Default)]
pub(crate) enum ThemeTransitionState {
    #[default]
    Idle,
    /// Snapshot captured; waiting for the host to apply the theme and start.
    ReadyToStart { options: TransitionOptions },
    /// Animating the reveal from the old snapshot to the new frame.
    Active {
        options: TransitionOptions,
        start: Instant,
        last_frame: Instant,
        frame_count: u32,
        max_frame_gap: Duration,
    },
}

/// Timing captured from the most recently completed native theme transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeTransitionMetrics {
    /// Number of rendered transition frames, including the completion frame.
    pub frame_count: u32,
    /// Largest interval between two rendered transition frames.
    pub max_frame_gap: Duration,
    /// Wall-clock duration from start through the completion frame.
    pub elapsed: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_options() {
        assert!(
            validate_options(TransitionOptions {
                duration: Duration::ZERO,
                ..TransitionOptions::default()
            })
            .is_err()
        );
        assert!(
            validate_options(TransitionOptions {
                blur_radius: f32::NAN,
                ..TransitionOptions::default()
            })
            .is_err()
        );
        assert!(validate_options(TransitionOptions::default()).is_ok());
    }

    #[test]
    fn reveal_radius_reaches_every_corner() {
        for origin in [
            RevealOrigin::TopLeft,
            RevealOrigin::TopRight,
            RevealOrigin::BottomLeft,
            RevealOrigin::BottomRight,
            RevealOrigin::Center,
            RevealOrigin::BottomUp,
        ] {
            let (point, radius) = reveal_geometry(origin, 1080.0, 720.0);
            for corner in [[0.0, 0.0], [1080.0, 0.0], [0.0, 720.0], [1080.0, 720.0]] {
                let x: f32 = corner[0] - point[0];
                let y: f32 = corner[1] - point[1];
                assert!(x.hypot(y) <= radius + 0.01);
            }
        }
    }

    #[test]
    fn rectangle_insets_match_the_reference_component() {
        assert_eq!(
            rectangle_insets(RevealOrigin::TopLeft),
            [0.0, 1.0, 1.0, 0.0]
        );
        assert_eq!(
            rectangle_insets(RevealOrigin::TopRight),
            [0.0, 0.0, 1.0, 1.0]
        );
        assert_eq!(
            rectangle_insets(RevealOrigin::BottomLeft),
            [1.0, 1.0, 0.0, 0.0]
        );
        assert_eq!(
            rectangle_insets(RevealOrigin::BottomRight),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(rectangle_insets(RevealOrigin::Center), [0.5; 4]);
        assert_eq!(
            rectangle_insets(RevealOrigin::BottomUp),
            [1.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn easing_has_css_endpoints_and_monotonic_output() {
        for style in [
            TransitionStyle::Rectangle,
            TransitionStyle::Circle,
            TransitionStyle::CircleBlur,
        ] {
            assert_eq!(transition_ease(style, 0.0), 0.0);
            assert_eq!(transition_ease(style, 1.0), 1.0);
            let mut previous = 0.0;
            for step in 1..=100 {
                let value = transition_ease(style, step as f32 / 100.0);
                assert!(value >= previous);
                previous = value;
            }
        }
    }
}
