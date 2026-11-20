//! What Linux means by a "screen size", and how it fails.
//!
//! Linux is two platforms wearing one name.  Under X11 the display server
//! owns the pixels and any client can read them.  Under Wayland the
//! compositor owns the pixels and a client gets them only through the
//! XDG desktop portal, which asks the user for permission per session.
//! The same `xcap` call therefore succeeds instantly on one and hangs on
//! a permission dialog on the other, and a remote-desktop host that just
//! retries "capture failed" forever is useless.  This module tells the two
//! apart so the failure can be explained instead.
//!
//! On XRandR a monitor is reported in *physical* pixels by the server but
//! `xcap` divides by the desktop scale factor before answering, so what
//! arrives here is the logical size.  The captured image is at the
//! physical resolution, so the two are reconciled the same way as on
//! macOS — by multiplying back.

/// Whether the size the backend reports is already in pixels.
///
/// XRandR gives the pixel size of the framebuffer, but `xcap` divides it
/// by the desktop scale factor, so what arrives here is the logical size
/// and the scale factor still has to be applied to get the pixel count.
pub const SIZE_IS_PIXELS: bool = false;

use super::CaptureError;

/// Which display server the session is running on.
///
/// This matters because the two fail in completely different ways and
/// because "no screens were found" means "install a portal" on one and
/// "the GPU is unplugged" on the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// A classic X11 session, where capture works without a prompt.
    X11,
    /// A Wayland session, where capture needs the desktop portal.
    Wayland,
}

/// What to tell the user when a Wayland session has no working portal.
///
/// Wayland deliberately does not let a client read the framebuffer, so
/// this is not a bug to work around: the portal is the supported path and
/// it has to be installed and running before capture can work at all.
pub const PORTAL_MISSING_REASON: &str = "the XDG desktop portal \
    (xdg-desktop-portal with a ScreenCast backend, usually \
    xdg-desktop-portal-gnome or xdg-desktop-portal-kde) is not available. \
    Wayland does not allow a client to read the screen without it; install the \
    portal package for your desktop and log in again";

/// Decides the session kind from the environment variables that describe it.
///
/// The values are passed in rather than read here so that the decision can
/// be tested for every combination without mutating the test process's
/// environment — which is global state shared between concurrently running
/// tests.
///
/// `WAYLAND_DISPLAY` is checked first because a Wayland session usually
/// also sets `DISPLAY` for XWayland, and the presence of an X socket does
/// not mean the compositor will let an X client read the real screen.
pub fn detect_session(
    wayland_display: Option<&str>,
    xdg_session_type: Option<&str>,
    x11_display: Option<&str>,
) -> SessionKind {
    if wayland_display.is_some_and(|value| !value.is_empty()) {
        return SessionKind::Wayland;
    }

    if xdg_session_type.is_some_and(|value| value.eq_ignore_ascii_case("wayland")) {
        return SessionKind::Wayland;
    }

    // An X11 display with no Wayland marker is X11; with neither, assume
    // X11 as well, because that is the one whose failure mode ("cannot
    // open display") is the more honest message.
    let _ = x11_display;
    SessionKind::X11
}

/// [`detect_session`] applied to this process's actual environment.
pub fn detect_session_from_env() -> SessionKind {
    let wayland = std::env::var("WAYLAND_DISPLAY").ok();
    let session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let display = std::env::var("DISPLAY").ok();

    detect_session(
        wayland.as_deref(),
        session_type.as_deref(),
        display.as_deref(),
    )
}

/// What to tell the user when there is no display server to talk to.
///
/// A headless machine — a server, a container, an SSH session — has no X11
/// display and no Wayland compositor.  The backend says something about a
/// closed connection or a display string it could not parse, and without
/// this translation the user reads an internal message and has no idea
/// what to do about it.
pub const NO_DISPLAY_REASON: &str = "there is no display server to capture from. \
    This machine has no X11 or Wayland session — it may be a server, a \
    container, or a session reached over SSH. Screen capture needs a \
    desktop session; log in on the machine's own display to use it";

/// Recognises the portal failures inside a backend error message.
///
/// The backend reports a D-Bus or portal error as an opaque string; the
/// distinction that matters to the user is "you need to install or start
/// the portal", and this is where that translation happens.  It returns
/// `None` for errors that are not portal-related so the caller can pass
/// them through unchanged.
pub fn explain_error(message: &str) -> Option<CaptureError> {
    let lowered = message.to_ascii_lowercase();

    let looks_like_portal = lowered.contains("portal")
        || lowered.contains("dbus")
        || lowered.contains("screencast")
        || lowered.contains("pipewire");

    if looks_like_portal {
        return Some(CaptureError::Unsupported {
            platform: "Wayland",
            reason: PORTAL_MISSING_REASON.to_string(),
        });
    }

    // The X11 client library reports a display it cannot reach in a
    // handful of ways, and the wording depends on which one failed and
    // on which feature was compiled in.  All of them mean the same thing
    // to the user: there is nothing here to capture.
    let looks_like_no_display = lowered.contains("parsing display string")
        || lowered.contains("screen matching the display")
        || lowered.contains("xopendisplay failed")
        || lowered.contains("cannot open display")
        || lowered.contains("connection error, possible i/o error");

    if looks_like_no_display {
        return Some(CaptureError::Unsupported {
            platform: "X11",
            reason: NO_DISPLAY_REASON.to_string(),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{make_screen, ScreenSpec};

    #[test]
    fn a_wayland_socket_wins_over_xwayland() {
        // A Wayland desktop almost always exports DISPLAY too, for XWayland
        // clients.  Trusting it would pick the wrong backend and produce a
        // screenshot of the XWayland root window instead of the screen.
        assert_eq!(
            detect_session(Some("wayland-0"), Some("wayland"), Some(":0")),
            SessionKind::Wayland
        );
    }

    #[test]
    fn an_x11_session_is_recognised() {
        assert_eq!(
            detect_session(None, Some("x11"), Some(":0")),
            SessionKind::X11
        );
        // No variables at all: nothing says Wayland, so X11 it is.
        assert_eq!(detect_session(None, None, None), SessionKind::X11);
    }

    #[test]
    fn an_empty_wayland_variable_does_not_count() {
        // Some display managers export WAYLAND_DISPLAY="" under X11.
        assert_eq!(
            detect_session(Some(""), Some("x11"), Some(":0")),
            SessionKind::X11
        );
    }

    #[test]
    fn a_portal_failure_is_explained() {
        let described =
            explain_error("org.freedesktop.portal.ScreenCast was not provided by any .service")
                .expect("a portal error must be recognised");
        assert!(matches!(
            described,
            CaptureError::Unsupported {
                platform: "Wayland",
                ..
            }
        ));
    }

    #[test]
    fn a_machine_without_a_display_server_is_explained() {
        // A headless machine has no X11 display and no Wayland
        // compositor.  The X11 client library says so in several ways,
        // and every one of them has to reach the user as "there is
        // nothing here to capture" rather than as an internal message
        // about a closed connection.
        for message in [
            "Connection closed, error during parsing display string",
            "Connection closed, the server does not have a screen matching the display",
            "XOpenDisplay failed to open a display. Check the $DISPLAY env var",
            "cannot open display :0",
            "Connection error, possible I/O error",
        ] {
            let described = explain_error(message)
                .unwrap_or_else(|| panic!("this failure must be recognised: {message}"));
            assert!(
                matches!(
                    described,
                    CaptureError::Unsupported {
                        platform: "X11",
                        ..
                    }
                ),
                "wrong answer for {message}"
            );
        }
    }

    #[test]
    fn an_unrelated_failure_is_passed_through() {
        // Something that is not about the display at all must not be
        // dressed up as a missing display: the two need different fixes.
        assert!(explain_error("the encoder ran out of memory").is_none());
        assert!(explain_error("permission denied").is_none());
    }

    #[test]
    fn a_scaled_screen_reports_the_pixels_the_capture_will_have() {
        // A HiDPI X11 desktop at 2x: xcap answers 1280x800 logical.
        let screen = make_screen(ScreenSpec {
            index: 0,
            name: "eDP-1".into(),
            width: 1280,
            height: 800,
            scale_factor: 2.0,
            primary: true,
            origin_x: 0.0,
            origin_y: 0.0,
            size_is_pixels: false,
        });
        assert_eq!(screen.width, 2560);
        assert_eq!(screen.height, 1600);
    }

    #[test]
    fn an_unscaled_screen_is_left_alone() {
        let screen = make_screen(ScreenSpec {
            index: 0,
            name: "HDMI-1".into(),
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
    }
}
