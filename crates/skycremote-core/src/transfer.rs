//! The files that go between the two ends of a session.
//!
//! Three things happen here: the other end asks what is in a folder,
//! the other end asks for a file, and the other end sends a file here.
//!
//! The files of the machine that is being controlled are read and
//! written through [`crate::files`], which keeps everything inside the
//! home folder of the person who is sharing.

use anyhow::Context;

use crate::files;
use crate::protocol::Message;
use crate::transport::Sender;
use base64::Engine;

/// How many bytes go in one piece of a file.
///
/// The control channel carries the pieces as text, so they are big
/// enough to keep the number of them down and small enough that one
/// piece does not hold the channel for a noticeable while.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// Answer a question about what is in a folder.
///
/// The answer carries the same number as the question, so that the
/// other end knows which question it belongs to.
pub async fn send_listing(sender: &Sender, token: u64, path: &str) {
    match files::list(path) {
        Ok((real, entries)) => {
            let _ = send(
                sender,
                &Message::FileList {
                    token,
                    path: files::relative(&real),
                    entries,
                },
            )
            .await;
        }
        Err(error) => {
            let _ = send(
                sender,
                &Message::FileError {
                    token,
                    message: error.to_string(),
                },
            )
            .await;
        }
    }
}

/// Send a file to the other end, a piece at a time.
///
/// The size is told first, so that the other end knows how much to
/// expect and can show how far it has come.
pub async fn send_file(sender: &Sender, token: u64, path: &str) {
    let size = match files::size_of(path) {
        Ok(size) => size,
        Err(error) => {
            let _ = send(
                sender,
                &Message::FileError {
                    token,
                    message: error.to_string(),
                },
            )
            .await;
            return;
        }
    };
    if send(
        sender,
        &Message::FileDownloadStart {
            token,
            path: path.to_string(),
            size,
        },
    )
    .await
    .is_err()
    {
        return;
    }

    let mut offset = 0u64;
    while offset < size {
        let chunk = match files::read_chunk(path, offset, CHUNK_SIZE) {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = send(
                    sender,
                    &Message::FileError {
                        token,
                        message: error.to_string(),
                    },
                )
                .await;
                return;
            }
        };
        if chunk.is_empty() {
            break;
        }
        let data = base64::engine::general_purpose::STANDARD.encode(&chunk);
        if send(
            sender,
            &Message::FileDownloadChunk {
                token,
                offset,
                data,
            },
        )
        .await
        .is_err()
        {
            return;
        }
        offset += chunk.len() as u64;
    }
    let _ = send(
        sender,
        &Message::FileDownloadEnd {
            token,
            path: path.to_string(),
        },
    )
    .await;
}

/// Read a piece of a file that came over the wire.
///
/// It gives back `None` when the piece is not text that makes sense,
/// so that a broken piece does not stop the whole file.
pub fn decode_piece(data: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

/// Send a message to the other end.
async fn send(sender: &Sender, message: &Message) -> anyhow::Result<()> {
    let text = serde_json::to_string(message).context("the message could not be written")?;
    sender
        .send_control(bytes::Bytes::from(text))
        .await
        .context("the message could not be sent")
}
