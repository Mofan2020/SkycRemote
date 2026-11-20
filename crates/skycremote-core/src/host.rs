//! The end that is being controlled.
//!
//! It does three things: it tells the tracker where it is, it sends
//! its screen to whoever connects, and it turns the input that comes
//! back into actions on this machine.
//!
//! The connection is made the same way as on the other end — straight
//! through, with the tracker only handing out addresses.  There is no
//! server in the middle and no fallback to one.

use anyhow::{anyhow, Context, Result};
use std::time::Duration;

use crate::capture::{self, Screen};
use crate::encode::EncoderConfig;
use crate::input::{self, Injector};
use crate::pipeline::{self, ScreenSource};
use crate::protocol::{Message, ScreenInfo};
use crate::transport;

/// How long to wait for the other end before trying again.
///
/// The other end may not be running yet, and the address may have
/// changed since the last try, so the wait is a round rather than a
/// single attempt.
const WAIT_FOR_THE_OTHER_END: Duration = Duration::from_secs(120);

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

/// Share this machine's screen, and take the input that comes back.
///
/// This does not return: it keeps listening so that the other end can
/// come and go.
pub async fn run(options: HostOptions) -> Result<()> {
    if options.device_id.trim().is_empty() {
        return Err(anyhow!("this device needs an id"));
    }
    // The permission is only needed once somebody is actually going to
    // be typing on this machine, and it can be given at any time.  So
    // a missing one is a word of advice now, not a refusal: the screen
    // can still be shared, and the permission can be handed over while
    // the session is running.
    // The check is here (not in the library) so the link to the
    // ApplicationServices framework lands in the final binary.
    #[cfg(target_os = "macos")]
    let has_permission = {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        unsafe { AXIsProcessTrusted() }
    };
    #[cfg(not(target_os = "macos"))]
    let has_permission = false;
    if !has_permission {
        eprintln!(
            "skycremote: 这个程序还不能控制键盘和鼠标（macOS 上是「辅助功能」权限）。\
             画面照常共享，但要操作对面这台机器，得先给这个权限。"
        );
    }

    loop {
        match serve_once(&options).await {
            Ok(()) => {
                eprintln!("skycremote: the other end hung up, waiting for it again");
            }
            Err(error) => {
                eprintln!("skycremote: {error}");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

/// One session: wait for the other end, then serve it until it leaves.
async fn serve_once(options: &HostOptions) -> Result<()> {
    let screens = capture::list_screens().context("the screens could not be listed")?;
    if screens.is_empty() {
        return Err(anyhow!("there are no screens to share"));
    }
    let shared = screens[0].clone();

    let (sender, mut receiver) = transport::listen_direct(
        &options.tracker,
        &options.device_id,
        &options.device_name,
        options.port,
        WAIT_FOR_THE_OTHER_END,
    )
    .await
    .context("the other end did not arrive")?;

    eprintln!(
        "skycremote: the other end is here, sharing {}",
        shared.name
    );

    // The pictures go out on their own task, because they are made at
    // the frame rate and the input has to be answered the moment it
    // arrives.
    let (switch_sender, mut switch_receiver) = tokio::sync::mpsc::unbounded_channel::<u32>();
    let picture_sender = sender.clone();
    let picture_screens = screens.clone();
    let picture_start = shared.clone();
    let fps = options.fps;
    let bitrate = options.bitrate_bps;
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
    });

    // What comes back is the input.
    let mut injector = input::injector()?;
    loop {
        match receiver.recv().await {
            transport::Incoming::Control(bytes) => {
                let Ok(message) = serde_json::from_slice::<Message>(&bytes) else {
                    continue;
                };
                match message {
                    Message::Input(event) => {
                        #[cfg(target_os = "macos")]
                        let has_permission = {
                            #[link(name = "ApplicationServices", kind = "framework")]
                            extern "C" {
                                fn AXIsProcessTrusted() -> bool;
                            }
                            unsafe { AXIsProcessTrusted() }
                        };
                        #[cfg(not(target_os = "macos"))]
                        let has_permission = false;
                        if !has_permission {
                            eprintln!(
                                "skycremote: 輸入沒有送達 —— 還沒有「輔助功能」權限"
                            );
                            continue;
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
