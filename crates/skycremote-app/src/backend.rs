//! What the window asks the network to do.
//!
//! The window talks to the network through one channel: commands come
//! in, events go out.  Everything that touches the screen or the
//! network goes through `skycremote-core`, so the same path is used
//! whether there is a window or not.
//!
//! The window belongs to the thread that draws it, and that thread is
//! not the one that does the network work.  So this side never touches
//! the window: it hands the answers and the events to the channel, and
//! the window's thread picks them up.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc::Sender as ChannelSender;

use skycremote_core::capture;
use skycremote_core::protocol::{InputEvent, Message};
use skycremote_core::session::FrameAssembler;
use skycremote_core::transport;

/// What the backend hands back to the window's thread.
#[derive(Debug)]
pub enum Out {
    /// The answer to a command.
    Reply {
        /// Which command this answers.
        id: u64,
        /// Whether it worked.
        ok: bool,
        /// What it produced.
        value: Value,
    },
    /// Something that happened, pushed into the window.
    Event {
        /// What kind of thing it is.
        name: String,
        /// The details.
        payload: Value,
    },
}

/// A command that came from the window.
#[derive(Debug)]
pub struct UiRequest {
    /// The number the window gave the command, so the answer can be
    /// matched to it.
    pub id: u64,
    /// What to do.
    pub command: String,
    /// The arguments.
    pub payload: Value,
}

impl UiRequest {
    /// Read a request out of the string the window sent.
    pub fn parse(body: &str) -> Option<Self> {
        let outer: Value = serde_json::from_str(body).ok()?;
        let id = outer.get("id")?.as_u64()?;
        let command = outer.get("command")?.as_str()?.to_string();
        // The payload arrives as a string, because the window has
        // already turned it into text before handing it over.
        let payload = match outer.get("payload").and_then(|value| value.as_str()) {
            Some(text) => serde_json::from_str(text).unwrap_or(Value::Null),
            None => Value::Null,
        };
        Some(Self {
            id,
            command,
            payload,
        })
    }
}

/// The things that are remembered between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Which language the interface is in.
    pub language: String,
    /// Which theme the interface uses: `system`, `light` or `dark`.
    pub theme: String,
    /// Where the tracker is.
    pub tracker: String,
    /// The id of this device.
    pub device_id: String,
    /// The name to show for this device.
    pub device_name: String,
    /// The picture quality: `auto`, `high`, `balanced` or `smooth`.
    pub quality: String,
    /// The bitrate in kilobits per second.
    pub bitrate: u32,
    /// The frames per second.
    pub framerate: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "zh".to_string(),
            theme: "system".to_string(),
            tracker: "http://127.0.0.1:21115".to_string(),
            device_id: String::new(),
            device_name: String::new(),
            quality: "balanced".to_string(),
            bitrate: 8000,
            framerate: 30,
        }
    }
}

impl Settings {
    /// Where the settings are kept.
    ///
    /// Windows has no home directory in the same shape, so the roaming
    /// application data is used there.
    fn path() -> PathBuf {
        #[cfg(windows)]
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        #[cfg(not(windows))]
        let base = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config"))
            .unwrap_or_else(|| PathBuf::from("."));

        base.join("skycremote").join("settings.json")
    }

    /// Read the settings.  A file that is not there, or not readable,
    /// means the defaults, not an error: the program must still start.
    fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// Write the settings, making the folder if it is not there yet.
    fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).with_context(|| {
                format!(
                    "the settings folder could not be made: {}",
                    folder.display()
                )
            })?;
        }
        let text =
            serde_json::to_string_pretty(self).context("the settings could not be written")?;
        std::fs::write(&path, text)
            .with_context(|| format!("the settings could not be written: {}", path.display()))?;
        Ok(())
    }
}

/// The session that is up, if there is one.
struct Session {
    /// The way out to the other end.
    sender: transport::Sender,
}

/// The backend of the window.
pub struct Backend {
    /// Where the answers and the events go.
    out: ChannelSender<Out>,
    /// What is remembered between runs.
    settings: Settings,
    /// The session, when one is up.
    session: Option<Session>,
}

impl Backend {
    /// Make a backend that hands its answers to this channel.
    pub fn new(out: ChannelSender<Out>) -> Self {
        Self {
            out,
            settings: Settings::load(),
            session: None,
        }
    }

    /// Take the commands as they come.
    pub async fn run(mut self, mut requests: tokio::sync::mpsc::UnboundedReceiver<UiRequest>) {
        while let Some(request) = requests.recv().await {
            let answer = self.dispatch(&request.command, &request.payload).await;
            match answer {
                Ok(value) => self.reply(request.id, true, value),
                Err(error) => self.reply(request.id, false, error_message(&error)),
            }
        }
    }

    /// Do what the window asked.
    async fn dispatch(&mut self, command: &str, payload: &Value) -> Result<Value> {
        // A mistake in the page is worth having in the log, because
        // the window looks fine even when it is broken.
        if command == "__pageError" {
            let text = payload
                .as_str()
                .map(|value| value.to_string())
                .unwrap_or_else(|| payload.to_string());
            tracing::error!("页面出错：{text}");
            return Ok(Value::Null);
        }
        match command {
            "connect" => self.connect(payload).await,
            "disconnect" => self.disconnect(),
            "listScreens" => self.list_screens(),
            "switchScreen" => self.switch_screen(payload).await,
            "sendInput" => self.send_input(payload).await,
            "getSettings" => Ok(json!(self.settings)),
            "saveSettings" => self.save_settings(payload),
            "listDevices" => self.list_devices().await,
            "setQuality" | "setBitrate" | "setFramerate" => self.set_quality(command, payload),
            "togglePrivacyScreen" => Err(not_done("privacyScreen")),
            "powerAction" => Err(not_done("powerAction")),
            "listFiles" | "uploadFile" | "downloadFile" | "cancelTransfer" => {
                Err(not_done("fileTransfer"))
            }
            "terminalExec" => Err(not_done("terminal")),
            other => Err(anyhow!("the command `{other}` is not known")),
        }
    }

    // MARK: - The session

    /// Connect to the other end, straight through, with no server in
    /// the middle.
    async fn connect(&mut self, payload: &Value) -> Result<Value> {
        let device_id = payload
            .get("deviceId")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if device_id.is_empty() {
            return Err(anyhow!("the id of the device is missing"));
        }
        if self.session.is_some() {
            return Err(anyhow!("there is already a session"));
        }

        let tracker = self.settings.tracker.clone();
        self.emit(
            "connection",
            json!({ "state": "connecting", "deviceId": device_id }),
        );

        let (sender, receiver) = transport::connect_direct(&tracker, &device_id).await?;

        // Read these before the sender is put away, so that the
        // numbers are of the connection that was just made.
        let peer = sender.peer_address().to_string();
        let latency = sender.latency_us() as f64 / 1000.0;

        self.session = Some(Session { sender });
        self.emit(
            "connection",
            json!({
                "state": "connected",
                "deviceId": device_id,
                "peer": peer,
                "latencyMs": latency,
            }),
        );

        // The other end talks back on its own: the list of screens, the
        // pictures, and the messages of the person on the other side.
        self.listen(receiver);

        Ok(json!({ "ok": true, "deviceId": device_id }))
    }

    /// Hang up.
    fn disconnect(&mut self) -> Result<Value> {
        let had = self.session.take();
        if let Some(session) = had {
            session.sender.close();
        }
        self.emit("disconnect", json!({ "reason": "the user hung up" }));
        Ok(json!({ "ok": true }))
    }

    /// Read what the other end says, and pass it on to the window.
    fn listen(&self, mut receiver: transport::Receiver) {
        let out = self.out.clone();
        tokio::spawn(async move {
            let mut assembler = FrameAssembler::new();
            loop {
                match receiver.recv().await {
                    transport::Incoming::Video(datagram) => match assembler.push(&datagram) {
                        Ok(Some(picture)) => {
                            push_picture(&out, &picture);
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    },
                    transport::Incoming::Control(bytes) => {
                        let Ok(text) = std::str::from_utf8(&bytes) else {
                            continue;
                        };
                        let Ok(message) = serde_json::from_str::<Message>(text) else {
                            continue;
                        };
                        match message {
                            Message::Screens { screens } => {
                                let screens: Vec<Value> = screens
                                    .iter()
                                    .map(|screen| {
                                        serde_json::to_value(screen).unwrap_or(Value::Null)
                                    })
                                    .collect();
                                emit_to(&out, "screens", json!({ "screens": screens }));
                            }
                            Message::Error { code, message } => {
                                emit_to(
                                    &out,
                                    "toast",
                                    json!({ "kind": "error", "code": code, "message": message }),
                                );
                            }
                            other => {
                                emit_to(
                                    &out,
                                    "connection",
                                    json!({ "state": "message", "message": other }),
                                );
                            }
                        }
                    }
                    transport::Incoming::Closed => {
                        emit_to(
                            &out,
                            "disconnect",
                            json!({ "reason": "the other end hung up" }),
                        );
                        break;
                    }
                }
            }
        });
    }

    // MARK: - The screen

    /// The screens of this machine.
    fn list_screens(&mut self) -> Result<Value> {
        let screens = capture::list_screens().map_err(|error| anyhow!(error.to_string()))?;
        let screens: Vec<Value> = screens
            .iter()
            .map(|screen| {
                json!({
                    "index": screen.index,
                    "name": screen.name,
                    "width": screen.width,
                    "height": screen.height,
                    "scale_factor": screen.scale_factor,
                    "primary": screen.primary,
                })
            })
            .collect();
        Ok(json!(screens))
    }

    /// Look at another screen.
    async fn switch_screen(&mut self, payload: &Value) -> Result<Value> {
        let index = payload
            .get("index")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| anyhow!("the index of the screen is missing"))?
            as u32;
        self.send(&Message::SwitchScreen { index }).await?;
        Ok(json!({ "ok": true, "index": index }))
    }

    // MARK: - The keyboard and the mouse

    /// Pass an input event on to the other end.
    async fn send_input(&mut self, payload: &Value) -> Result<Value> {
        let event: InputEvent =
            serde_json::from_value(payload.clone()).map_err(|error| anyhow!(error.to_string()))?;
        self.send(&Message::Input(event)).await?;
        Ok(json!({ "ok": true }))
    }

    // MARK: - The settings

    /// Remember the settings.
    fn save_settings(&mut self, payload: &Value) -> Result<Value> {
        if let Some(value) = payload.get("language").and_then(|value| value.as_str()) {
            self.settings.language = value.to_string();
        }
        if let Some(value) = payload.get("theme").and_then(|value| value.as_str()) {
            self.settings.theme = value.to_string();
        }
        if let Some(value) = payload.get("tracker").and_then(|value| value.as_str()) {
            self.settings.tracker = value.to_string();
        }
        if let Some(value) = payload.get("deviceId").and_then(|value| value.as_str()) {
            self.settings.device_id = value.to_string();
        }
        if let Some(value) = payload.get("deviceName").and_then(|value| value.as_str()) {
            self.settings.device_name = value.to_string();
        }
        if let Some(value) = payload.get("quality").and_then(|value| value.as_str()) {
            self.settings.quality = value.to_string();
        }
        if let Some(value) = payload.get("bitrate").and_then(|value| value.as_u64()) {
            self.settings.bitrate = value as u32;
        }
        if let Some(value) = payload.get("framerate").and_then(|value| value.as_u64()) {
            self.settings.framerate = value as u32;
        }
        self.settings.save()?;
        self.emit("settings", json!(self.settings));
        Ok(json!({ "ok": true }))
    }

    /// The devices the tracker knows about.
    ///
    /// The tracker only knows the devices that registered, and it does
    /// not keep a list of them: it answers questions about one device
    /// at a time.  So the list is built from the ids the user has used
    /// before.
    async fn list_devices(&mut self) -> Result<Value> {
        let mut devices = Vec::new();
        let id = self.settings.device_id.clone();
        if !id.is_empty() {
            devices.push(json!({
                "id": id,
                "name": self.settings.device_name,
                "online": false,
            }));
        }
        Ok(json!(devices))
    }

    /// Change how the picture is made.
    fn set_quality(&mut self, command: &str, payload: &Value) -> Result<Value> {
        match command {
            "setQuality" => {
                if let Some(value) = payload.get("quality").and_then(|value| value.as_str()) {
                    self.settings.quality = value.to_string();
                }
            }
            "setBitrate" => {
                if let Some(value) = payload.get("bitrate").and_then(|value| value.as_u64()) {
                    self.settings.bitrate = value as u32;
                }
            }
            "setFramerate" => {
                if let Some(value) = payload.get("framerate").and_then(|value| value.as_u64()) {
                    self.settings.framerate = value as u32;
                }
            }
            _ => {}
        }
        self.settings.save()?;
        Ok(json!({ "ok": true }))
    }

    // MARK: - Small helpers

    /// The way out to the other end, if there is one.
    fn sender(&self) -> Result<&transport::Sender> {
        self.session
            .as_ref()
            .map(|session| &session.sender)
            .ok_or_else(|| anyhow!("there is no session"))
    }

    /// Send a message to the other end.
    ///
    /// The sender is borrowed, not copied: the stream underneath does
    /// not allow two owners, so the session keeps the only one and
    /// lends it for the length of the message.
    async fn send(&self, message: &Message) -> Result<()> {
        let text = serde_json::to_string(message).context("the message could not be written")?;
        self.sender()?
            .send_control(bytes::Bytes::from(text))
            .await
            .context("the message could not be sent")
    }

    /// Answer a command.
    fn reply(&self, id: u64, ok: bool, value: Value) {
        let _ = self.out.send(Out::Reply { id, ok, value });
    }

    /// Push an event into the window.
    fn emit(&self, name: &str, payload: Value) {
        emit_to(&self.out, name, payload);
    }
}

/// Push an event into the window.
fn emit_to(out: &ChannelSender<Out>, name: &str, payload: Value) {
    let _ = out.send(Out::Event {
        name: name.to_string(),
        payload,
    });
}

/// Put one picture on the screen of the window.
///
/// The picture goes over as the H.264 that the encoder produced, not
/// as pixels.  The browser has a decoder for that, and sending the
/// compressed form is what keeps a full-screen picture inside a few
/// hundred kilobytes a second instead of several megabytes.
///
/// The bytes are written as text, because that is all that can cross
/// from here into the page.
fn push_picture(out: &ChannelSender<Out>, picture: &skycremote_core::encode::EncodedFrame) {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &picture.data);
    emit_to(
        out,
        "frame",
        json!({
            "width": picture.width,
            "height": picture.height,
            "keyframe": picture.keyframe,
            "bytes": picture.data.len(),
            "ptsNs": picture.pts.as_nanos() as u64,
            "codec": "h264",
            "data": encoded,
        }),
    );
}

/// The error the window is shown.
///
/// The window has two languages, so the message is a code and the
/// window turns it into words.  A message that is not a known code is
/// shown as it is.
fn error_message(error: &anyhow::Error) -> Value {
    if let Some(code) = error.downcast_ref::<NotDone>() {
        return json!({
            "code": "notImplemented",
            "feature": code.0,
            "message": format!("{} is not connected yet", code.0),
        });
    }
    json!({
        "code": "failed",
        "message": error.to_string(),
    })
}

/// A part of the program that is not there yet.
#[derive(Debug)]
struct NotDone(&'static str);

impl std::fmt::Display for NotDone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} is not connected yet", self.0)
    }
}

impl std::error::Error for NotDone {}

/// The error for a part of the program that is not there yet.
fn not_done(feature: &'static str) -> anyhow::Error {
    anyhow::Error::new(NotDone(feature))
}
