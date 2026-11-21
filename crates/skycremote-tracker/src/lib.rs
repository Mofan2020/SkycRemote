//! The tracker of SkycRemote.
//!
//! It does two things and both of them are about addresses:
//!
//! * it tells a device where its own packets come from, so the other end
//!   knows where to send to;
//! * it tells a device where the other end is.
//!
//! It never carries a byte of the session.  That is the whole point of
//! the project: a session that cannot be made direct simply fails, and
//! the user is told why.  There is no second path.
//!
//! The tracker holds nothing about the content of a session, so it does
//! not need to be trusted with it.  A tracker that lies about an address
//! can make a session fail; it cannot read one.

use axum::{
    extract::{ConnectInfo, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};

/// What the tracker says when it does something.
type Voice = Box<dyn Fn(&str) + Send + Sync>;

/// The place where the tracker says what it is doing.
///
/// It is set once, by the program that runs the tracker inside the
/// window: that is where the log of the person lives.  The tracker
/// itself only needs to say the words.
static VOICE: OnceLock<Voice> = OnceLock::new();

/// Let the tracker say what it is doing.
///
/// The tracker only ever talks about addresses, so a line is short.
pub fn set_voice(voice: impl Fn(&str) + Send + Sync + 'static) {
    let _ = VOICE.set(Box::new(voice));
}

/// Say one line, if somebody is listening.
fn say(message: &str) {
    if let Some(voice) = VOICE.get() {
        voice(message);
    }
}

/// The state the tracker keeps.
#[derive(Clone, Default)]
struct Inner {
    /// The devices that are online, by their id.
    devices: Arc<Mutex<HashMap<String, Device>>>,
}

/// One device that is online.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Device {
    /// The id of the device.
    id: String,
    /// The name to show for it.
    name: String,
    /// The address it can be reached at, with the port it listens on.
    address: SocketAddr,
    /// When it was last seen, in milliseconds since the epoch.
    last_seen: u64,
}

/// The body of a registration.
#[derive(Debug, Deserialize)]
struct Register {
    /// The id of the device.
    id: String,
    /// The name to show for it.
    name: String,
    /// The port it listens on.
    port: u16,
}

/// The answer to a registration.
#[derive(Debug, Serialize)]
struct RegisterReply {
    /// Whether the device was accepted.
    ok: bool,
}

/// The question that asks "where do my packets come from".
#[derive(Debug, Deserialize)]
struct AddressQuestion {
    /// The id of the asking device.
    id: String,
}

/// The answer: this is where your packets come from.
#[derive(Debug, Serialize)]
struct AddressAnswer {
    /// The address the packets come from.
    address: SocketAddr,
}

/// The question that asks where another device is.
#[derive(Debug, Deserialize)]
struct PeerQuestion {
    /// The id of the device to look for.
    id: String,
}

/// The answer: this is where the other end is.
#[derive(Debug, Serialize)]
struct PeerAnswer {
    /// Whether the device is online.
    online: bool,
    /// The address it can be reached at, with the port it listens on.
    address: SocketAddr,
}

/// The answer of the health check.
#[derive(Debug, Serialize)]
struct Health {
    /// Always "ok", when the tracker answers at all.
    status: String,
}

/// The milliseconds since the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Register a device, or refresh the registration of one that is already
/// known.
async fn register(
    State(inner): State<Inner>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    Json(body): Json<Register>,
) -> Json<RegisterReply> {
    let mut devices = inner
        .devices
        .lock()
        .expect("the device table is not poisoned");
    // The address the packets came from carries the address of the
    // machine; the port is the one the device said it listens on, which
    // is not the same as the port the packet came from.
    devices.insert(
        body.id.clone(),
        Device {
            id: body.id.clone(),
            name: body.name,
            address: SocketAddr::new(from.ip(), body.port),
            last_seen: now_ms(),
        },
    );
    say(&format!("登记：{}（{}:{}）", body.id, from.ip(), body.port));
    Json(RegisterReply { ok: true })
}

/// Tell a device where its own packets come from.
///
/// The tracker sees the packet, so it knows the address the other end
/// will have to send to.  This is what makes the hole punching possible.
async fn address(
    State(inner): State<Inner>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    Json(body): Json<AddressQuestion>,
) -> Json<AddressAnswer> {
    let mut devices = inner
        .devices
        .lock()
        .expect("the device table is not poisoned");
    let now = now_ms();
    // The device that asks is online, whatever it told before.
    devices
        .entry(body.id.clone())
        .and_modify(|device| device.last_seen = now)
        .or_insert(Device {
            id: body.id.clone(),
            name: String::new(),
            address: from,
            last_seen: now,
        });
    say(&format!("问自己的地址：{}（{from}）", body.id));
    Json(AddressAnswer { address: from })
}

/// Tell a device where the other end is.
async fn peer(State(inner): State<Inner>, Json(body): Json<PeerQuestion>) -> Json<PeerAnswer> {
    let mut devices = inner
        .devices
        .lock()
        .expect("the device table is not poisoned");
    // A device that has not said anything for two minutes is gone.
    let stale = now_ms().saturating_sub(120_000);
    devices.retain(|_, device| device.last_seen > stale);
    match devices.get(&body.id) {
        Some(device) => {
            say(&format!("问 {} 在哪：在线（{}）", body.id, device.address));
            Json(PeerAnswer {
                online: true,
                address: device.address,
            })
        }
        None => {
            say(&format!("问 {} 在哪：不在线", body.id));
            Json(PeerAnswer {
                online: false,
                address: SocketAddr::from(([0, 0, 0, 0], 0)),
            })
        }
    }
}

/// The health check.
async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
    })
}

/// Build the router.
fn router() -> Router {
    Router::new()
        .route("/register", post(register))
        .route("/address", post(address))
        .route("/peer", post(peer))
        .route("/health", get(health))
        .with_state(Inner::default())
}

/// Start the tracker on the given port.
///
/// The connection information is needed to see where the packets come
/// from, so the router is served with it.
pub async fn serve(port: u16) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "the tracker is listening");
    axum::serve(
        listener,
        router().into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_device_that_registers_can_be_looked_up() {
        let inner = Inner::default();
        {
            let mut devices = inner.devices.lock().unwrap();
            devices.insert(
                "abc".into(),
                Device {
                    id: "abc".into(),
                    name: "机器".into(),
                    address: "1.2.3.4:21116".parse().unwrap(),
                    last_seen: now_ms(),
                },
            );
        }
        let answer = peer(
            State(inner.clone()),
            Json(PeerQuestion { id: "abc".into() }),
        )
        .await;
        let body = answer.0;
        assert!(body.online);
        assert_eq!(body.address.port(), 21116);
        assert_eq!(body.address.ip().to_string(), "1.2.3.4");
    }

    #[tokio::test]
    async fn an_unknown_device_is_not_online() {
        let inner = Inner::default();
        let answer = peer(State(inner), Json(PeerQuestion { id: "nope".into() })).await;
        assert!(!answer.0.online);
        assert_eq!(answer.0.address.port(), 0);
    }

    #[tokio::test]
    async fn a_device_that_went_away_is_forgotten() {
        let inner = Inner::default();
        {
            let mut devices = inner.devices.lock().unwrap();
            devices.insert(
                "old".into(),
                Device {
                    id: "old".into(),
                    name: "旧机器".into(),
                    address: "1.2.3.4:21116".parse().unwrap(),
                    last_seen: now_ms().saturating_sub(300_000),
                },
            );
        }
        let answer = peer(State(inner), Json(PeerQuestion { id: "old".into() })).await;
        assert!(
            !answer.0.online,
            "a device that is silent for five minutes is gone"
        );
    }
}
