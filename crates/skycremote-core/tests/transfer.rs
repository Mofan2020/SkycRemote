//! The new things a session can carry, tested on a real connection.
//!
//! Nothing here touches a screen, a camera or a microphone.  The
//! connection is a real one over the loopback: two ends, a handshake,
//! and then the files and the clipboard going back and forth.
//!
//! The files that are used live in a folder of their own, so that the
//! files of the machine running the test are never touched.

use skycremote_core::handshake::{controller_handshake, host_handshake, HostIdentity};
use skycremote_core::protocol::Message;
use skycremote_core::transfer;
use skycremote_core::transport::{accept, connect, listen, open, Incoming, Side};
use std::time::Duration;

/// The identity of the machine that is being controlled.
fn identity() -> HostIdentity {
    let salt = b"the salt of this device".to_vec();
    let verifier = skycremote_core::crypto::code_verifier(&salt, "123456");
    HostIdentity {
        device_id: "the-device".into(),
        device_name: "The Device".into(),
        port: 21116,
        salt,
        verifier,
    }
}

/// The two ends of a session, already talking.
async fn a_session() -> (
    skycremote_core::transport::Sender,
    skycremote_core::transport::Receiver,
    skycremote_core::transport::Sender,
    skycremote_core::transport::Receiver,
) {
    let identity = identity();
    let (server, server_address) = listen(0).await.expect("the port must open");
    let client_endpoint = connect(server_address).await.expect("the port must open");

    let accept_task = tokio::spawn(async move {
        accept(&server, Duration::from_secs(20))
            .await
            .expect("the connection must come in")
    });

    let client = client_endpoint
        .connect(server_address, "skycremote")
        .expect("the connection must start")
        .await
        .expect("the connection must be made");
    let server_connection = accept_task.await.expect("the task must finish");

    let (client_side, server_side) = tokio::join!(
        open(client, Side::Initiator),
        open(server_connection, Side::Responder),
    );
    let (client_sender, mut client_receiver) = client_side.expect("the client must open");
    let (server_sender, mut server_receiver) = server_side.expect("the host must open");

    let (host_task, controller_task) = tokio::join!(
        host_handshake(&server_sender, &mut server_receiver, &identity),
        controller_handshake(&client_sender, &mut client_receiver, &identity, "123456"),
    );
    host_task.expect("the host must get in");
    controller_task.expect("the controller must get in");

    (
        client_sender,
        client_receiver,
        server_sender,
        server_receiver,
    )
}

/// Send a message to the other end.
async fn tell(sender: &skycremote_core::transport::Sender, message: &Message) {
    let text = serde_json::to_string(message).expect("the message must be serialisable");
    sender
        .send_control(bytes::Bytes::from(text))
        .await
        .expect("the message must go out");
}

/// Wait for the next message from the other end.
async fn hear(receiver: &mut skycremote_core::transport::Receiver) -> Message {
    loop {
        let Incoming::Control(bytes) = receiver.recv().await else {
            panic!("the channel closed before the message came");
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(message) = serde_json::from_str::<Message>(text) else {
            continue;
        };
        return message;
    }
}

/// A folder of its own for the test.
///
/// It is inside the home folder, because that is the only place the
/// files of the other end can be reached.  The folder is emptied when
/// the test is over.
fn scratch(name: &str) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").expect("there is a home folder");
    let folder = std::path::PathBuf::from(home)
        .join(format!(".skycremote-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("the scratch folder could not be made");
    folder
}

/// Take the folders of the test away when it is over.
fn clean_up() {
    let home = std::env::var_os("HOME").expect("there is a home folder");
    let home = std::path::PathBuf::from(home);
    let Ok(entries) = std::fs::read_dir(&home) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".skycremote-test-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[tokio::test]
async fn a_file_listing_goes_over_the_session() {
    let (_client_sender, mut client_receiver, server_sender, _server_receiver) = a_session().await;

    let outbox = scratch("outbox");
    std::fs::write(outbox.join("note.txt"), b"hello there").expect("the file must be written");

    transfer::send_listing(&server_sender, 7, &outbox.to_string_lossy()).await;

    match hear(&mut client_receiver).await {
        Message::FileList {
            token,
            path,
            entries,
        } => {
            assert_eq!(token, 7, "the answer must belong to the question");
            assert!(path.ends_with("outbox"), "the path is {path}");
            assert_eq!(entries.len(), 1, "there is one file in the folder");
            assert_eq!(entries[0].name, "note.txt");
            assert!(!entries[0].is_dir, "it is a file, not a folder");
            assert_eq!(entries[0].size, 11);
        }
        other => panic!("the wrong answer came back: {other:?}"),
    }
    clean_up();
}

#[tokio::test]
async fn a_file_is_sent_over_the_session() {
    let (_client_sender, mut client_receiver, server_sender, _server_receiver) = a_session().await;

    let outbox = scratch("outbox");
    let content: Vec<u8> = (0..40_000u32).map(|number| (number % 251) as u8).collect();
    std::fs::write(outbox.join("blob.bin"), &content).expect("the file must be written");

    transfer::send_file(
        &server_sender,
        9,
        &outbox.join("blob.bin").to_string_lossy(),
    )
    .await;

    let mut got: Vec<u8> = Vec::new();
    let mut size = 0u64;
    let mut done = false;
    while !done {
        match hear(&mut client_receiver).await {
            Message::FileDownloadStart {
                token, size: total, ..
            } => {
                assert_eq!(token, 9);
                size = total;
            }
            Message::FileDownloadChunk {
                token,
                offset,
                data,
            } => {
                assert_eq!(token, 9);
                let bytes = transfer::decode_piece(&data).expect("the piece must be base64");
                assert_eq!(offset, got.len() as u64, "the pieces come in order");
                got.extend_from_slice(&bytes);
            }
            Message::FileDownloadEnd { token, .. } => {
                assert_eq!(token, 9);
                done = true;
            }
            other => panic!("the wrong message came back: {other:?}"),
        }
    }

    assert_eq!(size, content.len() as u64, "the size must be told up front");
    assert_eq!(got, content, "not one byte may be lost");
    clean_up();
}

#[tokio::test]
async fn a_file_that_is_not_there_is_reported() {
    let (_client_sender, mut client_receiver, server_sender, _server_receiver) = a_session().await;

    let missing = scratch("outbox").join("not-there.txt");
    transfer::send_file(&server_sender, 3, &missing.to_string_lossy()).await;

    match hear(&mut client_receiver).await {
        Message::FileError { token, message } => {
            assert_eq!(token, 3);
            assert!(!message.is_empty(), "the reason must be in words");
        }
        other => panic!("the wrong message came back: {other:?}"),
    }
    clean_up();
}

#[tokio::test]
async fn the_clipboard_goes_over_the_session() {
    let (client_sender, _client_receiver, _server_sender, mut server_receiver) = a_session().await;

    let text = "the text that was copied";
    tell(
        &client_sender,
        &Message::Clipboard {
            text: text.to_string(),
        },
    )
    .await;

    match hear(&mut server_receiver).await {
        Message::Clipboard { text: got } => assert_eq!(got, text),
        other => panic!("the wrong message came back: {other:?}"),
    }
}

#[tokio::test]
async fn the_privacy_screen_goes_over_the_session() {
    let (client_sender, _client_receiver, _server_sender, mut server_receiver) = a_session().await;

    tell(&client_sender, &Message::PrivacyScreen { enabled: true }).await;

    match hear(&mut server_receiver).await {
        Message::PrivacyScreen { enabled } => assert!(enabled),
        other => panic!("the wrong message came back: {other:?}"),
    }
}

#[tokio::test]
async fn the_power_word_goes_over_the_session() {
    let (client_sender, _client_receiver, _server_sender, mut server_receiver) = a_session().await;

    tell(
        &client_sender,
        &Message::Power {
            action: "shutdown".to_string(),
        },
    )
    .await;

    match hear(&mut server_receiver).await {
        Message::Power { action } => assert_eq!(action, "shutdown"),
        other => panic!("the wrong message came back: {other:?}"),
    }
}

#[tokio::test]
async fn the_files_of_the_other_end_stay_inside_its_home_folder() {
    // A path that climbs out of the shared folder is refused, so that
    // knowing the access code is not the same as being able to read
    // the whole machine.
    let result = skycremote_core::files::resolve("../../../../etc/passwd");
    assert!(result.is_err(), "the path must be refused");
}
