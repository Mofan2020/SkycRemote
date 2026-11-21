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
use crate::clipboard;
use crate::encode::EncoderConfig;
use crate::files;
use crate::handshake::{self, HostIdentity, TrustedMachine};
use crate::input;
use crate::pipeline::{self, ScreenSource};
use crate::power::{self, PowerAction};
use crate::protocol::{Message, ScreenInfo};
use crate::transfer;
use crate::transport;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
    /// The other end proved that it knows the access code.
    ///
    /// The six digits are the same on both screens.  When they are
    /// the same, nobody is in the middle.
    Verified {
        /// The six digits the two ends show to each other.
        verify_code: String,
    },
    /// The privacy screen was turned on or off from this side.
    Privacy {
        /// Whether the screen is hidden now.
        enabled: bool,
    },
    /// Something is worth telling the user about, but the share goes on.
    Warning {
        /// What to say.
        message: String,
    },
    /// The machine on the other end asks to be remembered.
    ///
    /// It wants to come in without the access code next time.  The
    /// owner of this machine decides whether it may, so the window is
    /// told and asked.
    TrustRequested {
        /// The id of the machine that is asking.
        controller_id: String,
        /// The name that machine gives itself.
        controller_name: String,
    },
    /// A machine was remembered, and the value it was given.
    ///
    /// The value is what the other end keeps; this end keeps what the
    /// value is turned into, so that the code itself is nowhere to be
    /// found.
    TrustGranted {
        /// The id of the machine that was remembered.
        controller_id: String,
        /// The salt that goes with the value.
        salt: Vec<u8>,
        /// The value itself.
        verifier: [u8; 32],
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
    /// The salt of this device.
    ///
    /// It is different on every device, so that the same access code
    /// means something different on each of them.
    pub salt: Vec<u8>,
    /// What the access code of this device is turned into.
    ///
    /// The code itself is not kept here, only this value.  When the
    /// controller types the code, it turns it into the same value.
    pub verifier: [u8; 32],
    /// The machines this device has decided to remember.
    ///
    /// A machine in this list comes in with the value it was given,
    /// and does not have to say the access code.
    pub trusted: Vec<TrustedMachine>,
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
            salt: Vec::new(),
            verifier: [0u8; 32],
            trusted: Vec::new(),
        }
    }
}

/// What the picture task is told to do.
enum SourceCommand {
    /// Share another screen.
    Switch(u32),
    /// Make the pictures differently from now on.
    Update {
        /// How many pictures a second.
        fps: u32,
        /// How many bits a second they may use.
        bitrate_bps: u32,
    },
}

/// A handle for changing the settings of a share that is running.
///
/// The share is a task of its own, so the one who started it keeps
/// only this handle and sends the new settings through it.
#[derive(Clone)]
pub struct HostUpdates {
    /// Where the new settings go.
    sender: tokio::sync::mpsc::UnboundedSender<HostUpdate>,
}

impl HostUpdates {
    /// Change the settings of the running share.
    ///
    /// It says whether the share was still there to be told.  The
    /// pictures are made again with the new numbers, which costs one
    /// frame.
    pub fn send(&self, options: HostOptions) -> bool {
        self.sender.send(HostUpdate::Options(options)).is_ok()
    }

    /// Turn the privacy screen on or off from this side.
    ///
    /// The privacy screen hides the screen of this machine from the
    /// other end: the picture stops being sent.  It is the thing to
    /// use when somebody is standing behind this machine and should
    /// not see what is going on.
    ///
    /// It says whether the share was still there to be told.
    pub fn set_privacy(&self, enabled: bool) -> bool {
        self.sender.send(HostUpdate::Privacy(enabled)).is_ok()
    }

    /// Remember the machine that is asking to be remembered.
    ///
    /// It says whether the share was still there to be told.  The
    /// value the machine is given is made here and handed over on the
    /// line of the session, so it never touches the disk here.
    pub fn grant_trust(&self, controller_id: &str) -> bool {
        self.sender
            .send(HostUpdate::Trust(controller_id.to_string()))
            .is_ok()
    }

    /// The machines this device remembers, replaced by a new list.
    ///
    /// This is how a machine is forgotten: the list it is in is taken
    /// out, and the new list is put in its place.
    pub fn set_trusted(&self, trusted: Vec<TrustedMachine>) -> bool {
        self.sender.send(HostUpdate::TrustedList(trusted)).is_ok()
    }
}

/// A change to a share that is running.
#[derive(Debug, Clone)]
pub enum HostUpdate {
    /// New numbers for the pictures.
    Options(HostOptions),
    /// The privacy screen is on or off.
    Privacy(bool),
    /// The machine that is asking is to be remembered.
    Trust(String),
    /// The list of remembered machines, replaced by a new one.
    TrustedList(Vec<TrustedMachine>),
}

/// A share that is under the control of whoever started it.
///
/// The service is the one that holds the cancellation: whoever started
/// it keeps the service and can call [`HostService::cancel`], and the
/// share stops at the next place it looks.
pub struct HostService {
    /// How the share is set up.
    options: HostOptions,
    /// The machines this device remembers.
    ///
    /// It sits behind a lock because the window can change it while
    /// the share is on: a machine is remembered, or forgotten.
    trusted: Arc<Mutex<Vec<TrustedMachine>>>,
    /// The signal that says "stop".
    cancel: CancellationToken,
    /// Where the new settings go.
    updates: tokio::sync::mpsc::UnboundedSender<HostUpdate>,
    /// The new settings that come in.
    incoming: tokio::sync::mpsc::UnboundedReceiver<HostUpdate>,
}

impl HostService {
    /// Make a share that is not running yet.
    pub fn new(options: HostOptions) -> Self {
        let (updates, incoming) = tokio::sync::mpsc::unbounded_channel::<HostUpdate>();
        Self {
            trusted: Arc::new(Mutex::new(options.trusted.clone())),
            options,
            cancel: CancellationToken::new(),
            updates,
            incoming,
        }
    }

    /// A handle for changing the settings while the share is on.
    pub fn update_handle(&self) -> HostUpdates {
        HostUpdates {
            sender: self.updates.clone(),
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
    pub async fn run(mut self, events: UnboundedSender<HostEvent>) -> Result<()> {
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
    async fn serve_once(&mut self, events: &UnboundedSender<HostEvent>) -> Result<()> {
        let screens = capture::list_screens().context("the screens could not be listed")?;
        if screens.is_empty() {
            return Err(anyhow!("there are no screens to share"));
        }
        let shared = screens[0].clone();

        let _ = events.send(HostEvent::Waiting);

        // The things that do not change while the share is on are
        // taken out of `self`, so that the only thing borrowed from
        // it further down is the channel of new settings.
        let tracker = self.options.tracker.clone();
        let device_id = self.options.device_id.clone();
        let device_name = self.options.device_name.clone();
        let port = self.options.port;
        let cancel = self.cancel.clone();

        let (sender, mut receiver) = transport::listen_direct_cancellable(
            &tracker,
            &device_id,
            &device_name,
            port,
            WAIT_FOR_THE_OTHER_END,
            &cancel,
        )
        .await
        .context("the other end did not arrive")?;

        // The handshake is where the two ends say who they are and
        // prove that they know the access code.  Nothing is shared
        // before it is done, so a stranger never sees the screen.
        let identity = HostIdentity {
            device_id: device_id.clone(),
            device_name: device_name.clone(),
            port,
            salt: self.options.salt.clone(),
            verifier: self.options.verifier,
        };
        // The machines that are remembered.  The list is taken out
        // here so that the handshake does not keep the lock while it
        // waits for the other end.
        let trusted = match self.trusted.lock() {
            Ok(list) => list.clone(),
            Err(_) => Vec::new(),
        };
        let session = handshake::host_handshake(&sender, &mut receiver, &identity, &trusted)
            .await
            .context("the other end was not let in")?;

        // The privacy screen is on when the other end asks for it.  While
        // it is on, the picture is not sent at all: the other end sees
        // nothing of this machine.
        let privacy = Arc::new(AtomicBool::new(false));

        // The clipboard of this machine is watched, so that the other
        // end can copy from here.  Only the text is sent.
        let _clipboard_task = {
            let sender = sender.clone();
            let cancel = cancel.clone();
            Some(tokio::spawn(clipboard_watch(sender, cancel)))
        };
        let _ = events.send(HostEvent::Verified {
            verify_code: session.verify_code,
        });

        // Who is on the other end.  The name is only a name; the id is
        // what the list of remembered machines is looked up by.
        let peer_controller_id = session.controller_id.clone();
        let peer_controller_name = session.controller_name.clone();

        let _ = events.send(HostEvent::PeerJoined {
            peer: sender.peer_address().to_string(),
            screen: shared.name.clone(),
        });

        // The other end wants to know what screens there are, so that
        // the screen menu has something in it.  It is sent right away
        // rather than waiting to be asked.
        {
            let list: Vec<ScreenInfo> = screens.iter().map(screen_to_info).collect();
            let _ = send(&sender, &Message::Screens { screens: list }).await;
        }

        // The pictures go out on their own task, because they are made at
        // the frame rate and the input has to be answered the moment it
        // arrives.
        let (command_sender, mut command_receiver) =
            tokio::sync::mpsc::unbounded_channel::<SourceCommand>();
        let picture_sender = sender.clone();
        let picture_screens = screens.clone();
        let picture_start = shared.clone();
        let mut fps = self.options.fps;
        let mut bitrate = self.options.bitrate_bps;
        let picture_cancel = cancel.clone();
        let picture_privacy = privacy.clone();
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
                        // With the privacy screen on, nothing is sent.
                        if !picture_privacy.load(Ordering::Relaxed) {
                            pipeline::send_picture(&picture_sender, &picture, frame_id);
                        }
                        frame_id = frame_id.wrapping_add(1);
                    }
                    command = command_receiver.recv() => {
                        let Some(command) = command else { break };
                        match command {
                            SourceCommand::Switch(index) => {
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
                            SourceCommand::Update { fps: new_fps, bitrate_bps: new_bitrate } => {
                                // The pictures are made again with the
                                // new numbers.  One frame is lost while
                                // the capture is set up again.
                                fps = new_fps;
                                bitrate = new_bitrate;
                                source.stop();
                                source = match start_source(&current, bitrate, fps) {
                                    Ok(source) => source,
                                    Err(error) => {
                                        eprintln!("skycremote: the capture could not be set up again: {error}");
                                        return;
                                    }
                                };
                            }
                        }
                    }
                }
            }
            source.stop();
        });

        // The files that are being sent to this machine.
        let mut uploads: HashMap<u64, (std::path::PathBuf, u64)> = HashMap::new();

        // What comes back is the input.
        let mut injector = input::injector()?;
        loop {
            let incoming = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                // The settings can be changed while the share is on.
                // The new numbers are handed to the picture task, and
                // the loop goes on waiting for the input.
                change = self.incoming.recv() => {
                    match change {
                        Some(HostUpdate::Options(options)) => {
                            let _ = command_sender.send(SourceCommand::Update {
                                fps: options.fps,
                                bitrate_bps: options.bitrate_bps,
                            });
                        }
                        Some(HostUpdate::Privacy(enabled)) => {
                            // The privacy screen is turned on from this
                            // side: the other end is told, so that it
                            // knows the screen went dark on purpose.
                            privacy.store(enabled, Ordering::Relaxed);
                            let _ = send(&sender, &Message::PrivacyScreen { enabled }).await;
                            let _ = events.send(HostEvent::Privacy { enabled });
                        }
                        Some(HostUpdate::Trust(controller_id)) => {
                            // The other end asked to be remembered, and
                            // the owner of this machine said yes.  A
                            // value is made here and handed over on the
                            // line of the session, which is already
                            // encrypted, so it never goes through the
                            // air in the clear.
                            let (salt, verifier) = crate::crypto::trusted_pair();
                            let granted = send(
                                &sender,
                                &Message::TrustGrant {
                                    controller_id: controller_id.clone(),
                                    salt: crate::crypto::base64_encode(&salt),
                                    verifier: crate::crypto::base64_encode(&verifier),
                                },
                            )
                            .await
                            .is_ok();
                            if !granted {
                                // The other end is not there any more, so
                                // the value never reached it.  It is not
                                // remembered here either: a value that
                                // nobody holds is of no use to anybody.
                                let _ = events.send(HostEvent::Warning {
                                    message: "对面已经断开了，没有记住".to_string(),
                                });
                                continue;
                            }
                            if let Ok(mut list) = self.trusted.lock() {
                                list.retain(|machine| machine.controller_id != controller_id);
                                list.push(TrustedMachine {
                                    controller_id: controller_id.clone(),
                                    salt: salt.clone(),
                                    verifier,
                                });
                            }
                            let _ = events.send(HostEvent::TrustGranted {
                                controller_id,
                                salt,
                                verifier,
                            });
                        }
                        Some(HostUpdate::TrustedList(list)) => {
                            // The list of remembered machines is
                            // replaced: this is how a machine is
                            // forgotten.
                            if let Ok(mut slot) = self.trusted.lock() {
                                *slot = list;
                            }
                        }
                        None => break,
                    }
                    continue;
                }
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
                            let _ = command_sender.send(SourceCommand::Switch(index));
                        }
                        Message::FileListRequest { token, path } => {
                            transfer::send_listing(&sender, token, &path).await;
                        }
                        Message::FileRequest { token, path } => {
                            transfer::send_file(&sender, token, &path).await;
                        }
                        Message::FileUploadStart { token, path, size } => {
                            let target = files::incoming_path(&path);
                            let result = files::prepare_upload(&target.to_string_lossy());
                            match result {
                                Ok(()) => {
                                    uploads.insert(token, (target, size));
                                }
                                Err(error) => {
                                    let _ = send(
                                        &sender,
                                        &Message::FileError {
                                            token,
                                            message: error.to_string(),
                                        },
                                    )
                                    .await;
                                }
                            }
                        }
                        Message::FileUploadChunk {
                            token,
                            offset,
                            data,
                        } => {
                            let Some((target, _)) = uploads.get(&token) else {
                                continue;
                            };
                            let bytes = match transfer::decode_piece(&data) {
                                Some(bytes) => bytes,
                                None => {
                                    let _ = send(
                                        &sender,
                                        &Message::FileError {
                                            token,
                                            message: "这一块文件读不出来".to_string(),
                                        },
                                    )
                                    .await;
                                    continue;
                                }
                            };
                            if let Err(error) =
                                files::write_chunk(&target.to_string_lossy(), offset, &bytes)
                            {
                                let _ = send(
                                    &sender,
                                    &Message::FileError {
                                        token,
                                        message: error.to_string(),
                                    },
                                )
                                .await;
                            }
                        }
                        Message::FileUploadEnd { token, path } => {
                            let target = uploads.remove(&token);
                            let name = target
                                .map(|(target, _)| {
                                    target
                                        .file_name()
                                        .map(|name| name.to_string_lossy().to_string())
                                        .unwrap_or_else(|| path.clone())
                                })
                                .unwrap_or(path);
                            let _ = events.send(HostEvent::Warning {
                                message: format!("收到了文件 {name}"),
                            });
                        }
                        Message::Clipboard { text } => {
                            // The other end copied something.  It is put
                            // on this machine's clipboard, and it is not
                            // sent back again.
                            if let Err(error) = clipboard::write_text(&text) {
                                eprintln!("skycremote: the clipboard was not written: {error}");
                            }
                        }
                        Message::TrustRequest => {
                            // The other end wants to be remembered, so
                            // that it does not have to say the access
                            // code next time.  The owner of this machine
                            // decides, so the window is told and asked.
                            let _ = events.send(HostEvent::TrustRequested {
                                controller_id: peer_controller_id.clone(),
                                controller_name: peer_controller_name.clone(),
                            });
                        }
                        Message::PrivacyScreen { enabled } => {
                            privacy.store(enabled, Ordering::Relaxed);
                            let _ = events.send(HostEvent::Warning {
                                message: if enabled {
                                    "画面已经停下来了".to_string()
                                } else {
                                    "画面又回来了".to_string()
                                },
                            });
                        }
                        Message::Power { action } => {
                            let Some(wanted) = PowerAction::parse(&action) else {
                                let _ = send(
                                    &sender,
                                    &Message::Error {
                                        code: "unknownPower".to_string(),
                                        message: "看不懂要做什么".to_string(),
                                    },
                                )
                                .await;
                                continue;
                            };
                            let _ = events.send(HostEvent::Warning {
                                message: format!("对面要这台电脑{}", power_word(wanted)),
                            });
                            if let Err(error) = power::perform(wanted) {
                                eprintln!("skycremote: the machine would not do that: {error}");
                                let _ = send(
                                    &sender,
                                    &Message::Error {
                                        code: "powerFailed".to_string(),
                                        message: error,
                                    },
                                )
                                .await;
                            }
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

/// The word a person would use for what the machine is asked to do.
fn power_word(action: PowerAction) -> &'static str {
    match action {
        PowerAction::Shutdown => "关机",
        PowerAction::Restart => "重启",
        PowerAction::Sleep => "睡眠",
        PowerAction::LogOff => "注销",
    }
}

/// Watch the clipboard of this machine and tell the other end about it.
///
/// The text is only sent when it changed, so that the other end is not
/// sent the same thing over and over.  The text that came from the
/// other end is written on this machine, and it is not sent back: that
/// would keep the two ends going back and forth for ever.
async fn clipboard_watch(sender: transport::Sender, cancel: tokio_util::sync::CancellationToken) {
    let mut last = String::new();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(std::time::Duration::from_millis(800)) => {}
        }
        let Ok(text) = clipboard::read_text() else {
            continue;
        };
        if text.is_empty() || text == last {
            continue;
        }
        last = text.clone();
        if send(&sender, &Message::Clipboard { text }).await.is_err() {
            return;
        }
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
