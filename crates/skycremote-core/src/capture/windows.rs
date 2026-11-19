//! What Windows means by a "screen size".
//!
//! Windows is the odd one out: a monitor's `dmPelsWidth` is already the
//! number of physical pixels the panel has, so unlike macOS and Linux
//! there is nothing to convert.  The DPI scale factor is still reported
//! and still matters — it is what maps the coordinates a user's mouse
//! event carries onto those pixels — but multiplying the size by it would
//! double a 4K screen's claimed resolution and make the encoder allocate
//! four times the memory it needs.
//!
//! The other Windows-specific fact is that capture and DPI awareness are
//! linked: a process that is not per-monitor DPI aware gets a virtualised
//! desktop from the OS, so windows are rendered at 96 DPI and then scaled,
//! and a capture of that desktop is blurry.  The host binary has to
//! declare per-monitor awareness in its manifest for the pixels here to
//! mean what the panel really shows.

use super::Screen;

/// Builds a [`Screen`] from what a Windows backend reported.
///
/// `width` and `height` are `dmPelsWidth`/`dmPelsHeight`, which are
/// physical pixels; they are passed through untouched.  `scale_factor` is
/// kept because input coordinates arrive in logical units and must be
/// multiplied by it to land on the right pixel.
pub fn make_screen(
    index: usize,
    name: String,
    width: u32,
    height: u32,
    scale_factor: f64,
    primary: bool,
) -> Screen {
    Screen {
        index,
        name,
        width,
        height,
        scale_factor,
        primary,
    }
}

/// Whether a reported size looks like it came out of a virtualised
/// (non-DPI-aware) desktop.
///
/// The GDI path reports the size of the virtual desktop rather than the
/// panel when the process is not per-monitor DPI aware.  There is no API
/// that says so directly, but a size that is exactly the physical size
/// divided by the scale factor is the signature of it, and a session that
/// notices this can warn instead of silently shipping a blurry screen.
pub fn looks_virtualised(reported_width: u32, physical_width: u32, scale_factor: f64) -> bool {
    if scale_factor <= 1.0 || physical_width == 0 {
        return false;
    }

    let expected = (f64::from(physical_width) / scale_factor).round() as u32;

    reported_width == expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_size_is_already_in_pixels_and_must_not_be_scaled() {
        // A 4K panel at 200%: dmPelsWidth is 3840, and that is the size of
        // the image the GDI capture returns.  Multiplying by 2.0 here —
        // which is what the macOS and Linux paths must do — would claim a
        // 7680-pixel-wide screen that does not exist.
        let screen = make_screen(0, "\\\\.\\DISPLAY1".into(), 3840, 2160, 2.0, true);
        assert_eq!(screen.width, 3840);
        assert_eq!(screen.height, 2160);
        assert_eq!(screen.scale_factor, 2.0);
    }

    #[test]
    fn a_one_x_screen_passes_through() {
        let screen = make_screen(1, "\\\\.\\DISPLAY2".into(), 1920, 1080, 1.0, false);
        assert_eq!(screen.width, 1920);
        assert_eq!(screen.height, 1080);
    }

    #[test]
    fn a_virtualised_desktop_is_detected() {
        // A 4K panel at 200% seen by a DPI-unaware process reports
        // 1920 pixels, which is 3840 / 2.0.
        assert!(looks_virtualised(1920, 3840, 2.0));
        // An honest 1920-wide panel at 1x is not virtualised.
        assert!(!looks_virtualised(1920, 1920, 1.0));
    }

    #[test]
    fn a_per_monitor_aware_process_is_not_flagged() {
        // The same 4K panel, this time reported honestly.
        assert!(!looks_virtualised(3840, 3840, 2.0));
    }

    #[test]
    fn the_primary_flag_survives_the_conversion() {
        let screen = make_screen(0, "primary".into(), 2560, 1440, 1.5, true);
        assert!(screen.primary);
        assert_eq!(screen.width, 2560);
    }
}
