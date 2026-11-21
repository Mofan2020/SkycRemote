//! Tests for the capture module.
//!
//! Most of these deliberately avoid requiring a real screen: a headless CI
//! machine has none, and a test that only passes on a developer's desktop
//! is worse than no test.  What is checked here is the arithmetic that the
//! rest of the pipeline depends on, and the promise that a bad request
//! produces an error instead of a panic.
//!
//! The few tests that do ask for the real screen are **not** part of the
//! normal run.  On macOS, asking for the screen makes the system ask the
//! person first: the run puts a dialog on the screen and waits for an
//! answer.  A test run that interrupts the person it runs on is a test
//! run that gets skipped, so those tests only run when they are asked
//! for:
//!
//! ```
//! SKYCREMOTE_CAPTURE_TESTS=1 cargo test -p skycremote-core
//! ```
//!
//! Nothing is recorded and no picture is kept: those tests look at the
//! size of a frame and throw it away.

use std::time::{Duration, Instant};

use super::*;

/// Whether the tests that ask for the real screen may run.
///
/// The name of the switch says what it does.  It is off unless somebody
/// sets it, because the default run has to leave the person in front of
/// the machine alone.
fn the_real_screen_may_be_asked_for() -> bool {
    std::env::var_os("SKYCREMOTE_CAPTURE_TESTS").is_some()
}

/// Say that a test was left out, so that it is not a silent hole.
fn skipped_because_the_screen_is_asked_for() {
    eprintln!(
        "skipped: this test asks the system for the screen (set SKYCREMOTE_CAPTURE_TESTS=1 to run it)"
    );
}

#[test]
fn the_pacer_keeps_frames_at_the_target_rate() {
    // At 10 fps a frame is due every 100 ms.  Walking the clock forward in
    // 100 ms steps must keep every one of them, and a step that lands
    // exactly on the deadline counts as due.
    let mut pacer = FramePacer::new(10);
    let start = Instant::now();

    for step in 0..10u32 {
        let now = start + Duration::from_millis(u64::from(step) * 100);
        assert!(pacer.should_capture(now), "frame {step} was dropped");
    }

    assert_eq!(pacer.dropped_frames(), 0);
}

#[test]
fn the_pacer_drops_frames_that_are_ahead_of_the_clock() {
    // A capture loop that spins as fast as the backend allows would hand
    // the encoder hundreds of frames a second.  Only the ones that are due
    // may be kept; the rest must be dropped without blocking.
    let mut pacer = FramePacer::new(30);
    let start = Instant::now();

    let mut kept = 0;
    // 300 samples one millisecond apart, i.e. 300 ms of wall clock.
    for step in 0..300u32 {
        if pacer.should_capture(start + Duration::from_millis(u64::from(step))) {
            kept += 1;
        }
    }

    // 30 fps over 300 ms is nine or ten frames, not three hundred.
    assert!(
        (9..=11).contains(&kept),
        "expected about 10 frames in 300 ms at 30 fps, kept {kept}"
    );
    assert_eq!(kept as u64 + pacer.dropped_frames(), 300);
}

#[test]
fn the_pacer_resyncs_after_a_long_stall() {
    // If the machine sleeps, the deadline is left far in the past.  A
    // pacer that simply added one interval would then keep every frame
    // until it caught up, which is the opposite of rate limiting.
    let mut pacer = FramePacer::new(10);
    let start = Instant::now();

    assert!(pacer.should_capture(start));

    // Ten seconds later — a lid close, a debugger pause, anything.
    let after_stall = start + Duration::from_secs(10);
    assert!(pacer.should_capture(after_stall));

    // The next one is due a full interval after that, not immediately.
    assert!(!pacer.should_capture(after_stall + Duration::from_millis(1)));
    assert!(pacer.should_capture(after_stall + Duration::from_millis(100)));
}

#[test]
fn a_rate_of_zero_keeps_every_frame() {
    // Zero means "no target rate was asked for", which must not be read as
    // "keep none" — a caller that forgot to set a rate would get a black
    // screen with no error to explain it.
    let mut pacer = FramePacer::new(0);
    let start = Instant::now();

    for step in 0..50u32 {
        assert!(pacer.should_capture(start + Duration::from_millis(u64::from(step))));
    }
    assert_eq!(pacer.dropped_frames(), 0);
}

#[test]
fn asking_for_a_screen_that_does_not_exist_is_an_error_not_a_panic() {
    // A peer can send any index it likes.  Whatever the machine has, an
    // out-of-range request has to come back as an error.
    if !the_real_screen_may_be_asked_for() {
        skipped_because_the_screen_is_asked_for();
        return;
    }
    let result = capture_frame(usize::MAX);

    match result {
        Err(CaptureError::UnknownScreen { index, .. }) => assert_eq!(index, usize::MAX),
        // A headless machine may legitimately answer "no screens at all"
        // before it ever gets to look at the index.
        Err(CaptureError::NoScreens) => {}
        Err(CaptureError::Unsupported { .. }) => {}
        Err(other) => panic!("unexpected error for an out-of-range index: {other}"),
        Ok(_) => panic!("capturing screen usize::MAX must not succeed"),
    }
}

#[test]
fn listing_screens_never_panics() {
    // The point of this test is not that screens exist, but that both
    // answers are returned as a `Result`.  A headless machine, a locked
    // session and a Wayland box without a portal are all normal.
    if !the_real_screen_may_be_asked_for() {
        skipped_because_the_screen_is_asked_for();
        return;
    }
    match list_screens() {
        Ok(screens) => {
            for (position, screen) in screens.iter().enumerate() {
                assert_eq!(
                    screen.index, position,
                    "screens must be numbered in the order they are listed"
                );
                assert!(screen.width > 0, "screen {position} has no width");
                assert!(screen.height > 0, "screen {position} has no height");
                assert!(
                    screen.scale_factor.is_finite() && screen.scale_factor > 0.0,
                    "screen {position} has an unusable scale factor"
                );
            }
        }
        Err(CaptureError::NoScreens) | Err(CaptureError::Unsupported { .. }) => {}
        Err(other) => panic!("listing screens failed in an unexpected way: {other}"),
    }
}

#[test]
fn a_captured_frame_matches_the_size_the_screen_promised() {
    // The invariant the encoder relies on: the size a screen advertised is
    // the size the frame actually has.  On a Retina Mac this is where a
    // points/pixels mix-up would show up as a frame twice as wide as the
    // screen claimed.
    if !the_real_screen_may_be_asked_for() {
        skipped_because_the_screen_is_asked_for();
        return;
    }
    let Ok(screens) = list_screens() else {
        // Nothing to check on a machine with no screens.
        return;
    };
    let Ok(frame) = capture_frame(0) else {
        // A locked session or missing permission is not a test failure.
        return;
    };

    assert!(
        screen_matches_frame(&screens[0], &frame),
        "screen 0 promised {}x{} but the frame is {}x{}",
        screens[0].width,
        screens[0].height,
        frame.width(),
        frame.height()
    );
}

#[test]
fn a_reported_size_never_collapses_to_nothing() {
    // Drivers report zero and NaN while a display is being switched.  A
    // single bad value must not turn into a zero-pixel screen, because the
    // encoder cannot allocate a 0x0 frame and the session would die.
    assert_eq!(physical_size(0, 0, 2.0), (1, 1));
    assert_eq!(physical_size(100, 100, 0.0), (100, 100));
    assert_eq!(physical_size(100, 100, f64::NAN), (100, 100));
    assert_eq!(physical_size(100, 100, -3.0), (100, 100));
    assert_eq!(physical_size(100, 100, f64::INFINITY), (100, 100));
}

#[test]
fn a_frame_exposes_its_pixels_and_size() {
    let image = image::RgbaImage::from_pixel(4, 3, image::Rgba([1, 2, 3, 255]));
    let frame = Frame {
        image,
        captured_at: Instant::now(),
    };

    assert_eq!(frame.width(), 4);
    assert_eq!(frame.height(), 3);
    assert_eq!(frame.rgba().len(), 4 * 3 * 4);
}
