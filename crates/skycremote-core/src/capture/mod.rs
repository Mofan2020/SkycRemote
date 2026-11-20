//! Reading the pixels of this machine's screens.
//!
//! Every platform hands out screens through a different API, and the two
//! crates that paper over that (`xcap` and `display-info`) disagree about
//! what their numbers mean: on macOS `xcap` reports a screen in *points*
//! while the captured image comes back in *pixels*, so a 2x Retina desktop
//! answers `1440x900` and then hands back a `2880x1800` image.  A remote
//! session that trusts the wrong one of those numbers puts the cursor in
//! the wrong place, so this module normalises everything into a single
//! meaning before it leaves the module:
//!
//! * [`Screen::width`] / [`Screen::height`] are **pixels** — the size of
//!   the image [`capture_frame`] will hand back for that screen.
//! * [`Screen::scale_factor`] is the number of pixels per logical unit, so
//!   a pointer position in logical space can be turned into a pixel
//!   position by multiplying.
//!
//! The rule "`Screen::width` equals the width of the next [`Frame`]" is the
//! invariant the rest of the pipeline depends on; it is what keeps the
//! encoder from resizing mid-stream and what keeps input events landing on
//! the pixel the user aimed at.

pub mod linux;
pub mod macos;
pub mod windows;

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use image::RgbaImage;
use thiserror::Error;

/// One screen of the machine being shared.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// The position of this screen in the list the capture end reports.
    /// This is the value the other end sends back to select a screen.
    pub index: usize,
    /// The name the operating system gives the screen, for a UI to show.
    pub name: String,
    /// Width in pixels — the width of the image [`capture_frame`] returns.
    pub width: u32,
    /// Height in pixels — the height of the image [`capture_frame`] returns.
    pub height: u32,
    /// Pixels per logical unit: 2.0 on a Retina display, 1.0 on a plain one.
    pub scale_factor: f64,
    /// Whether this screen holds the menu bar / taskbar / main desktop.
    pub primary: bool,
    /// Where the top left corner of this screen sits on the desktop.
    ///
    /// In the units the platform's own input system uses: points on
    /// macOS, pixels elsewhere.  A pointer position that comes from the
    /// other end is in the pixels of the picture that was sent, so it
    /// has to be moved into the desktop's frame before it can be used.
    pub origin_x: f64,
    pub origin_y: f64,
}

/// One captured picture of a screen.
///
/// The pixels are kept in an [`RgbaImage`] rather than a bare `Vec<u8>`
/// because every consumer in this crate already speaks `image`'s types:
/// handing them pre-typed pixels saves a copy and a chance to get the row
/// stride wrong.
#[derive(Debug)]
pub struct Frame {
    /// RGBA8 pixels, row-major, four bytes per pixel.
    pub image: RgbaImage,
    /// When the capture actually happened, used to stamp the encoded frame.
    pub captured_at: Instant,
}

impl Frame {
    /// The width of the picture in pixels.
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    /// The height of the picture in pixels.
    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// The raw RGBA8 bytes, row-major.
    pub fn rgba(&self) -> &[u8] {
        self.image.as_raw()
    }
}

/// Everything that can go wrong while looking at a screen.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// The machine reports no screens at all.  A headless server, or a
    /// session that is not allowed to see the display, answers this way.
    #[error("no screen is attached to this machine")]
    NoScreens,

    /// The caller asked for a screen that is not in the list.  It carries
    /// both numbers because the useful part of the message is *how many*
    /// screens exist, which is what tells a peer to re-list and retry.
    #[error("screen index {index} does not exist; {available} screen(s) are attached")]
    UnknownScreen {
        /// The index the caller asked for.
        index: usize,
        /// How many screens the machine actually has.
        available: usize,
    },

    /// The platform cannot capture at all here — Wayland without a working
    /// desktop portal is the case this exists for.  It is a separate
    /// variant from [`CaptureError::Backend`] because it is not worth
    /// retrying: the user has to install or enable something first.
    #[error("screen capture is unavailable on {platform}: {reason}")]
    Unsupported {
        /// Which platform refused, for the log line.
        platform: &'static str,
        /// What is missing, in words the user can act on.
        reason: String,
    },

    /// The capture backend failed at the moment of use.  This one *is*
    /// worth retrying: a screen that was just unplugged, a portal that is
    /// restarting, and a locked session all show up here.
    #[error("the capture backend failed: {0}")]
    Backend(String),
}

/// Lists the screens this machine has, in the order the capture end will
/// use them.
///
/// The list comes from `xcap` because it is the crate that can actually
/// capture; `display-info` is consulted only to fill in what `xcap` could
/// not answer.  Listing from one source and capturing from another would
/// let the two disagree about what "screen 1" is, so one of them has to be
/// the authority and it has to be the one holding the pixels.
pub fn list_screens() -> Result<Vec<Screen>, CaptureError> {
    let monitors = xcap::Monitor::all().map_err(|err| explain_backend_failure(&err))?;

    if monitors.is_empty() {
        return Err(no_screens_error());
    }

    // display-info is best effort: if it fails we still have a usable list
    // from xcap, so its absence must never fail the call.
    let infos = display_info::DisplayInfo::all().unwrap_or_default();

    let mut screens = Vec::with_capacity(monitors.len());
    for (index, monitor) in monitors.iter().enumerate() {
        screens.push(screen_from_monitor(index, monitor, infos.get(index))?);
    }

    Ok(screens)
}

/// Captures the screen at `index` as it is right now.
///
/// This is deliberately a one-shot call: a capture loop calls it in a
/// tight loop and drops the frames it does not want, rather than the
/// capture code trying to guess the wanted frame rate.  [`FramePacer`]
/// is the piece that decides which captured frame is worth keeping.
pub fn capture_frame(index: usize) -> Result<Frame, CaptureError> {
    let screens = xcap::Monitor::all().map_err(|err| explain_backend_failure(&err))?;

    if screens.is_empty() {
        return Err(no_screens_error());
    }

    let monitor = screens.get(index).ok_or(CaptureError::UnknownScreen {
        index,
        available: screens.len(),
    })?;

    let image = monitor
        .capture_image()
        .map_err(|err| explain_backend_failure(&err))?;

    // `capture_image` can answer with an empty picture when the session is
    // locked or when the process lacks the screen-recording permission.
    // Treating that as a real frame would hand the encoder a 0x0 image it
    // cannot even allocate, so it is reported as a failure instead.
    if image.width() == 0 || image.height() == 0 {
        return Err(CaptureError::Backend(
            "the backend returned an empty picture, which usually means the session is \
             locked or this process is not allowed to record the screen"
                .to_string(),
        ));
    }

    Ok(Frame {
        image,
        captured_at: Instant::now(),
    })
}

/// Drops captured frames that are ahead of the target frame rate.
///
/// A capture loop runs as fast as the backend allows — often faster than
/// the encoder can spend bits on.  This helper decides, from the clock
/// alone, whether a frame that was just captured is due.  It never sleeps:
/// blocking the capture thread would make the capture the slowest part of
/// the pipeline and would also delay the frames that *are* due.
///
/// The pacer drops frames rather than delaying them, so the timestamp of a
/// kept frame is the time it was actually captured.  That is what keeps
/// the remote picture in step with the remote clock.
#[derive(Debug)]
pub struct FramePacer {
    /// The interval between kept frames, or `None` when every frame is due.
    interval: Option<Duration>,
    /// When the next frame becomes due.
    next_deadline: Instant,
    /// How many frames this pacer has told the caller to throw away.
    dropped: u64,
}

impl FramePacer {
    /// Builds a pacer that keeps `fps` frames a second.
    ///
    /// A rate of zero means "keep everything": a caller that has not asked
    /// for a rate should not have frames silently thrown away.  Rates above
    /// the screen's refresh rate are harmless — they simply keep every
    /// frame the backend produces.
    pub fn new(fps: u32) -> Self {
        let interval = if fps == 0 {
            None
        } else {
            Some(Duration::from_secs_f64(1.0 / f64::from(fps)))
        };

        Self {
            interval,
            next_deadline: Instant::now(),
            dropped: 0,
        }
    }

    /// Answers whether the frame seen at `now` should be kept.
    ///
    /// The caller passes the time in rather than the pacer reading the
    /// clock itself so that the decision is reproducible in tests, and so
    /// that a caller which already knows when the frame was taken does not
    /// pay for a second clock read.
    pub fn should_capture(&mut self, now: Instant) -> bool {
        let Some(interval) = self.interval else {
            return true;
        };

        if now >= self.next_deadline {
            // A long stall — the machine slept, or the encoder blocked —
            // would otherwise leave the deadline in the past forever and
            // the pacer would keep every frame until it caught up.  Resync
            // to the present instead, so the rate is right from here on.
            let behind = now.duration_since(self.next_deadline);
            self.next_deadline = if behind >= interval {
                now + interval
            } else {
                self.next_deadline + interval
            };
            true
        } else {
            self.dropped += 1;
            false
        }
    }

    /// When the next frame will be due.  A caller with nothing else to do
    /// can wait until then instead of spinning, but it must not shorten
    /// the wait or frames will be dropped that were due.
    pub fn next_deadline(&self) -> Instant {
        self.next_deadline
    }

    /// How many frames have been dropped so far, for the session statistics.
    pub fn dropped_frames(&self) -> u64 {
        self.dropped
    }
}

/// Turns the logical size a backend reported into the pixel size the
/// captured image will have.
///
/// The scale factor comes from the operating system and is trusted only
/// while it is a sane positive number: drivers do report zero, and a
/// single bad value would otherwise turn every screen into a zero-pixel
/// one and take the whole session down with it.
fn physical_size(width: u32, height: u32, scale_factor: f64) -> (u32, u32) {
    let scale = sane_scale(scale_factor);

    let scale_dim = |value: u32| -> u32 {
        let scaled = (f64::from(value) * scale).round();
        // A screen that claims to be smaller than one pixel is not credible;
        // clamping keeps the encoder's allocation arithmetic away from zero.
        scaled.max(1.0) as u32
    };

    (scale_dim(width), scale_dim(height))
}

/// Clamps a reported scale factor to something a pixel count can be
/// multiplied by without turning into zero, infinity or NaN.
fn sane_scale(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

/// What a backend reported about one screen.
///
/// The fields are gathered here rather than passed on one by one, because
/// they always travel together: the size, the scale and the place on the
/// desktop are one screen's description, not eight unrelated numbers.
#[derive(Debug, Clone)]
pub struct ScreenSpec {
    /// The position of this screen in the list the capture end reports.
    pub index: usize,
    /// The name the operating system gives the screen.
    pub name: String,
    /// The width the backend reported, in its own units.
    pub width: u32,
    /// The height the backend reported, in its own units.
    pub height: u32,
    /// Pixels per logical unit.
    pub scale_factor: f64,
    /// Whether this screen holds the menu bar / taskbar.
    pub primary: bool,
    /// Where the top left corner sits on the desktop.
    pub origin_x: f64,
    /// Where the top left corner sits on the desktop.
    pub origin_y: f64,
    /// Whether `width` and `height` are already in pixels.
    ///
    /// macOS and Linux report points — the size a window would have — so
    /// the scale factor still has to be applied to get the number of
    /// pixels the capture will really contain.  Windows reports
    /// `dmPelsWidth`, which is physical pixels already; multiplying it
    /// again would claim a screen twice as wide as the panel.
    ///
    /// This is a property of the backend, not of the machine the code
    /// happens to run on, so it is carried in the data rather than
    /// decided by a `cfg` block.  That way the Windows numbers can be
    /// checked on any machine.
    pub size_is_pixels: bool,
}

/// Builds one [`Screen`] from what a backend reported.
pub fn make_screen(spec: ScreenSpec) -> Screen {
    let (width, height) = if spec.size_is_pixels {
        (spec.width, spec.height)
    } else {
        physical_size(spec.width, spec.height, spec.scale_factor)
    };

    Screen {
        index: spec.index,
        name: spec.name,
        width,
        height,
        scale_factor: spec.scale_factor,
        primary: spec.primary,
        origin_x: spec.origin_x,
        origin_y: spec.origin_y,
    }
}

/// Builds one [`Screen`] from what `xcap` reported, falling back to
/// `display-info` for any field the backend refused to answer.
fn screen_from_monitor(
    index: usize,
    monitor: &xcap::Monitor,
    info: Option<&display_info::DisplayInfo>,
) -> Result<Screen, CaptureError> {
    let width = monitor
        .width()
        .ok()
        .filter(|value| *value > 0)
        .or_else(|| info.map(|info| info.width))
        .unwrap_or(0);

    let height = monitor
        .height()
        .ok()
        .filter(|value| *value > 0)
        .or_else(|| info.map(|info| info.height))
        .unwrap_or(0);

    if width == 0 || height == 0 {
        return Err(CaptureError::Backend(format!(
            "screen {index} reported a {width}x{height} size, which is not usable"
        )));
    }

    let scale_factor = monitor
        .scale_factor()
        .ok()
        .map(f64::from)
        .filter(|value| value.is_finite() && *value > 0.0)
        .or_else(|| {
            info.map(|info| f64::from(info.scale_factor))
                .filter(|value| value.is_finite() && *value > 0.0)
        })
        .unwrap_or(1.0);

    let name = monitor
        .friendly_name()
        .or_else(|_| monitor.name())
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| info.map(|info| info.name.clone()))
        .unwrap_or_else(|| format!("Screen {index}"));

    let primary = monitor
        .is_primary()
        .unwrap_or_else(|_| info.is_some_and(|info| info.is_primary));

    // The desktop's own idea of where this screen sits.  Without it a
    // pointer would land at the top left of the desktop instead of on
    // the screen that was being looked at.
    let (origin_x, origin_y) = info
        .map(|info| (f64::from(info.x), f64::from(info.y)))
        .unwrap_or((0.0, 0.0));

    Ok(make_screen(ScreenSpec {
        index,
        name,
        width,
        height,
        scale_factor,
        primary,
        origin_x,
        origin_y,
        size_is_pixels: platform::SIZE_IS_PIXELS,
    }))
}

#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "windows")]
use windows as platform;

/// Turns a backend error into the one this module promises to return.
///
/// The platforms disagree about how loudly they refuse — Wayland says
/// "portal not found" in a D-Bus error, X11 says "cannot open display" —
/// so each platform module gets a chance to recognise its own failure and
/// turn it into a message that says what the user must install.
fn explain_backend_failure(err: &xcap::XCapError) -> CaptureError {
    let message = err.to_string();

    #[cfg(target_os = "linux")]
    if let Some(unsupported) = linux::explain_error(&message) {
        return unsupported;
    }

    CaptureError::Backend(message)
}

/// The error for "the machine has no screens", which differs by platform:
/// on Wayland the usual cause is a missing portal, not a missing monitor.
fn no_screens_error() -> CaptureError {
    #[cfg(target_os = "linux")]
    if linux::detect_session_from_env() == linux::SessionKind::Wayland {
        return CaptureError::Unsupported {
            platform: "Wayland",
            reason: linux::PORTAL_MISSING_REASON.to_string(),
        };
    }

    CaptureError::NoScreens
}

/// The invariant every other module relies on, kept in one place: a screen's
/// reported size is the size of the frames that come out of it.
#[cfg(test)]
fn screen_matches_frame(screen: &Screen, frame: &Frame) -> bool {
    screen.width == frame.width() && screen.height == frame.height()
}
