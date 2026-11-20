//! Finding out where this machine can be reached, and punching a hole
//! through the NAT so that the other end can reach it.
//!
//! There is no server in the middle of a session, so the two ends have to
//! find each other on their own.  They do it in three steps:
//!
//! 1. **Learn the address.**  Each end asks the tracker what its own
//!    public address looks like from the outside.  The tracker sees the
//!    packet, so it knows the address the packets come from.
//! 2. **Punch.**  Both ends send packets to the other's address at the
//!    same time.  A NAT that sees traffic to an address starts letting
//!    traffic from that address through, so the two ends can talk.
//! 3. **Talk.**  The direct connection is established over the hole.
//!
//! Not every NAT lets this work.  A symmetric NAT that maps every
//! destination to a different port cannot be punched through, and neither
//! can a network that blocks UDP.  When that happens the session fails and
//! the user is told why -- there is no relay to fall back on, because a
//! relay would be exactly the thing this project does not have.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// How long the tracker waits before it answers a "what is my address"
/// question.
const ADDRESS_TIMEOUT: Duration = Duration::from_secs(3);

/// How many packets are sent while punching.
const PUNCH_ATTEMPTS: u32 = 12;

/// The gap between two punches.
const PUNCH_INTERVAL: Duration = Duration::from_millis(250);

/// The body of the question the tracker answers.
#[derive(Debug, Serialize, Deserialize)]
pub struct AddressQuestion {
    /// The id of the asking device.
    pub id: String,
}

/// The answer of the tracker: this is where your packets come from.
#[derive(Debug, Serialize, Deserialize)]
pub struct AddressAnswer {
    /// The address the packets come from.
    pub address: SocketAddr,
}

/// The question that asks for the address of another device.
#[derive(Debug, Serialize, Deserialize)]
pub struct PeerQuestion {
    /// The id of the device to look for.
    pub id: String,
}

/// The answer: this is where the other end is.
#[derive(Debug, Serialize, Deserialize)]
pub struct PeerAnswer {
    /// Whether the device is online.
    pub online: bool,
    /// The address it can be reached at, with the port it listens on.
    pub address: SocketAddr,
}

/// The result of trying to reach the other end.
#[derive(Debug, thiserror::Error)]
pub enum HoleError {
    /// The other end is not online.
    #[error("the other device is not online")]
    NotOnline,
    /// The tracker did not answer in time.
    #[error("the tracker did not answer in time")]
    NoAnswer,
    /// The hole could not be punched.
    #[error("the direct connection could not be made: both ends are behind a NAT that cannot be punched through")]
    CannotPunch,
}

/// The result of a successful punch.
#[derive(Debug)]
pub struct Hole {
    /// The address of the other end.
    pub peer: SocketAddr,
    /// The local socket the traffic goes through.
    pub socket: std::net::UdpSocket,
}

/// Learn the public address of this machine, as the tracker sees it.
///
/// The tracker answers with the address the question came from, which is
/// the address the other end will have to send to.
pub async fn discover_address(tracker: &str, device_id: &str) -> Result<SocketAddr, HoleError> {
    let question = AddressQuestion {
        id: device_id.to_string(),
    };
    let client = reqwest::Client::builder()
        .timeout(ADDRESS_TIMEOUT)
        .build()
        .map_err(|_| HoleError::NoAnswer)?;
    let answer: AddressAnswer = client
        .post(format!("{tracker}/address"))
        .json(&question)
        .send()
        .await
        .map_err(|_| HoleError::NoAnswer)?
        .json()
        .await
        .map_err(|_| HoleError::NoAnswer)?;
    Ok(answer.address)
}

/// Ask the tracker where the other end is.
pub async fn find_peer(tracker: &str, device_id: &str) -> Result<SocketAddr, HoleError> {
    let question = PeerQuestion {
        id: device_id.to_string(),
    };
    let client = reqwest::Client::builder()
        .timeout(ADDRESS_TIMEOUT)
        .build()
        .map_err(|_| HoleError::NoAnswer)?;
    let answer: PeerAnswer = client
        .post(format!("{tracker}/peer"))
        .json(&question)
        .send()
        .await
        .map_err(|_| HoleError::NoAnswer)?
        .json()
        .await
        .map_err(|_| HoleError::NoAnswer)?;
    if !answer.online {
        return Err(HoleError::NotOnline);
    }
    Ok(answer.address)
}

/// Punch a hole towards the other end and wait for an answer.
///
/// Both ends call this at the same time.  The packets go out before the
/// answer is waited for, because the other end's NAT only starts letting
/// the traffic through once it has seen a packet from us.
pub fn punch(socket: std::net::UdpSocket, peer: SocketAddr) -> Result<Hole, HoleError> {
    let deadline = Instant::now() + ADDRESS_TIMEOUT;
    // The socket is blocking here, with a read timeout, so that a
    // waiting recv_from actually waits.  It is set back to
    // non-blocking at the end, because the network library that takes
    // over the socket wants it that way.
    socket.set_read_timeout(Some(PUNCH_INTERVAL)).ok();
    let mut attempt = 0u32;
    while attempt < PUNCH_ATTEMPTS && Instant::now() < deadline {
        // A byte that both ends recognise: the answer carries it back.
        let _ = socket.send_to(PUNCH_MARKER, peer);
        attempt += 1;

        // Is there anything waiting already?
        let mut buffer = [0u8; 64];
        match socket.recv_from(&mut buffer) {
            Ok((read, from)) => {
                if read >= PUNCH_MARKER.len() && &buffer[..PUNCH_MARKER.len()] == PUNCH_MARKER {
                    // The other end started the hole; answer it so it
                    // learns the way back.
                    let _ = socket.send_to(PUNCH_REPLY, from);
                    socket.set_nonblocking(true).ok();
                    return Ok(Hole { peer: from, socket });
                }
                if read >= PUNCH_REPLY.len() && &buffer[..PUNCH_REPLY.len()] == PUNCH_REPLY {
                    socket.set_nonblocking(true).ok();
                    return Ok(Hole { peer, socket });
                }
            }
            Err(_) => {
                // A timeout, or nothing there yet.  Keep punching.
            }
        }
    }
    Err(HoleError::CannotPunch)
}

/// Wait for the other end to punch a hole towards us.
///
/// This is the other half of [`punch`].  The side that waits does not
/// know the other's address yet; it learns it from the packet that
/// arrives, and answers that packet so the other end finds the way
/// back.  Both sides punch at the same time, so the order does not
/// matter.
pub fn punch_wait(socket: std::net::UdpSocket, deadline: Instant) -> Result<Hole, HoleError> {
    socket.set_read_timeout(Some(PUNCH_INTERVAL)).ok();
    let mut buffer = [0u8; 64];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((read, from)) => {
                if read >= PUNCH_MARKER.len() && &buffer[..PUNCH_MARKER.len()] == PUNCH_MARKER {
                    let _ = socket.send_to(PUNCH_REPLY, from);
                    socket.set_nonblocking(true).ok();
                    return Ok(Hole { peer: from, socket });
                }
            }
            Err(_) => {
                // Nothing yet.  Keep waiting.
            }
        }
    }
    Err(HoleError::CannotPunch)
}

/// The byte that starts the hole.  Both ends send it and both ends look
/// for it.
pub const PUNCH_MARKER: &[u8] = b"SCR1";

/// The byte that says the hole is open.
pub const PUNCH_REPLY: &[u8] = b"SCR2";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_and_the_reply_are_different() {
        assert_ne!(PUNCH_MARKER, PUNCH_REPLY);
    }

    #[test]
    fn a_peer_that_is_not_online_is_an_error() {
        let error = HoleError::NotOnline;
        assert!(error.to_string().contains("not online"));
    }

    #[test]
    fn the_question_carries_the_device_id() {
        let question = AddressQuestion { id: "abc".into() };
        let text = serde_json::to_string(&question).expect("serialisable");
        assert!(text.contains("abc"));
    }
}
