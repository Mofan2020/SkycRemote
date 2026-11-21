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
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender as ChannelSender;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use skycremote_core::capture;
use skycremote_core::handshake::{self, HostIdentity};
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

/// The tracker the program uses when nothing else is said.
const DEFAULT_TRACKER: &str = "http://127.0.0.1:21115";

/// The bitrate in megabits a second, when nothing else is said.
const DEFAULT_BITRATE_MBPS: u32 = 8;

/// The frames a second, when nothing else is said.
const DEFAULT_FRAMERATE: u32 = 30;

/// The things that are remembered between runs.
///
/// Every setting is kept the way the window wrote it, so a setting
/// the program itself does not use still survives a restart.  The few
/// the program needs are read back by name.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Everything that was written down.
    values: BTreeMap<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        let mut values = BTreeMap::new();
        values.insert("language".to_string(), json!("zh"));
        values.insert("theme".to_string(), json!("system"));
        values.insert("tracker".to_string(), json!(DEFAULT_TRACKER));
        values.insert("quality".to_string(), json!("auto"));
        values.insert("bitrate".to_string(), json!(DEFAULT_BITRATE_MBPS));
        values.insert("framerate".to_string(), json!(DEFAULT_FRAMERATE));
        Self { values }
    }
}

impl Serialize for Settings {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.values.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = BTreeMap::<String, Value>::deserialize(deserializer)?;
        Ok(Self { values })
    }
}

impl Settings {
    /// A setting that is text.
    pub fn text(&self, key: &str) -> String {
        match self.values.get(key) {
            Some(Value::String(text)) => text.clone(),
            _ => Self::default_text(key).to_string(),
        }
    }

    /// A setting that is a number.
    pub fn number(&self, key: &str) -> u32 {
        match self.values.get(key) {
            Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0).round() as u32,
            _ => Self::default_number(key),
        }
    }

    /// Write a text setting down.
    pub fn set_text(&mut self, key: &str, value: &str) {
        self.values
            .insert(key.to_string(), Value::String(value.to_string()));
    }

    /// Write a number setting down.
    pub fn set_number(&mut self, key: &str, value: u32) {
        self.values.insert(key.to_string(), json!(value));
    }

    /// Write a setting down the way it came.
    pub fn put(&mut self, key: &str, value: Value) {
        self.values.insert(key.to_string(), value);
    }

    /// The value a text setting starts with.
    fn default_text(key: &str) -> &'static str {
        match key {
            "language" => "zh",
            "theme" => "system",
            "tracker" => DEFAULT_TRACKER,
            "quality" => "auto",
            _ => "",
        }
    }

    /// The value a number setting starts with.
    fn default_number(key: &str) -> u32 {
        match key {
            "bitrate" => DEFAULT_BITRATE_MBPS,
            "framerate" => DEFAULT_FRAMERATE,
            _ => 0,
        }
    }

    /// Where the tracker is.
    pub fn tracker(&self) -> String {
        self.text("tracker")
    }

    /// The id of this device.
    pub fn device_id(&self) -> String {
        self.text("deviceId")
    }

    /// The name to show for this device.
    pub fn device_name(&self) -> String {
        self.text("deviceName")
    }

    /// The picture quality.
    pub fn quality(&self) -> String {
        self.text("quality")
    }

    /// The bitrate in megabits a second.
    pub fn bitrate_mbps(&self) -> u32 {
        self.number("bitrate")
    }

    /// The frames a second.
    pub fn framerate(&self) -> u32 {
        self.number("framerate")
    }

    /// The access code of this device, the one others type to control it.
    pub fn access_code(&self) -> String {
        self.text("accessCode")
    }

    /// The salt of this device.
    ///
    /// It is made once and then kept, so that the value the access
    /// code is turned into stays the same.
    pub fn salt(&self) -> Vec<u8> {
        match self
            .values
            .get("accessSalt")
            .and_then(|value| value.as_str())
        {
            Some(text) => match hex_to_bytes(text) {
                Some(bytes) if !bytes.is_empty() => bytes,
                _ => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    /// Put the salt of this device down.
    pub fn set_salt(&mut self, salt: &[u8]) {
        self.values
            .insert("accessSalt".to_string(), json!(bytes_to_hex(salt)));
    }

    /// What the access code of this device is turned into.
    pub fn verifier(&self, access_code: &str) -> [u8; 32] {
        let salt = self.salt();
        skycremote_core::crypto::code_verifier(&salt, access_code)
    }

    /// The salt and the value of the access code of this device.
    ///
    /// The salt is made the first time it is asked for and then kept,
    /// so that the value of the access code does not change from one
    /// moment to the next.
    pub fn access_material(&mut self, access_code: &str) -> (Vec<u8>, [u8; 32]) {
        let mut salt = self.salt();
        if salt.is_empty() {
            let mut fresh = [0u8; 32];
            rand::fill(&mut fresh);
            salt = fresh.to_vec();
            self.set_salt(&salt);
        }
        let verifier = self.verifier(access_code);
        (salt, verifier)
    }
}

/// The bytes of a hex text.
fn hex_to_bytes(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for pair in bytes.chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push((high * 16 + low) as u8);
    }
    Some(out)
}

/// The hex text of some bytes.
fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
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
        Self::load_from(&Self::path())
    }

    /// Read the settings from a file.
    ///
    /// A file that is not there, or not readable, means the defaults,
    /// not an error: the program must still start.
    fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&text).unwrap_or_default();

        // The bitrate used to be counted in kilobits.  A number that
        // big can only be one of those, so it is turned back into
        // megabits.
        if settings.number("bitrate") > 1000 {
            let kilobits = settings.number("bitrate");
            settings.set_number("bitrate", kilobits / 1000);
        }
        settings
    }

    /// Write the settings, making the folder if it is not there yet.
    ///
    /// The file is written next to the one it replaces and moved over
    /// it, so that a crash in the middle does not leave half a file
    /// behind: a half file would be read as no settings at all.
    fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// Write the settings to a file.
    fn save_to(&self, path: &Path) -> Result<()> {
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
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text).with_context(|| {
            format!("the settings could not be written: {}", temporary.display())
        })?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("the settings could not be written: {}", path.display()))?;
        Ok(())
    }
}

/// The session that is up, if there is one.
struct Session {
    /// The way out to the other end.
    sender: transport::Sender,
}

/// The file transfers that are going on.
///
/// Every transfer has a number, so that the answer can be paired with
/// the question even when two of them are going at the same time.
#[derive(Debug, Default)]
struct TransferState {
    /// The number of the next transfer.
    next: Mutex<u64>,
    /// The uploads that are going on, by number.
    uploads: Mutex<HashMap<u64, Upload>>,
}

/// One file that is being received.
struct IncomingFile {
    /// The file being written.
    file: std::fs::File,
    /// How many bytes are expected.
    size: u64,
    /// How many bytes have arrived.
    received: u64,
    /// Where the file is being written.
    path: std::path::PathBuf,
}

/// One file that is being sent.
#[derive(Debug)]
struct Upload {
    /// The name of the file.
    name: String,
}

impl TransferState {
    /// The number of the next transfer.
    fn next_token(&self) -> u64 {
        let mut slot = self.next.lock().expect("the transfer number lock");
        *slot += 1;
        *slot
    }

    /// Remember a file that is being sent.
    fn begin_upload(&self, token: u64, name: &str) {
        let mut slot = self.uploads.lock().expect("the upload lock");
        slot.insert(
            token,
            Upload {
                name: name.to_string(),
            },
        );
    }

    /// The file is sent: this gives back its name.
    fn finish_upload(&self, token: u64) -> Option<String> {
        let mut slot = self.uploads.lock().expect("the upload lock");
        slot.remove(&token).map(|upload| upload.name)
    }

    /// Forget a transfer.
    fn cancel(&self, token: u64) {
        let mut uploads = self.uploads.lock().expect("the upload lock");
        uploads.remove(&token);
    }
}

/// One line in the log the window shows.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    /// When it happened, in milliseconds since the epoch.  The window
    /// turns it into the time of the day.
    pub time: u64,
    /// "info", "warning" or "error".
    pub level: String,
    /// What it says.
    pub message: String,
}

/// How many times the connection is made again after the line goes down.
const RECONNECT_TRIES: u32 = 10;

/// How long the first pause between two tries is.  It grows with every
/// try, so that a machine that is not there is not hammered.
const RECONNECT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// How many lines the log keeps.  Older ones are let go.
const LOG_LIMIT: usize = 500;

/// The backend of the window.
pub struct Backend {
    /// Where the answers and the events go.
    out: ChannelSender<Out>,
    /// What is remembered between runs.
    settings: Settings,
    /// The session, when one is up.
    ///
    /// It sits behind a lock because two things need it: the window's
    /// thread, which sends the input, and the task that reads from the
    /// other end, which puts a new session in place when the line goes
    /// down and comes back.
    session: Arc<Mutex<Option<Session>>>,
    /// The file transfers that are going on.
    transfers: Arc<TransferState>,
    /// The signal that stops the session task when the person hangs up.
    session_cancel: Arc<tokio_util::sync::CancellationToken>,
    /// The signal that stops the share, when this machine is sharing.
    host: Option<tokio_util::sync::CancellationToken>,
    /// How to change the settings of the share that is running.
    host_updates: Option<skycremote_core::host::HostUpdates>,
    /// The signal that stops the tracker of this machine, when this
    /// machine is the one that runs it.
    tracker_cancel: Option<tokio_util::sync::CancellationToken>,
    /// The lines of the log.
    logs: Arc<Mutex<Vec<LogEntry>>>,
}

impl Backend {
    /// Make a backend that hands its answers to this channel.
    ///
    /// The tracker can be given from the outside: the command line
    /// tool passes the one that was asked for, and it wins over the
    /// one in the settings.
    pub fn new(out: ChannelSender<Out>, tracker: Option<String>) -> Self {
        let mut settings = Settings::load();
        if let Some(tracker) = tracker {
            if !tracker.trim().is_empty() {
                settings.set_text("tracker", tracker.trim());
            }
        }
        Self {
            out,
            settings,
            session: Arc::new(Mutex::new(None)),
            transfers: Arc::new(TransferState::default()),
            session_cancel: Arc::new(tokio_util::sync::CancellationToken::new()),
            host: None,
            host_updates: None,
            tracker_cancel: None,
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Write a line into the log, and tell the window about it.
    fn log(&self, level: &str, message: impl AsRef<str>) {
        log_to(&self.out, &self.logs, level, message);
    }

    /// Take the commands as they come.
    pub async fn run(mut self, mut requests: tokio::sync::mpsc::UnboundedReceiver<UiRequest>) {
        // Starting the program puts the command line tool on the PATH.
        // It is done here, in the background, so that the window is
        // open before the copy is finished.
        let out = self.out.clone();
        tokio::task::spawn_blocking(move || {
            let report = skycremote_installer::cli::install();
            emit_to(
                &out,
                "cliInstall",
                json!({
                    "ok": report.ok,
                    "message": report.message,
                    "path": report.path.map(|path| path.to_string_lossy().to_string()),
                }),
            );
        });

        // What the program is doing when it starts, and where the
        // settings live.
        self.log(
            "info",
            format!("SkycRemote {} 起来了", env!("CARGO_PKG_VERSION")),
        );
        self.log("info", format!("设置文件在 {}", Settings::path().display()));

        // The tracker that belongs to this machine is started here,
        // so that the person does not have to remember a second
        // program.
        self.ensure_tracker();

        while let Some(request) = requests.recv().await {
            let answer = self.dispatch(&request.command, &request.payload).await;
            match answer {
                Ok(value) => self.reply(request.id, true, value),
                Err(error) => {
                    let message = error.to_string();
                    self.log("error", format!("{} 没有成功：{message}", request.command));
                    self.reply(request.id, false, error_message(&error));
                }
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
            "startHost" => self.start_host(payload).await,
            "stopHost" => self.stop_host(),
            "hostStatus" => Ok(json!({ "sharing": self.host.is_some() })),
            "listScreens" => self.list_screens(),
            "switchScreen" => self.switch_screen(payload).await,
            "sendInput" => self.send_input(payload).await,
            "getSettings" => Ok(json!(self.settings)),
            "getLogs" => Ok(json!({ "entries": self.log_entries() })),
            "trackerInfo" => {
                // What the other computer has to type: the tracker of
                // this machine is only reachable at 127.0.0.1 from
                // here.
                let url = self.settings.tracker();
                let (host, port) =
                    tracker_address(&url).unwrap_or_else(|| ("127.0.0.1".to_string(), 21115));
                let lan = lan_address()
                    .map(|address| format!("http://{address}:{port}"))
                    .unwrap_or_default();
                Ok(json!({
                    "ok": true,
                    "url": url,
                    "lanUrl": lan,
                    "local": is_local_host(&host),
                    "running": port_is_open(&host, port),
                }))
            }
            "clearLogs" => {
                self.clear_logs();
                Ok(json!({ "ok": true }))
            }
            "quit" => {
                // The person asked the program to stop.  The window
                // sees the answer and the event, and stops.
                self.emit("quit", json!({}));
                Ok(json!({ "ok": true }))
            }
            "log" => {
                let level = payload
                    .get("level")
                    .and_then(|value| value.as_str())
                    .unwrap_or("info")
                    .to_string();
                let message = payload
                    .get("message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                if !message.trim().is_empty() {
                    self.log(&level, message);
                }
                Ok(json!({ "ok": true }))
            }
            "appInfo" => Ok(json!({
                "name": "SkycRemote",
                "version": env!("CARGO_PKG_VERSION"),
            })),
            "saveSettings" => self.save_settings(payload),
            "listDevices" => self.list_devices().await,
            "setQuality" | "setBitrate" | "setFramerate" => self.set_quality(command, payload),
            "togglePrivacyScreen" => self.toggle_privacy(payload).await,
            "toggleHostPrivacy" => self.toggle_host_privacy(payload),
            "powerAction" => self.power_action(payload).await,
            "listFiles" => self.list_files(payload).await,
            "downloadFile" => self.download_file(payload).await,
            "uploadStart" => self.upload_start(payload).await,
            "uploadChunk" => self.upload_chunk(payload).await,
            "uploadEnd" => self.upload_end(payload).await,
            "cancelTransfer" => self.cancel_transfer(payload),
            "sendClipboard" => self.send_clipboard(payload).await,
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
        if self.has_session() {
            return Err(anyhow!("there is already a session"));
        }

        let tracker = self.settings.tracker();
        self.emit(
            "connection",
            json!({ "state": "connecting", "deviceId": device_id }),
        );
        self.log("info", format!("正在连 {device_id}，Tracker {tracker}"));

        // The access code of the machine that is being controlled.
        // Without it there is no session: the other end does not let
        // anybody in who cannot say the code.
        let access_code = payload
            .get("accessCode")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if access_code.is_empty() {
            return Err(Refused::refused("noAccessCode", "还没有填访问口令"));
        }

        let identity = HostIdentity {
            device_id: self.settings.device_id(),
            device_name: self.settings.device_name(),
            port: 0,
            salt: Vec::new(),
            verifier: [0u8; 32],
        };
        let (sender, receiver, verify_code) =
            match Self::open_session(&tracker, &device_id, &access_code, &identity).await {
                Ok(parts) => parts,
                Err(error) => {
                    let text = error.to_string();
                    if text.contains("口令") {
                        return Err(Refused::refused("accessDenied", "访问口令不对"));
                    }
                    return Err(anyhow!("握手没有成功：{text}"));
                }
            };

        // Read these before the sender is put away, so that the
        // numbers are of the connection that was just made.
        let peer = sender.peer_address().to_string();
        let latency = sender.latency_us() as f64 / 1000.0;

        self.set_session(Some(Session { sender }));
        self.log("info", format!("连上了 {device_id}"));
        self.emit(
            "connection",
            json!({
                "state": "connected",
                "deviceId": device_id,
                "peer": peer,
                "latencyMs": latency,
                "verifyCode": verify_code,
            }),
        );

        // The other end talks back on its own: the list of screens, the
        // pictures, and the messages of the person on the other side.
        // When the line goes down, this task tries to make it again.
        self.listen(receiver, tracker, device_id.clone(), access_code, identity);

        Ok(json!({
            "ok": true,
            "deviceId": device_id,
            "verifyCode": verify_code,
        }))
    }

    /// Hang up.
    fn disconnect(&mut self) -> Result<Value> {
        self.session_cancel.cancel();
        let had = self.take_session();
        if let Some(session) = had {
            session.sender.close();
            self.log("info", "挂断了");
        }
        self.emit("disconnect", json!({ "reason": "the user hung up" }));
        Ok(json!({ "ok": true }))
    }

    // MARK: - The share

    /// Start sharing the screen of this machine.
    ///
    /// The share runs on its own task and tells the window what
    /// happens through the "host" event, so the window can show it
    /// without waiting for anything.
    async fn start_host(&mut self, payload: &Value) -> Result<Value> {
        if self.host.is_some() {
            return Err(anyhow!("这台电脑已经在共享了"));
        }

        let device_id = text_of(payload, "deviceId")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| self.settings.device_id());
        if device_id.trim().is_empty() {
            return Err(anyhow!("还没有给这台电脑起一个设备 ID"));
        }
        let device_name = text_of(payload, "deviceName")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| self.settings.device_name());

        // The tracker is one thing in one place: the settings.  What
        // the window asked for is not taken from here, so that the
        // control end and the sharing end always use the same one.
        let tracker = self.settings.tracker();

        // The id and the name are settings too, and they are
        // remembered.
        self.settings.set_text("deviceId", &device_id);
        self.settings.set_text("deviceName", &device_name);
        self.settings.save()?;

        // The access code of this machine, and the value it is turned
        // into.  The code itself is what the person tells the other
        // side; the value is what the handshake uses.
        let access_code = self.settings.access_code();
        if access_code.trim().is_empty() {
            return Err(Refused::refused(
                "noAccessCode",
                "还没有设置访问口令，先在设置里填一个再共享",
            ));
        }
        let (salt, verifier) = self.settings.access_material(&access_code);
        self.settings.save()?;

        let mut options = self.host_options();
        options.salt = salt;
        options.verifier = verifier;
        let service = skycremote_core::host::HostService::new(options);
        let token = service.token();
        let updates = service.update_handle();

        let (events_tx, mut events_rx) =
            tokio::sync::mpsc::unbounded_channel::<skycremote_core::host::HostEvent>();
        let out = self.out.clone();
        let logs = Arc::clone(&self.logs);
        tokio::spawn(async move {
            while let Some(event) = events_rx.recv().await {
                let payload = match event {
                    skycremote_core::host::HostEvent::Waiting => {
                        log_to(&out, &logs, "info", "在等对面连过来");
                        json!({ "state": "waiting" })
                    }
                    skycremote_core::host::HostEvent::PeerJoined { peer, screen } => {
                        log_to(&out, &logs, "info", format!("对面连进来了：{peer}"));
                        json!({ "state": "sharing", "peer": peer, "screen": screen })
                    }
                    skycremote_core::host::HostEvent::PeerLeft => {
                        log_to(&out, &logs, "info", "对面断开了");
                        json!({ "state": "waiting" })
                    }
                    skycremote_core::host::HostEvent::Verified { verify_code } => {
                        log_to(
                            &out,
                            &logs,
                            "info",
                            format!("对面已经通过口令核对，安全码 {verify_code}"),
                        );
                        json!({ "state": "sharing", "verifyCode": verify_code })
                    }
                    skycremote_core::host::HostEvent::Privacy { enabled } => {
                        log_to(
                            &out,
                            &logs,
                            "info",
                            if enabled {
                                "隐私屏打开了，画面不再往外发".to_string()
                            } else {
                                "隐私屏关了，画面又发出去了".to_string()
                            },
                        );
                        json!({ "state": "sharing", "privacy": enabled })
                    }
                    skycremote_core::host::HostEvent::Warning { message } => {
                        log_to(&out, &logs, "warning", message.clone());
                        json!({ "state": "warning", "message": message })
                    }
                };
                emit_to(&out, "host", payload);
            }
        });
        tokio::spawn(async move {
            let _ = service.run(events_tx).await;
        });

        self.host = Some(token);
        self.host_updates = Some(updates);
        self.log(
            "info",
            format!(
                "开始共享：设备 ID {device_id}，Tracker {tracker}，码率 {} Mbps，帧率 {}",
                self.settings.bitrate_mbps(),
                self.settings.framerate()
            ),
        );
        self.emit("host", json!({ "state": "waiting" }));
        Ok(json!({ "ok": true, "deviceId": device_id }))
    }

    /// Stop sharing.
    fn stop_host(&mut self) -> Result<Value> {
        if let Some(token) = self.host.take() {
            token.cancel();
            self.log("info", "共享已经停了");
        }
        self.host_updates = None;
        self.emit("host", json!({ "state": "stopped" }));
        Ok(json!({ "ok": true }))
    }

    /// Read what the other end says, and pass it on to the window.
    ///
    /// When the line goes down, the connection is made again by itself:
    /// the person did not ask for it to end, so it should not end.  The
    /// window is told what is going on while that happens.
    fn listen(
        &self,
        receiver: transport::Receiver,
        tracker: String,
        device_id: String,
        access_code: String,
        identity: HostIdentity,
    ) {
        let out = self.out.clone();
        let logs = Arc::clone(&self.logs);
        let session = self.session.clone();
        let cancel = self.session_cancel.clone();
        let downloads: Arc<Mutex<HashMap<u64, IncomingFile>>> =
            Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(async move {
            let mut receiver = receiver;
            loop {
                let closed = Self::read_session(&mut receiver, &out, &downloads).await;
                if cancel.is_cancelled() {
                    // The person hung up on purpose.  Nothing is done.
                    return;
                }
                if !closed {
                    // The other end said goodbye.
                    emit_to(
                        &out,
                        "disconnect",
                        json!({ "reason": "the other end hung up" }),
                    );
                    return;
                }

                // The line went down by itself.  The connection is made
                // again, with a pause that grows a little every time,
                // so that a machine which is not there is not hammered.
                let mut attempt = 0;
                let mut back = false;
                while !back {
                    attempt += 1;
                    if attempt > RECONNECT_TRIES {
                        emit_to(&out, "disconnect", json!({ "reason": "连不上了" }));
                        return;
                    }
                    emit_to(
                        &out,
                        "connection",
                        json!({ "state": "reconnecting", "attempt": attempt }),
                    );
                    let wait = RECONNECT_WAIT * attempt;
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(wait) => {}
                    }
                    match Self::open_session(&tracker, &device_id, &access_code, &identity).await {
                        Ok((sender, new_receiver, verify_code)) => {
                            if let Ok(mut slot) = session.lock() {
                                *slot = Some(Session { sender });
                            }
                            log_to(
                                &out,
                                &logs,
                                "info",
                                format!("第 {attempt} 次重连成功，安全码 {verify_code}"),
                            );
                            emit_to(
                                &out,
                                "connection",
                                json!({
                                    "state": "reconnected",
                                    "attempt": attempt,
                                    "verifyCode": verify_code,
                                }),
                            );
                            receiver = new_receiver;
                            back = true;
                        }
                        Err(error) => {
                            log_to(&out, &logs, "warning", format!("重连没有成功：{error}"));
                        }
                    }
                }
            }
        });
    }

    /// Read one session from the other end.
    ///
    /// It says whether the line went down by itself (true) or the other end
    /// said goodbye (false).  The window is told about everything that
    /// arrives.
    async fn read_session(
        receiver: &mut transport::Receiver,
        out: &ChannelSender<Out>,
        downloads: &Arc<Mutex<HashMap<u64, IncomingFile>>>,
    ) -> bool {
        let mut assembler = FrameAssembler::new();
        loop {
            match receiver.recv().await {
                transport::Incoming::Video(datagram) => match assembler.push(&datagram) {
                    Ok(Some(picture)) => push_picture(out, &picture),
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
                                .map(|screen| serde_json::to_value(screen).unwrap_or(Value::Null))
                                .collect();
                            emit_to(out, "screens", json!({ "screens": screens }));
                        }
                        Message::Error { code, message } => {
                            emit_to(
                                out,
                                "toast",
                                json!({ "kind": "error", "code": code, "message": message }),
                            );
                        }
                        Message::Clipboard { text } => {
                            // The other end copied something.  It is put on
                            // this machine's clipboard, so that pasting
                            // here works, and the window shows it.
                            let _ = skycremote_core::clipboard::write_text(&text);
                            emit_to(out, "clipboard", json!({ "text": text }));
                        }
                        Message::PrivacyScreen { enabled } => {
                            emit_to(out, "privacy", json!({ "enabled": enabled }));
                        }
                        Message::FileList {
                            token,
                            path,
                            entries,
                        } => {
                            let entries: Vec<Value> = entries
                                .iter()
                                .map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null))
                                .collect();
                            emit_to(
                                out,
                                "files",
                                json!({
                                    "token": token,
                                    "path": path,
                                    "entries": entries,
                                }),
                            );
                        }
                        Message::FileDownloadStart { token, path, size } => {
                            let target = download_path(&path);
                            match std::fs::File::create(&target) {
                                Ok(file) => {
                                    let mut downloads =
                                        downloads.lock().expect("the download lock");
                                    downloads.insert(
                                        token,
                                        IncomingFile {
                                            file,
                                            size,
                                            received: 0,
                                            path: target,
                                        },
                                    );
                                }
                                Err(error) => {
                                    emit_to(
                                        out,
                                        "fileError",
                                        json!({
                                            "token": token,
                                            "message": format!("这个文件存不下来：{error}"),
                                        }),
                                    );
                                }
                            }
                            emit_to(
                                out,
                                "fileProgress",
                                json!({
                                    "token": token,
                                    "received": 0,
                                    "size": size,
                                }),
                            );
                        }
                        Message::FileDownloadChunk {
                            token,
                            offset,
                            data,
                        } => {
                            let bytes = match base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                &data,
                            ) {
                                Ok(bytes) => bytes,
                                Err(_) => {
                                    emit_to(
                                        out,
                                        "fileError",
                                        json!({
                                            "token": token,
                                            "message": "这一块文件读不出来".to_string(),
                                        }),
                                    );
                                    continue;
                                }
                            };
                            let mut received = 0u64;
                            let mut total = 0u64;
                            {
                                let mut slot = downloads.lock().expect("the download lock");
                                if let Some(incoming) = slot.get_mut(&token) {
                                    use std::io::{Seek, SeekFrom, Write};
                                    if incoming.file.seek(SeekFrom::Start(offset)).is_ok() {
                                        let _ = incoming.file.write_all(&bytes);
                                    }
                                    incoming.received = offset + bytes.len() as u64;
                                    received = incoming.received;
                                    total = incoming.size;
                                }
                            }
                            emit_to(
                                out,
                                "fileProgress",
                                json!({
                                    "token": token,
                                    "received": received,
                                    "size": total,
                                }),
                            );
                        }
                        Message::FileDownloadEnd { token, path } => {
                            let mut slot = downloads.lock().expect("the download lock");
                            let incoming = slot.remove(&token);
                            drop(slot);
                            let name = std::path::Path::new(&path)
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.clone());
                            let size = incoming.as_ref().map(|file| file.received).unwrap_or(0);
                            let landed = incoming
                                .map(|file| file.path)
                                .unwrap_or_else(|| download_path(&path));
                            emit_to(
                                out,
                                "fileDone",
                                json!({
                                    "token": token,
                                    "name": name,
                                    "size": size,
                                    "path": landed.to_string_lossy().to_string(),
                                }),
                            );
                        }
                        Message::FileError { token, message } => {
                            emit_to(
                                out,
                                "fileError",
                                json!({
                                    "token": token,
                                    "message": message,
                                }),
                            );
                        }
                        other => {
                            emit_to(
                                out,
                                "connection",
                                json!({ "state": "message", "message": other }),
                            );
                        }
                    }
                }
                transport::Incoming::Closed => return true,
            }
        }
    }

    /// Make a connection and do the handshake.
    ///
    /// It gives back the way out, the way in, and the six digits the two
    /// ends show to each other.
    async fn open_session(
        tracker: &str,
        device_id: &str,
        access_code: &str,
        identity: &HostIdentity,
    ) -> Result<(transport::Sender, transport::Receiver, String)> {
        let (sender, mut receiver) = transport::connect_direct(tracker, device_id)
            .await
            .map_err(|error| anyhow!("连不上 {device_id}：{error}"))?;
        match handshake::controller_handshake(&sender, &mut receiver, identity, access_code).await {
            Ok(session) => Ok((sender, receiver, session.verify_code)),
            Err(error) => {
                sender.close();
                Err(error)
            }
        }
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

    // MARK: - The files, the clipboard, the power

    /// Ask the other end what is in a folder.
    async fn list_files(&mut self, payload: &Value) -> Result<Value> {
        let path = text_of(payload, "path").unwrap_or_default();
        let token = self.transfers.next_token();
        self.send(&Message::FileListRequest { token, path }).await?;
        Ok(json!({ "ok": true, "token": token }))
    }

    /// Ask the other end for a file.
    async fn download_file(&mut self, payload: &Value) -> Result<Value> {
        let path = text_of(payload, "path").unwrap_or_default();
        if path.trim().is_empty() {
            return Err(anyhow!("没有说要看哪个文件"));
        }
        let token = self.transfers.next_token();
        self.send(&Message::FileRequest { token, path }).await?;
        Ok(json!({ "ok": true, "token": token }))
    }

    /// Start sending a file to the other end.
    async fn upload_start(&mut self, payload: &Value) -> Result<Value> {
        let name = text_of(payload, "name").unwrap_or_default();
        if name.trim().is_empty() {
            return Err(anyhow!("没有说文件叫什么"));
        }
        let size = payload
            .get("size")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let token = self.transfers.next_token();
        self.transfers.begin_upload(token, &name);
        self.send(&Message::FileUploadStart {
            token,
            path: name,
            size,
        })
        .await?;
        Ok(json!({ "ok": true, "token": token }))
    }

    /// Send one piece of a file that is being uploaded.
    async fn upload_chunk(&mut self, payload: &Value) -> Result<Value> {
        let token = payload
            .get("token")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| anyhow!("这个分片不知道是哪一次上传的"))?;
        let offset = payload
            .get("offset")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let data = text_of(payload, "data").unwrap_or_default();
        let sent = offset + (data.len() as u64 / 4 * 3);
        self.send(&Message::FileUploadChunk {
            token,
            offset,
            data,
        })
        .await?;
        self.emit(
            "uploadProgress",
            json!({ "token": token, "sent": sent, "size": sent }),
        );
        Ok(json!({ "ok": true }))
    }

    /// The file is sent.
    async fn upload_end(&mut self, payload: &Value) -> Result<Value> {
        let token = payload
            .get("token")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| anyhow!("这一次上传没有编号"))?;
        let name = self
            .transfers
            .finish_upload(token)
            .unwrap_or_else(|| "file".to_string());
        self.send(&Message::FileUploadEnd {
            token,
            path: name.clone(),
        })
        .await?;
        self.emit("uploadDone", json!({ "token": token, "name": name }));
        Ok(json!({ "ok": true }))
    }

    /// Give up on a transfer.
    fn cancel_transfer(&mut self, payload: &Value) -> Result<Value> {
        let token = payload
            .get("token")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        self.transfers.cancel(token);
        Ok(json!({ "ok": true }))
    }

    /// Put the text of this machine's clipboard on the other end's.
    async fn send_clipboard(&mut self, payload: &Value) -> Result<Value> {
        let text = text_of(payload, "text").unwrap_or_default();
        self.send(&Message::Clipboard { text }).await?;
        Ok(json!({ "ok": true }))
    }

    /// Hide this machine's screen from the other end, or show it again.
    ///
    /// This is the thing to use when somebody is standing behind this
    /// machine and should not see what is going on: the picture stops
    /// being sent.
    fn toggle_host_privacy(&mut self, payload: &Value) -> Result<Value> {
        let enabled = payload
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let Some(updates) = self.host_updates.clone() else {
            return Err(anyhow!("这台电脑没有在共享"));
        };
        if !updates.set_privacy(enabled) {
            return Err(anyhow!("共享已经停了"));
        }
        Ok(json!({ "ok": true, "enabled": enabled }))
    }

    /// Hide the picture of the other end, or show it again.
    async fn toggle_privacy(&mut self, payload: &Value) -> Result<Value> {
        let enabled = payload
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        self.send(&Message::PrivacyScreen { enabled }).await?;
        Ok(json!({ "ok": true, "enabled": enabled }))
    }

    /// Ask the other end to shut down, restart, sleep or log off.
    async fn power_action(&mut self, payload: &Value) -> Result<Value> {
        let action = text_of(payload, "action").unwrap_or_default();
        let Some(wanted) = skycremote_core::power::PowerAction::parse(&action) else {
            return Err(anyhow!("看不懂要做什么"));
        };
        self.send(&Message::Power {
            action: wanted.as_wire_word().to_string(),
        })
        .await?;
        Ok(json!({ "ok": true }))
    }

    // MARK: - The settings

    /// Remember the settings.
    ///
    /// The settings the program uses are read by name; everything else
    /// is kept the way it came, so a setting the window knows and the
    /// program does not still survives a restart.
    fn save_settings(&mut self, payload: &Value) -> Result<Value> {
        let tracker_before = self.settings.tracker();

        if let Some(value) = payload.get("language").and_then(|value| value.as_str()) {
            self.settings.set_text("language", value);
        }
        if let Some(value) = payload.get("theme").and_then(|value| value.as_str()) {
            self.settings.set_text("theme", value);
        }
        if let Some(value) = payload.get("tracker").and_then(|value| value.as_str()) {
            self.settings.set_text("tracker", value.trim());
        }
        if let Some(value) = payload.get("deviceId").and_then(|value| value.as_str()) {
            self.settings.set_text("deviceId", value.trim());
        }
        if let Some(value) = payload.get("deviceName").and_then(|value| value.as_str()) {
            self.settings.set_text("deviceName", value.trim());
        }
        if let Some(value) = payload.get("quality").and_then(|value| value.as_str()) {
            self.settings.set_text("quality", value);
        }
        if let Some(value) = payload.get("bitrate").and_then(|value| value.as_u64()) {
            self.settings.set_number("bitrate", value as u32);
        }
        if let Some(value) = payload.get("framerate").and_then(|value| value.as_u64()) {
            self.settings.set_number("framerate", value as u32);
        }
        // The access code of this machine.  It is what somebody else
        // types to control this computer.
        if let Some(value) = payload.get("accessCode").and_then(|value| value.as_str()) {
            self.settings.set_text("accessCode", value.trim());
            // The salt and the value the code is turned into follow
            // the code, so that they are ready for the next session.
            let code = value.trim().to_string();
            self.settings.access_material(&code);
        }
        // The codes of the machines this one has connected to.
        if let Some(object) = payload
            .get("savedCodes")
            .and_then(|value| value.as_object())
        {
            let codes: Value = Value::Object(object.clone());
            self.settings.put("savedCodes", codes);
        }
        if let Some(object) = payload.as_object() {
            for (key, value) in object {
                match key.as_str() {
                    "language" | "theme" | "tracker" | "deviceId" | "deviceName" | "quality"
                    | "bitrate" | "framerate" | "accessCode" | "savedCodes" => {}
                    _ => self.settings.put(key, value.clone()),
                }
            }
        }

        self.settings.save()?;
        self.emit("settings", json!(self.settings));

        // What became of the settings that were just saved.  The
        // numbers of the pictures can be changed while the share is
        // on; the tracker and the device id belong to a session and
        // wait for the next one.
        let mut applied = Vec::new();
        let bitrate = self.settings.bitrate_mbps();
        let framerate = self.settings.framerate();
        if let Some(updates) = self.host_updates.clone() {
            if updates.send(self.host_options()) {
                applied.push(format!("码率 {bitrate} Mbps、帧率 {framerate} 已经生效"));
            }
        }
        if tracker_before != self.settings.tracker() {
            applied
                .push("Tracker 改了，正在进行的共享还是旧的，要下次开始共享时才算数".to_string());
            // The tracker of this machine follows the address that is
            // written down now.
            self.ensure_tracker();
        }
        for line in &applied {
            self.log("info", line.clone());
        }

        Ok(json!({
            "ok": true,
            "settings": self.settings,
            "applied": applied,
        }))
    }

    /// Make sure there is a tracker where the settings say there is.
    ///
    /// A tracker that belongs to this machine is started here, so that
    /// the person does not have to remember a second program.  A
    /// tracker that is somewhere else is left alone: that one is
    /// somebody else's to run.
    fn ensure_tracker(&mut self) {
        let url = self.settings.tracker();
        let Some((host, port)) = tracker_address(&url) else {
            self.log("warning", format!("Tracker 地址看不懂：{url}"));
            return;
        };
        if !is_local_host(&host) {
            self.log(
                "info",
                format!("Tracker 在 {url}，那是别的机器上的，本机不另起一个"),
            );
            return;
        }
        if port_is_open(&host, port) {
            self.log("info", format!("本机的 tracker 已经在跑（{host}:{port}）"));
            return;
        }

        // A tracker of our own that is already running is stopped
        // first: the address may have changed.
        if let Some(cancel) = self.tracker_cancel.take() {
            cancel.cancel();
        }

        let cancel = tokio_util::sync::CancellationToken::new();
        self.tracker_cancel = Some(cancel.clone());
        let out = self.out.clone();
        let logs = Arc::clone(&self.logs);

        // The tracker says what it is doing, so that the log of the
        // person shows it: who registered, who was looked for.
        let voice_out = out.clone();
        let voice_logs = Arc::clone(&logs);
        skycremote_tracker::set_voice(move |message| {
            log_to(&voice_out, &voice_logs, "info", message);
        });
        tokio::spawn(async move {
            let server = skycremote_tracker::serve(port);
            tokio::select! {
                _ = cancel.cancelled() => {
                    log_to(&out, &logs, "info", "本机的 tracker 停了");
                }
                result = server => match result {
                    Ok(()) => log_to(&out, &logs, "info", "本机的 tracker 停了"),
                    Err(error) => log_to(
                        &out,
                        &logs,
                        "error",
                        format!("本机的 tracker 起不来：{error}"),
                    ),
                }
            }
        });

        let mut message = format!("本机的 tracker 已经起来了：http://{host}:{port}");
        if let Some(address) = lan_address() {
            message.push_str(&format!(
                "；另一台电脑把 Tracker 填成 http://{address}:{port}"
            ));
        }
        self.log("info", message);
    }

    /// The share, with the settings as they are now.
    fn host_options(&self) -> skycremote_core::host::HostOptions {
        skycremote_core::host::HostOptions {
            tracker: self.settings.tracker(),
            device_id: self.settings.device_id(),
            device_name: self.settings.device_name(),
            port: 21116,
            fps: self.settings.framerate(),
            bitrate_bps: self.settings.bitrate_mbps().saturating_mul(1_000_000),
            salt: Vec::new(),
            verifier: [0u8; 32],
        }
    }

    /// The lines of the log.
    fn log_entries(&self) -> Vec<LogEntry> {
        match self.logs.lock() {
            Ok(list) => list.clone(),
            Err(_) => Vec::new(),
        }
    }

    /// Let the log go.
    fn clear_logs(&self) {
        if let Ok(mut list) = self.logs.lock() {
            list.clear();
        }
    }

    /// The devices the tracker knows about.
    ///
    /// The tracker only knows the devices that registered, and it does
    /// not keep a list of them: it answers questions about one device
    /// at a time.  So the list is built from the ids the user has used
    /// before.
    async fn list_devices(&mut self) -> Result<Value> {
        let mut devices = Vec::new();
        let id = self.settings.device_id();
        if !id.is_empty() {
            devices.push(json!({
                "id": id,
                "name": self.settings.device_name(),
                "online": false,
            }));
        }
        Ok(json!(devices))
    }

    /// Change how the picture is made.
    ///
    /// The numbers can be changed while the share is on, so the new
    /// ones are handed to the share that is running.
    fn set_quality(&mut self, command: &str, payload: &Value) -> Result<Value> {
        match command {
            "setQuality" => {
                if let Some(value) = payload.get("quality").and_then(|value| value.as_str()) {
                    self.settings.set_text("quality", value);
                }
            }
            "setBitrate" => {
                if let Some(value) = payload.get("bitrate").and_then(|value| value.as_u64()) {
                    self.settings.set_number("bitrate", value as u32);
                }
            }
            "setFramerate" => {
                if let Some(value) = payload.get("framerate").and_then(|value| value.as_u64()) {
                    self.settings.set_number("framerate", value as u32);
                }
            }
            _ => {}
        }
        self.settings.save()?;
        if let Some(updates) = self.host_updates.clone() {
            if updates.send(self.host_options()) {
                self.log(
                    "info",
                    format!(
                        "码率 {} Mbps、帧率 {} 已经生效",
                        self.settings.bitrate_mbps(),
                        self.settings.framerate()
                    ),
                );
            }
        }
        Ok(json!({ "ok": true }))
    }

    // MARK: - Small helpers

    /// The way out to the other end, if there is one.
    fn sender(&self) -> Result<transport::Sender> {
        self.session
            .lock()
            .ok()
            .and_then(|session| session.as_ref().map(|session| session.sender.clone()))
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

    /// Whether there is a session.
    fn has_session(&self) -> bool {
        self.session
            .lock()
            .map(|session| session.is_some())
            .unwrap_or(false)
    }

    /// Put a session in place, or take the one that is there away.
    fn set_session(&self, session: Option<Session>) {
        if let Ok(mut slot) = self.session.lock() {
            *slot = session;
        }
    }

    /// Take the session away.
    fn take_session(&self) -> Option<Session> {
        self.session.lock().ok().and_then(|mut slot| slot.take())
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

/// Read a string out of the payload, if it is there.
fn text_of(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

/// Push an event into the window.
fn emit_to(out: &ChannelSender<Out>, name: &str, payload: Value) {
    let _ = out.send(Out::Event {
        name: name.to_string(),
        payload,
    });
}

/// Write a line into the log, and tell the window about it.
///
/// The log belongs to the program, not to the window: the window can
/// show it, but it does not have to be open for a line to be kept.
fn log_to(
    out: &ChannelSender<Out>,
    logs: &Arc<Mutex<Vec<LogEntry>>>,
    level: &str,
    message: impl AsRef<str>,
) {
    let entry = LogEntry {
        time: now_millis(),
        level: level.to_string(),
        message: message.as_ref().to_string(),
    };
    if let Ok(mut list) = logs.lock() {
        list.push(entry.clone());
        if list.len() > LOG_LIMIT {
            let extra = list.len() - LOG_LIMIT;
            list.drain(0..extra);
        }
    }
    let payload = serde_json::to_value(&entry).unwrap_or(Value::Null);
    emit_to(out, "log", payload);
}

/// The host and the port of a tracker address.
fn tracker_address(url: &str) -> Option<(String, u16)> {
    let rest = url
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let rest = rest.split('/').next().unwrap_or("");
    if rest.is_empty() {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().ok()?),
        None => (rest.to_string(), 21115),
    };
    Some((host, port))
}

/// Whether the address points at this machine.
fn is_local_host(host: &str) -> bool {
    matches!(
        host,
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | "0.0.0.0"
    )
}

/// Whether something is listening on that port already.
fn port_is_open(host: &str, port: u16) -> bool {
    let target = match host {
        "localhost" => format!("127.0.0.1:{port}"),
        "0.0.0.0" => format!("127.0.0.1:{port}"),
        other => format!("{other}:{port}"),
    };
    let Ok(address) = target.parse::<std::net::SocketAddr>() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(300)).is_ok()
}

/// The address of this machine on the local network.
///
/// It is the address the other computer has to use: a tracker that
/// runs here is only reachable at 127.0.0.1 from here.
fn lan_address() -> Option<std::net::IpAddr> {
    let interfaces = if_addrs::get_if_addrs().ok()?;
    let mut other = None;
    for interface in interfaces {
        if interface.is_loopback() {
            continue;
        }
        let std::net::IpAddr::V4(address) = interface.ip() else {
            continue;
        };
        if address.is_private() || address.is_link_local() {
            return Some(std::net::IpAddr::V4(address));
        }
        other.get_or_insert(std::net::IpAddr::V4(address));
    }
    other
}

/// Where a file that was sent over is put on this machine.
///
/// It is a folder of its own under the home folder, so that the files
/// that came in are easy to find.
fn download_path(name: &str) -> std::path::PathBuf {
    let safe: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file")
        .chars()
        .map(|character| {
            if character.is_alphanumeric()
                || character == '.'
                || character == '-'
                || character == '_'
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.trim().is_empty() {
        "file".to_string()
    } else {
        safe
    };
    let folder = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("Downloads")
        .join("SkycRemote");
    let _ = std::fs::create_dir_all(&folder);
    folder.join(safe)
}

/// The time of the day, in milliseconds since the epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
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
    if let Some(refused) = error.downcast_ref::<Refused>() {
        return json!({
            "code": refused.code,
            "message": refused.message,
        });
    }
    json!({
        "code": "failed",
        "message": error.to_string(),
    })
}

/// A request that was refused, with a reason the window knows.
///
/// The window turns the code into words, so that a person is told
/// what to do instead of seeing that something went wrong.
#[derive(Debug)]
struct Refused {
    /// The code the window knows.
    code: &'static str,
    /// The sentence for the person.
    message: String,
}

impl Refused {
    /// Make a refusal.
    fn refused(code: &'static str, message: &str) -> anyhow::Error {
        anyhow::Error::new(Self {
            code,
            message: message.to_string(),
        })
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for Refused {}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own for the test, so that the real settings of
    /// this machine are never touched.
    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("skycremote-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("the scratch folder could not be made");
        folder.join("settings.json")
    }

    /// The settings have to survive the trip through the file.  This
    /// is the whole point of the file: what was set stays set.
    #[test]
    fn settings_survive_the_file() {
        let path = scratch("round-trip");

        let mut settings = Settings::default();
        settings.set_text("tracker", "http://example.test:1234");
        settings.set_text("language", "en");
        settings.set_number("bitrate", 42);
        settings.set_number("framerate", 24);
        settings.put("fileDownloadDir", json!("/tmp/where"));
        settings
            .save_to(&path)
            .expect("the settings could not be written");

        let read = Settings::load_from(&path);
        assert_eq!(read.tracker(), "http://example.test:1234");
        assert_eq!(read.text("language"), "en");
        assert_eq!(read.bitrate_mbps(), 42);
        assert_eq!(read.framerate(), 24);
        assert_eq!(read.text("fileDownloadDir"), "/tmp/where");

        let _ = std::fs::remove_dir_all(path.parent().expect("no folder"));
    }

    /// The bitrate used to be counted in kilobits.  A file from back
    /// then must not be read as thousands of megabits.
    #[test]
    fn an_old_bitrate_is_turned_back_into_megabits() {
        let path = scratch("old-bitrate");
        std::fs::write(&path, r#"{"bitrate": 8000}"#).expect("the file could not be written");

        let read = Settings::load_from(&path);
        assert_eq!(read.bitrate_mbps(), 8);

        let _ = std::fs::remove_dir_all(path.parent().expect("no folder"));
    }

    /// A tracker address is read the way a person writes it.
    #[test]
    fn a_tracker_address_is_read() {
        assert_eq!(
            tracker_address("http://127.0.0.1:21115"),
            Some(("127.0.0.1".to_string(), 21115))
        );
        assert_eq!(
            tracker_address("http://192.168.1.10"),
            Some(("192.168.1.10".to_string(), 21115))
        );
        assert_eq!(tracker_address(""), None);
        assert!(is_local_host("127.0.0.1"));
        assert!(is_local_host("localhost"));
        assert!(!is_local_host("192.168.1.10"));
    }

    /// A file that is not there is not an error: the program starts
    /// with the settings it starts with.
    #[test]
    fn a_missing_file_gives_the_defaults() {
        let path = scratch("missing").with_extension("nothing.json");
        let read = Settings::load_from(&path);
        assert_eq!(read.tracker(), DEFAULT_TRACKER);
        assert_eq!(read.bitrate_mbps(), DEFAULT_BITRATE_MBPS);
    }

    /// The access code has to survive the trip through the file, and
    /// the value it is turned into has to stay the same afterwards.
    #[test]
    fn the_access_code_survives_the_file() {
        let path = scratch("access-code");

        let mut settings = Settings::default();
        settings.set_text("accessCode", "8899");
        let (salt, verifier) = settings.access_material("8899");
        settings
            .save_to(&path)
            .expect("the settings could not be written");

        let read = Settings::load_from(&path);
        assert_eq!(read.access_code(), "8899");
        assert_eq!(read.salt(), salt, "the salt must not change");
        assert_eq!(
            read.verifier("8899"),
            verifier,
            "the value of the access code must not change"
        );

        let _ = std::fs::remove_dir_all(path.parent().expect("no folder"));
    }

    /// Two devices have two different codes, and the code of one
    /// device does not work on another one.
    #[test]
    fn the_value_of_the_code_is_different_on_another_device() {
        let mut one = Settings::default();
        let (_, one_verifier) = one.access_material("1234");

        let mut other = Settings::default();
        let (_, other_verifier) = other.access_material("1234");

        assert_ne!(
            one_verifier, other_verifier,
            "the same code on another device must give another value"
        );
    }

    /// The codes of the machines this one has connected to are kept,
    /// one for each of them.
    #[test]
    fn the_codes_of_the_other_machines_are_kept() {
        let path = scratch("saved-codes");

        let mut settings = Settings::default();
        let mut codes = serde_json::Map::new();
        codes.insert("device-one".to_string(), json!("1111"));
        codes.insert("device-two".to_string(), json!("2222"));
        settings.put("savedCodes", Value::Object(codes));
        settings
            .save_to(&path)
            .expect("the settings could not be written");

        let read = Settings::load_from(&path);
        let saved = read
            .values
            .get("savedCodes")
            .and_then(|value| value.as_object())
            .expect("the codes must be there");
        assert_eq!(saved.len(), 2);
        assert_eq!(saved.get("device-one"), Some(&json!("1111")));
        assert_eq!(saved.get("device-two"), Some(&json!("2222")));

        let _ = std::fs::remove_dir_all(path.parent().expect("no folder"));
    }

    /// The hex text and the bytes it stands for are the same thing
    /// both ways round.
    #[test]
    fn hex_and_bytes_are_the_same_thing() {
        let bytes = vec![0u8, 1, 15, 16, 200, 255];
        let text = bytes_to_hex(&bytes);
        assert_eq!(text, "00010f10c8ff");
        assert_eq!(hex_to_bytes(&text), Some(bytes));
        assert_eq!(hex_to_bytes("abc"), None, "half a byte is not a byte");
        assert_eq!(hex_to_bytes("zz"), None, "not hex is not hex");
    }
}
