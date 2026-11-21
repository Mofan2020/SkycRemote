//! The end that is being controlled.
//!
//! It does three things: it tells the tracker where it is, it sends
//! its screen to whoever connects, and it turns the input that comes
//! back into actions on this machine.
//!
//! The connection is made the same way as on the other end — straight
//! through, with the tracker only handing out addresses.  There is no
//! server in the middle and no fallback to one.
//!
//! The share can be started and stopped while the program runs, because
//! the window is the one that starts it: the window asks for a share,
//! this serves it, and the window is told what happens through
//! [`HostEvent`].  Stopping is a cancellation, not a kill: the wait for
//! the other end and the session itself both watch for it.

use anyhow::{anyhow, Context, Result};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::capture::{self, Screen};
use crate::encode::EncoderConfig;
use crate::input;
use crate::pipeline::{self, ScreenSource};
use crate::protocol::{Message, ScreenInfo};
use crate::transport;

/// How long to wait for the other end before trying again.
///
/// The other end may not be running yet, and the address may have
/// changed since the last try, so the wait is a round rather than a
/// single attempt.
const WAIT_FOR_THE_OTHER_END: Duration = Duration::from_secs(120);

/// How long to wait before trying again after something went wrong.
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// What the sharing end tells the window about.
///
/// The window is the one that shows the state of the share, so the
/// share has to be able to say where it is: waiting for the other end,
/// serving it, or just plain out of order.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// Waiting for the other end to come.
    Waiting,
    /// The other end is here and the screen is going out.
    PeerJoined {
        /// Where the other end is.
        peer: String,
        /// Which screen is being shared.
        screen: String,
    },
    /// The other end left.
    PeerLeft,
    /// Something is worth telling the user about, but the share goes on.
    Warning {
        /// What to say.
        message: String,
    },
}

/// How the shared end is set up.
#[derive(Debug, Clone)]
pub struct HostOptions {
    /// Where the tracker is.
    pub tracker: String,
    /// The id of this device.
    pub device_id: String,
    /// The name to show for this device.
    pub device_name: String,
    /// The port to listen on.
    pub port: u16,
    /// How many pictures a second to send.
    pub fps: u32,
    /// How many bits a second the pictures may use.
    pub bitrate_bps: u32,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            tracker: "http://127.0.0.1:21115".to_string(),
            device_id: String::new(),
            device_name: String::new(),
            port: 21116,
            fps: 30,
            bitrate_bps: 2_000_000,
        }
    }
}

/// A share that is under the control of whoever started it.
///
/// The service is the one that holds the cancellation: whoever started
/// it keeps the service and can call [`HostService::cancel`], and the
/// share stops at the next place it looks.
pub struct HostService {
    /// How the share is set up.
    options: HostOptions,
    /// The signal that says "stop".
    cancel: CancellationToken,
}

impl HostService {
    /// Make a share that is not running yet.
    pub fn new(options: HostOptions) -> Self {
        Self {
            options,
            cancel: CancellationToken::new(),
        }
    }

    /// Ask the share to stop.
    ///
    /// It stops at the next place it looks: at most one read timeout
    /// for the wait, and right away once the session is up.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The token, for whoever wants to wait for the share to end.
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Share this machine's screen, and take the input that comes back.
    ///
    /// This does not return while the share is on: it keeps listening
    /// so that the other end can come and go.  It returns when the
    /// share is cancelled.
    pub async fn run(self, events: UnboundedSender<HostEvent>) -> Result<()> {
        if self.options.device_id.trim().is_empty() {
            return Err(anyhow!("this device needs an id"));
        }
        // The permission is only needed once somebody is actually going to
        // be typing on this machine, and it can be given at any time.  So
        // a missing one is a word of advice now, not a refusal: the screen
        // can still be shared, and the permission can be handed over while
        // the session is running.
        if !input::can_inject() {
            let _ = events.send(HostEvent::Warning {
                message: input::injection_hint().to_string(),
            });
        }

        loop {
            if self.cancel.is_cancelled() {
                return Ok(());
            }
            match self.serve_once(&events).await {
                Ok(()) => {
                    if self.cancel.is_cancelled() {
                        return Ok(());
                    }
                    let _ = events.send(HostEvent::PeerLeft);
                    let _ = events.send(HostEvent::Waiting);
                }
                Err(error) => {
                    if self.cancel.is_cancelled() {
                        return Ok(());
                    }
                    let _ = events.send(HostEvent::Warning {
                        message: error.to_string(),
                    });
                    // Wait a little before trying again, and give up the
                    // wait as soon as the share is called off.
                    tokio::select! {
                        _ = self.cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(RETRY_AFTER) => {}
                    }
                }
            }
        }
    }

    /// One session: wait for the other end, then serve it until it leaves.
    async fn serve_once(&self, events: &UnboundedSender<HostEvent>) -> Result<()> {
        let screens = capture::list_screens().context("the screens could not be listed")?;
        if screens.is_empty() {
            return Err(anyhow!("there are no screens to share"));
        }
        let shared = screens[0].clone();

        let _ = events.send(HostEvent::Waiting);

        let (sender, mut receiver) = transport::listen_direct_cancellable(
            &self.options.tracker,
            &self.options.device_id,
            &self.options.device_name,
            self.options.port,
            WAIT_FOR_THE_OTHER_END,
            &self.cancel,
        )
        .await
        .context("the other end did not arrive")?;

        let _ = events.send(HostEvent::PeerJoined {
            peer: sender.peer_address().to_string(),
            screen: shared.name.clone(),
        });

        // The pictures go out on their own task, because they are made at
        // the frame rate and the input has to be answered the moment it
        // arrives.
        let (switch_sender, mut switch_receiver) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let picture_sender = sender.clone();
        let picture_screens = screens.clone();
        let picture_start = shared.clone();
        let fps = self.options.fps;
        let bitrate = self.options.bitrate_bps;
        let picture_cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut current = picture_start;
            let mut source = match start_source(&current, bitrate, fps) {
                Ok(source) => source,
                Err(error) => {
                    eprintln!("skycremote: the screen could not be captured: {error}");
                    return;
                }
            };
            let mut frame_id = 1u32;
            loop {
                tokio::select! {
                    _ = picture_cancel.cancelled() => break,
                    picture = source.next_picture() => {
                        let Some(picture) = picture else { break };
                        pipeline::send_picture(&picture_sender, &picture, frame_id);
                        frame_id = frame_id.wrapping_add(1);
                    }
                    index = switch_receiver.recv() => {
                        let Some(index) = index else { break };
                        let Some(next) = picture_screens.get(index as usize) else {
                            continue;
                        };
                        source.stop();
                        current = next.clone();
                        source = match start_source(&current, bitrate, fps) {
                            Ok(source) => source,
                            Err(error) => {
                                eprintln!("skycremote: that screen could not be captured: {error}");
                                return;
                            }
                        };
                        eprintln!("skycremote: now sharing {}", current.name);
                    }
                }
            }
            source.stop();
        });

        // What comes back is the input.
        let mut injector = input::injector()?;
        loop {
            let incoming = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => break,
                incoming = receiver.recv() => incoming,
            };
            match incoming {
                transport::Incoming::Control(bytes) => {
                    let Ok(message) = serde_json::from_slice::<Message>(&bytes) else {
                        continue;
                    };
                    match message {
                        Message::Input(event) => {
                            // macOS throws the events away when the
                            // accessibility permission is missing, and it
                            // says nothing while doing it.  The other
                            // systems refuse in their own words, so the
                            // check is only needed here.
                            #[cfg(target_os = "macos")]
                            {
                                if !input::can_inject() {
                                    eprintln!("skycremote: 輸入沒有送達 —— 還沒有「輔助功能」權限");
                                    continue;
                                }
                            }
                            if let Err(error) = injector.inject(&event, &shared) {
                                eprintln!("skycremote: the input was not carried out: {error}");
                            }
                        }
                        Message::ListScreens => {
                            let list: Vec<ScreenInfo> =
                                screens.iter().map(screen_to_info).collect();
                            let _ = send(&sender, &Message::Screens { screens: list }).await;
                        }
                        Message::SwitchScreen { index } => {
                            let _ = switch_sender.send(index);
                        }
                        _ => {}
                    }
                }
                transport::Incoming::Video(_) => {}
                transport::Incoming::Closed => break,
            }
        }
        Ok(())
    }
}

/// Start capturing and encoding one screen.
fn start_source(
    screen: &Screen,
    bitrate_bps: u32,
    fps: u32,
) -> Result<ScreenSource, crate::pipeline::PipelineError> {
    let mut config = EncoderConfig::for_screen(screen.width, screen.height);
    config.bitrate_bps = bitrate_bps;
    ScreenSource::start(screen.clone(), config, fps)
}

/// One screen, in the shape the other end is told about.
fn screen_to_info(screen: &Screen) -> ScreenInfo {
    ScreenInfo {
        index: screen.index as u32,
        name: screen.name.clone(),
        width: screen.width,
        height: screen.height,
        scale_factor: screen.scale_factor,
        primary: screen.primary,
    }
}

/// Send a message to the other end.
async fn send(sender: &transport::Sender, message: &Message) -> Result<()> {
    let text = serde_json::to_string(message).context("the message could not be written")?;
    sender
        .send_control(bytes::Bytes::from(text))
        .await
        .context("the message could not be sent")
}
