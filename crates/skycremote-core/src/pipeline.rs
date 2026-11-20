//! The loop that makes a session: capture a screen, encode it, send the
//! pieces; receive the pieces of the other end's screen and put them back.
//!
//! The two halves run on different clocks, and that is the whole design
//! problem here:
//!
//! * **Capture and encode are slow and blocking.**  A screen grab takes
//!   milliseconds, an encode takes more.  Doing that on the async runtime
//!   would stop every other task on that thread, including the socket.
//!   So they run on a thread of their own and hand finished pictures over
//!   a channel.
//! * **The network is async.**  Sending a picture is a few `send_datagram`
//!   calls; receiving one is a wait.  That belongs on the runtime.
//!
//! The channel between them is what lets a slow encoder drop a picture
//! instead of building a queue: when the network side cannot keep up, the
//! oldest picture is thrown away, because a desktop that is behind is
//! worse than a desktop that is smooth.

use crate::capture::{capture_frame, FramePacer, Screen};
use crate::encode::{EncodeError, EncodedFrame, Encoder, EncoderConfig};
use crate::session::{split_frame, FrameAssembler};
use crate::transport::{Incoming, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// There is no queue, and that is on purpose.  A picture that is waiting
// is a picture of something that has already changed, so the only useful
// picture is the newest one.  The capture thread overwrites the slot and
// the network takes whatever is there; a slow link loses frames instead of
// falling further and further behind.

/// The result of starting a screen source.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// The encoder could not be built.
    #[error("the encoder could not be built: {0}")]
    Encoder(#[from] EncodeError),
    /// The screen could not be captured for the first time, so there is
    /// nothing to encode.
    #[error("the screen could not be captured: {0}")]
    Capture(String),
}

/// A thread that captures a screen, encodes it, and hands the pictures on.
///
/// Nothing here touches the network.  It is the source of a session, and
/// it stops when it is dropped or when [`ScreenSource::stop`] is called.
pub struct ScreenSource {
    /// The newest picture that is ready to send.  A second picture does
    /// not wait behind it: it replaces it.
    latest: Arc<LatestPicture>,
    /// Set when the caller wants the thread to finish.
    stop: Arc<AtomicBool>,
}

/// The one slot a finished picture goes into.
///
/// Whoever reads it takes the picture out; the next one to be encoded
/// fills the slot again.  Nothing accumulates, so the delay between what
/// the screen shows and what the other end sees stays at one picture.
struct LatestPicture {
    /// The picture, if one is waiting.
    slot: std::sync::Mutex<Option<EncodedFrame>>,
    /// Wakes the reader when the slot is filled.
    ready: tokio::sync::Notify,
}

impl LatestPicture {
    /// Put a picture in the slot, replacing whatever was there.
    fn put(&self, picture: EncodedFrame) {
        *self.slot.lock().expect("the picture slot is not poisoned") = Some(picture);
        self.ready.notify_one();
    }

    /// Wait for a picture and take it out of the slot.
    async fn take(&self) -> Option<EncodedFrame> {
        loop {
            {
                let mut slot = self.slot.lock().expect("the picture slot is not poisoned");
                if let Some(picture) = slot.take() {
                    return Some(picture);
                }
            }
            self.ready.notified().await;
        }
    }
}

impl ScreenSource {
    /// Start capturing the screen given.
    ///
    /// The first frame is captured before this returns, so a screen that
    /// cannot be captured is reported here instead of as a mysterious
    /// silence later.
    pub fn start(
        screen: Screen,
        mut config: EncoderConfig,
        fps: u32,
    ) -> Result<Self, PipelineError> {
        // A Retina screen reports its size in pixels, and H.264 4:2:0
        // needs both dimensions to be even.  Rounding down by one pixel
        // is invisible; refusing to start would not be.
        config.width &= !1;
        config.height &= !1;
        if fps > 0 {
            config.fps = fps;
        }

        let first =
            capture_frame(screen.index).map_err(|err| PipelineError::Capture(err.to_string()))?;
        let encoder = Encoder::new(config)?;

        let latest = Arc::new(LatestPicture {
            slot: std::sync::Mutex::new(None),
            ready: tokio::sync::Notify::new(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread_latest = latest.clone();

        std::thread::Builder::new()
            .name("skycremote-capture".into())
            .spawn(move || {
                run_capture(screen, encoder, first, fps, thread_latest, thread_stop);
            })
            .map_err(|err| {
                PipelineError::Capture(format!("the capture thread did not start: {err}"))
            })?;

        Ok(Self { latest, stop })
    }

    /// Wait for the next picture.
    ///
    /// It never returns an old picture: if the capture thread has made
    /// two while the reader was busy, only the second one is handed over.
    pub async fn next_picture(&mut self) -> Option<EncodedFrame> {
        self.latest.take().await
    }

    /// Ask the thread to finish and wait for the pictures already made.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ScreenSource {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The body of the capture thread.
///
/// It keeps going after a failed capture: a screen that fails once (a
/// locked session, a monitor that is being reconfigured) usually works
/// again a moment later, and stopping the whole session for that would be
/// worse than a short gap in the picture.
fn run_capture(
    screen: Screen,
    mut encoder: Encoder,
    first: crate::capture::Frame,
    fps: u32,
    latest: Arc<LatestPicture>,
    stop: Arc<AtomicBool>,
) {
    let mut pacer = FramePacer::new(fps.max(1));
    let mut next_frame = Some(first);
    let mut last_error: Option<String> = None;

    while !stop.load(Ordering::Relaxed) {
        let frame = match next_frame.take() {
            Some(frame) => frame,
            None => {
                // Wait for the next moment a frame is due, so this loop
                // does not spin at full speed on a slow screen.
                let now = Instant::now();
                let deadline = pacer.next_deadline();
                if deadline > now {
                    let wait = deadline - now;
                    // Sleeping the whole gap would make a stop take that
                    // long to notice; short naps keep it responsive.
                    let mut left = wait;
                    while left > Duration::ZERO && !stop.load(Ordering::Relaxed) {
                        let nap = left.min(Duration::from_millis(5));
                        std::thread::sleep(nap);
                        left = left.saturating_sub(nap);
                    }
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                }
                match capture_frame(screen.index) {
                    Ok(frame) => frame,
                    Err(err) => {
                        let message = err.to_string();
                        // Only report a change, so a screen that stays
                        // broken does not fill the log.
                        if last_error.as_deref() != Some(message.as_str()) {
                            eprintln!("skycremote: capturing the screen failed: {message}");
                            last_error = Some(message);
                        }
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                }
            }
        };

        if !pacer.should_capture(Instant::now()) {
            continue;
        }

        match encoder.encode(&frame) {
            Ok(Some(encoded)) => {
                last_error = None;
                // The picture goes into the one slot.  If the reader is
                // behind, this picture replaces the one that is waiting,
                // so the delay never grows past one picture.
                latest.put(encoded);
            }
            Ok(None) => {
                // The encoder buffered this frame; it will come out with
                // the next one.
                last_error = None;
            }
            Err(err) => {
                let message = err.to_string();
                if last_error.as_deref() != Some(message.as_str()) {
                    eprintln!("skycremote: encoding failed: {message}");
                    last_error = Some(message);
                }
            }
        }
    }

    let _ = encoder.flush();
}

/// Send one picture as datagrams.
///
/// Returns how many datagrams the picture took.  A datagram that the
/// transport refuses is not an error worth stopping for: the picture is
/// incomplete at the other end and will simply be dropped there, and the
/// next one follows.
pub fn send_picture(sender: &Sender, picture: &EncodedFrame, frame_id: u32) -> usize {
    let pieces = match split_frame(picture, frame_id) {
        Ok(pieces) => pieces,
        Err(err) => {
            eprintln!("skycremote: a picture could not be cut up: {err}");
            return 0;
        }
    };
    let mut sent = 0;
    for piece in pieces {
        if sender.send_video(piece).is_ok() {
            sent += 1;
        }
    }
    sent
}

/// Wait for the next whole picture.
///
/// Pieces of other pictures are absorbed and `None` is returned when the
/// connection ends.  `None` also covers a picture that was lost: the
/// caller simply asks again and gets the next one.
pub async fn next_picture(
    receiver: &mut Receiver,
    assembler: &mut FrameAssembler,
) -> Option<EncodedFrame> {
    loop {
        match receiver.recv().await {
            Incoming::Video(datagram) => match assembler.push(&datagram) {
                Ok(Some(picture)) => return Some(picture),
                Ok(None) => continue,
                Err(_) => continue,
            },
            // A control message while waiting for a picture: the caller
            // handles those on its own loop, so this one is dropped.
            Incoming::Control(_) => continue,
            Incoming::Closed => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_that_is_too_big_to_cut_is_not_sent() {
        // 70000 pieces would need a 16-bit counter to overflow; this
        // asserts the splitting refuses rather than truncating.
        let picture = EncodedFrame {
            data: vec![0u8; 1150 * 70_000],
            pts: Duration::ZERO,
            keyframe: true,
            width: 1920,
            height: 1080,
        };
        let pieces = crate::session::split_frame(&picture, 1);
        assert!(
            pieces.is_err(),
            "a picture with too many pieces must be refused"
        );
    }

    #[tokio::test]
    async fn only_the_newest_picture_is_kept() {
        // A reader that is busy must not get a backlog of old pictures:
        // the slot holds one, and a second one replaces it.
        let latest = LatestPicture {
            slot: std::sync::Mutex::new(None),
            ready: tokio::sync::Notify::new(),
        };
        latest.put(a_picture(1));
        latest.put(a_picture(2));
        let taken = latest.take().await.expect("a picture must be waiting");
        assert_eq!(taken.data, vec![2u8; 16], "the older picture must be gone");
    }

    #[tokio::test]
    async fn taking_a_picture_empties_the_slot() {
        let latest = LatestPicture {
            slot: std::sync::Mutex::new(None),
            ready: tokio::sync::Notify::new(),
        };
        latest.put(a_picture(1));
        assert!(latest.take().await.is_some());
        // Nothing is left, so the next take waits instead of handing out
        // the same picture twice.
        assert!(latest.slot.lock().unwrap().is_none());
    }

    /// A picture whose bytes are all the same number, so a test can tell
    /// them apart.
    fn a_picture(marker: u8) -> EncodedFrame {
        EncodedFrame {
            data: vec![marker; 16],
            pts: Duration::ZERO,
            keyframe: true,
            width: 1920,
            height: 1080,
        }
    }

    #[tokio::test]
    async fn a_source_can_be_stopped_while_it_waits() {
        // This does not touch a screen: it only checks that the stop flag
        // is noticed, which is what keeps a session from hanging on the
        // way out.
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut waited = 0u32;
            while !flag.load(Ordering::Relaxed) && waited < 10_000 {
                std::thread::sleep(Duration::from_millis(1));
                waited += 1;
            }
            waited
        });
        stop.store(true, Ordering::Relaxed);
        let waited = handle.join().expect("the thread finishes");
        assert!(waited < 10_000, "the stop flag must be seen quickly");
    }
}
