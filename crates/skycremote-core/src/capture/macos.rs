//! What macOS means by a "screen size".
//!
//! CoreGraphics describes a display in *points*: a 13" MacBook reports
//! `1440x900` no matter whether it is drawing 1 or 2 pixels per point.
//! The captured image, however, always comes back at the panel's real
//! resolution, so the same screen answers `2880x1800` from
//! `CGWindowListCreateImage`.  Everything that positions a cursor or sizes
//! a video has to agree on one of those two numbers, and this crate picks
//! pixels, so the conversion happens here and nowhere else.
//!
//! The scale factor is read from the display mode rather than assumed to
//! be 2: external 4K panels run at 1.5x and 1.25x, and a Mac driving two
//! screens of different densities has to report each of them correctly.

/// Whether the size the backend reports is already in pixels.
///
/// CoreGraphics reports the size in points, so the scale factor still has
/// to be applied to get the pixel count the capture will really have.
pub const SIZE_IS_PIXELS: bool = false;

/// The pixels-per-point ratio of a display, given its two descriptions.
///
/// CoreGraphics already hands out a scale factor, but it is derived by
/// dividing the mode's pixel width by the display's point width and any
/// of those two numbers can be missing — a display that is being switched
/// between resolutions reports a zero for a frame or two.  Recomputing it
/// from the two sizes lets the caller notice that and fall back to 1.0
/// instead of dividing by zero.
pub fn retina_scale_factor(logical_width: u32, pixel_width: u32) -> f64 {
    if logical_width == 0 || pixel_width == 0 {
        return 1.0;
    }

    let ratio = f64::from(pixel_width) / f64::from(logical_width);

    if ratio.is_finite() && ratio > 0.0 {
        ratio
    } else {
        1.0
    }
}

/// Whether this screen is a Retina-class panel.
///
/// Kept as a named helper because "scale factor greater than one" is the
/// question the encoder asks when it decides whether the frame it was
/// handed is bigger than the logical desktop the user thinks they have.
pub fn is_retina(scale_factor: f64) -> bool {
    scale_factor > 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{make_screen, ScreenSpec};

    #[test]
    fn a_retina_display_reports_its_physical_pixels() {
        // A 13" MacBook: 1440x900 points, 2 pixels per point.
        let screen = make_screen(ScreenSpec {
            index: 0,
            name: "Built-in".into(),
            width: 1440,
            height: 900,
            scale_factor: 2.0,
            primary: true,
            origin_x: 0.0,
            origin_y: 0.0,
            size_is_pixels: false,
        });
        assert_eq!(screen.width, 2880);
        assert_eq!(screen.height, 1800);
        assert!(is_retina(screen.scale_factor));
    }

    #[test]
    fn a_plain_display_is_left_alone() {
        let screen = make_screen(ScreenSpec {
            index: 1,
            name: "Dell".into(),
            width: 1920,
            height: 1080,
            scale_factor: 1.0,
            primary: false,
            origin_x: 0.0,
            origin_y: 0.0,
            size_is_pixels: false,
        });
        assert_eq!(screen.width, 1920);
        assert_eq!(screen.height, 1080);
        assert!(!is_retina(screen.scale_factor));
    }

    #[test]
    fn fractional_scaling_rounds_to_the_nearest_pixel() {
        // A 4K panel at 1.5x is 2560x1440 points and 3840x2160 pixels.
        let screen = make_screen(ScreenSpec {
            index: 0,
            name: "4K".into(),
            width: 2560,
            height: 1440,
            scale_factor: 1.5,
            primary: true,
            origin_x: 0.0,
            origin_y: 0.0,
            size_is_pixels: false,
        });
        assert_eq!(screen.width, 3840);
        assert_eq!(screen.height, 2160);
    }

    #[test]
    fn a_broken_scale_factor_falls_back_to_one() {
        // A display mid-resolution-switch reports zero; multiplying by it
        // would produce a zero-pixel screen and break every later step.
        assert_eq!(retina_scale_factor(1440, 0), 1.0);
        assert_eq!(retina_scale_factor(0, 2880), 1.0);

        let screen = make_screen(ScreenSpec {
            index: 0,
            name: "Switching".into(),
            width: 800,
            height: 600,
            scale_factor: 0.0,
            primary: false,
            origin_x: 0.0,
            origin_y: 0.0,
            size_is_pixels: false,
        });
        assert_eq!(screen.width, 800);
        assert_eq!(screen.height, 600);
    }

    #[test]
    fn the_scale_factor_is_recomputed_from_the_two_widths() {
        assert_eq!(retina_scale_factor(1440, 2880), 2.0);
        assert_eq!(retina_scale_factor(2560, 3840), 1.5);
        assert_eq!(retina_scale_factor(1920, 1920), 1.0);
    }
}
