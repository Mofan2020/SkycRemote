//! A session the way it really happens, minus the hole.
//!
//! Two ends, one connection, the handshake: they swap keys, prove that
//! they know the access code, and from then on everything is encrypted.
//!
//! The hole through the NAT is not part of this.  It cannot be made
//! on one machine: the tracker sees the address the packets come
//! from, which is the mapping of the NAT and not the port the host
//! listens on.  Two real machines behind two NATs are the only place
//! that works, and that is a thing a person has to try.
//!
//! Nothing here touches a screen, a camera or a microphone.  The
//! picture that is sent is one that is made in memory.

use skycremote_core::handshake::{controller_handshake, host_handshake, HostIdentity};
use skycremote_core::protocol::Message;
use skycremote_core::session::{split_frame, FrameAssembler};
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

/// One session: the handshake, then a control message each way, then a
/// picture.
#[tokio::test]
async fn two_ends_talk_encrypted_after_the_handshake() {
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
    let host = host_task.expect("the host must get in");
    let controller = controller_task.expect("the controller must get in");

    assert_eq!(
        controller.verify_code, host.verify_code,
        "the two ends must show the same six digits"
    );

    // A control message goes over the encrypted channel.
    let text = serde_json::to_string(&Message::ListScreens).expect("serialisable");
    client_sender
        .send_control(bytes::Bytes::from(text))
        .await
        .expect("the message must go out");
    match server_receiver.recv().await {
        Incoming::Control(data) => {
            let text = std::str::from_utf8(&data).expect("readable");
            assert!(
                text.contains("ListScreens"),
                "the message must come back whole: {text}"
            );
        }
        other => panic!("expected a control message, got {other:?}"),
    }

    // The other way round too.
    let text = serde_json::to_string(&Message::Screens {
        screens: Vec::new(),
    })
    .expect("serialisable");
    server_sender
        .send_control(bytes::Bytes::from(text))
        .await
        .expect("the message must go out");
    match client_receiver.recv().await {
        Incoming::Control(data) => {
            let text = std::str::from_utf8(&data).expect("readable");
            assert!(
                text.contains("Screens"),
                "the message must come back whole: {text}"
            );
        }
        other => panic!("expected a control message, got {other:?}"),
    }

    // And a picture goes over the encrypted video channel.
    let mut assembler = FrameAssembler::new();
    assembler.set_picture_size(64, 64);
    let pieces = split_frame(
        &skycremote_core::encode::EncodedFrame {
            data: (0..5000u32).map(|i| (i % 251) as u8).collect(),
            pts: Duration::from_millis(10),
            keyframe: true,
            width: 64,
            height: 64,
        },
        1,
    )
    .expect("splittable");
    for piece in pieces {
        server_sender
            .send_video(piece)
            .expect("the piece must go out");
    }
    let mut arrived = None;
    while arrived.is_none() {
        match client_receiver.recv().await {
            Incoming::Video(datagram) => {
                if let Some(picture) = assembler.push(&datagram).expect("no stale piece") {
                    arrived = Some(picture.data);
                }
            }
            Incoming::Control(_) => continue,
            Incoming::Closed => panic!("the connection closed too early"),
        }
    }
    assert_eq!(arrived.expect("the picture must come back").len(), 5000);
}

/// A controller that does not know the access code gets nothing.
#[tokio::test]
async fn the_wrong_code_gets_no_session() {
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
        controller_handshake(&client_sender, &mut client_receiver, &identity, "000000"),
    );

    assert!(
        controller_task.is_err(),
        "the controller must not get in with the wrong code"
    );
    assert!(
        host_task.is_err(),
        "the host must not let the wrong code in"
    );
}
