//! The whole path, once.
//!
//! A picture is made, encoded, cut into pieces, sent over a real
//! connection, put back together on the other side and decoded.  Then it
//! is compared with the picture that went in.
//!
//! Every part of this has its own test.  This one is here to catch the
//! mistakes that only appear when the parts are joined: a header that is
//! counted wrong, a piece that is dropped, a stream that mixes two
//! messages.

use image::RgbaImage;
use skycremote_core::capture::Frame;
use skycremote_core::encode::{Encoder, EncoderConfig};
use skycremote_core::session::{split_frame, FrameAssembler};
use skycremote_core::transport::{accept, connect, listen, open, Incoming, Side};
use std::time::{Duration, Instant};

/// How big the picture is.  Small enough to encode quickly, big enough
/// that the picture is cut into several pieces.
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

/// A picture with a colour that changes in every direction, so a mistake
/// that shifts or scrambles the picture shows up in the comparison.
fn a_test_picture() -> RgbaImage {
    let mut image = RgbaImage::new(WIDTH, HEIGHT);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]);
    }
    image
}

/// The average difference between two pictures, per channel.
///
/// A lossy codec does not give the same bytes back, so the comparison is
/// about how far off the result is, not whether it is equal.
fn average_difference(left: &RgbaImage, right: &RgbaImage) -> f64 {
    let mut total = 0u64;
    let mut count = 0u64;
    for (a, b) in left.pixels().zip(right.pixels()) {
        let a = a.0;
        let b = b.0;
        for index in 0..3 {
            total += u64::from(a[index].abs_diff(b[index]));
            count += 1;
        }
    }
    if count == 0 {
        return f64::MAX;
    }
    total as f64 / count as f64
}

#[tokio::test]
async fn a_picture_survives_the_whole_path() {
    // The two ends of a session.
    let (server, server_address) = listen(0).await.expect("the port must open");
    let client_endpoint = connect(server_address).await.expect("the port must open");

    let accept_task = tokio::spawn(async move {
        accept(&server, Duration::from_secs(10))
            .await
            .expect("the connection must come in")
    });

    let client = client_endpoint
        .connect(server_address, "skycremote")
        .expect("the connection must start")
        .await
        .expect("the connection must be made");
    let server_connection = accept_task.await.expect("the task must finish");

    let (client_sender, _client_receiver) = open(client, Side::Initiator)
        .await
        .expect("the channels must open");
    let (_server_sender, mut server_receiver) = open(server_connection, Side::Responder)
        .await
        .expect("the channels must open");

    // The picture that goes in.
    let original = a_test_picture();
    let frame = Frame {
        image: original.clone(),
        captured_at: Instant::now(),
    };

    // It is encoded.
    let config = EncoderConfig {
        width: WIDTH,
        height: HEIGHT,
        fps: 30,
        bitrate_bps: 4_000_000,
        keyframe_interval: 60,
    };
    let mut encoder = Encoder::new(config).expect("the encoder must start");
    let encoded = encoder
        .encode(&frame)
        .expect("the picture must encode")
        .expect("a picture must come out");
    assert!(encoded.keyframe, "the first picture must stand on its own");
    assert!(!encoded.data.is_empty());

    // It is cut into pieces and sent.
    let pieces = split_frame(&encoded, 1).expect("the picture must be cut up");
    assert!(pieces.len() > 1, "this picture must need several pieces");
    for piece in &pieces {
        client_sender
            .send_video(piece.clone())
            .expect("the piece must go out");
    }

    // It is put back together on the other side.
    let mut assembler = FrameAssembler::new();
    assembler.set_picture_size(WIDTH, HEIGHT);
    let mut arrived = None;
    while arrived.is_none() {
        match server_receiver.recv().await {
            Incoming::Video(datagram) => {
                if let Some(picture) = assembler.push(&datagram).expect("no stale piece") {
                    arrived = Some(picture);
                }
            }
            Incoming::Control(_) => continue,
            Incoming::Closed => panic!("the connection closed before the picture arrived"),
        }
    }
    let arrived = arrived.expect("the picture must come back");

    // Not one byte was lost or changed on the way.
    assert_eq!(
        arrived.data, encoded.data,
        "the picture must arrive exactly as it was sent"
    );
    assert_eq!(arrived.pts, encoded.pts, "the time stamp must survive");
    assert!(arrived.keyframe, "the key frame mark must survive");

    // And it is still a picture: it decodes, and what comes out is close
    // to what went in.
    let mut decoder = openh264::decoder::Decoder::new().expect("the decoder must start");
    let decoded = decoder
        .decode(&arrived.data)
        .expect("the picture must decode")
        .expect("a frame must come out of the decoder");

    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    decoded.write_rgba8(&mut pixels);
    let decoded_image = RgbaImage::from_raw(WIDTH, HEIGHT, pixels).expect("the size must match");

    let difference = average_difference(&original, &decoded_image);
    assert!(
        difference < 12.0,
        "the picture came back too different: the average difference is {difference}"
    );
}

#[tokio::test]
async fn several_pictures_in_a_row_all_arrive() {
    // A session does not send one picture; it sends thousands.  The pieces
    // of two pictures must not be mixed up.
    let (server, server_address) = listen(0).await.expect("the port must open");
    let client_endpoint = connect(server_address).await.expect("the port must open");

    let accept_task = tokio::spawn(async move {
        accept(&server, Duration::from_secs(10))
            .await
            .expect("the connection must come in")
    });

    let client = client_endpoint
        .connect(server_address, "skycremote")
        .expect("the connection must start")
        .await
        .expect("the connection must be made");
    let server_connection = accept_task.await.expect("the task must finish");

    // The picture is sent from the client and read on the server side.
    let (client_sender, _client_receiver) = open(client, Side::Initiator)
        .await
        .expect("the channels must open");
    let (_server_sender, mut server_receiver) = open(server_connection, Side::Responder)
        .await
        .expect("the channels must open");

    let config = EncoderConfig {
        width: WIDTH,
        height: HEIGHT,
        fps: 30,
        bitrate_bps: 4_000_000,
        keyframe_interval: 60,
    };
    let mut encoder = Encoder::new(config).expect("the encoder must start");

    let mut expected = Vec::new();
    for index in 0..3u8 {
        let mut image = RgbaImage::new(WIDTH, HEIGHT);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgba([
                (x % 256) as u8,
                (y % 256) as u8,
                index.wrapping_mul(80).wrapping_add((x + y) as u8),
                255,
            ]);
        }
        let frame = Frame {
            image,
            captured_at: Instant::now(),
        };
        let encoded = encoder
            .encode(&frame)
            .expect("the picture must encode")
            .expect("a picture must come out");
        expected.push(encoded.data.clone());
        let pieces = split_frame(&encoded, index as u32 + 1).expect("splittable");
        for piece in pieces {
            client_sender
                .send_video(piece)
                .expect("the piece must go out");
        }
    }

    let mut assembler = FrameAssembler::new();
    assembler.set_picture_size(WIDTH, HEIGHT);
    let mut arrived: Vec<Vec<u8>> = Vec::new();
    while arrived.len() < expected.len() {
        match server_receiver.recv().await {
            Incoming::Video(datagram) => {
                if let Some(picture) = assembler.push(&datagram).expect("no stale piece") {
                    arrived.push(picture.data);
                }
            }
            Incoming::Control(_) => continue,
            Incoming::Closed => panic!(
                "the connection closed too early: {} pictures complete, {} pending",
                arrived.len(),
                assembler.pending_pictures()
            ),
        }
    }

    assert_eq!(arrived, expected, "every picture must arrive, in order");
}

/// A session that has been through the handshake still works: the
/// control messages arrive, and so do the pictures.
#[tokio::test]
async fn a_session_with_the_handshake_still_works() {
    use skycremote_core::handshake::{controller_handshake, host_handshake, HostIdentity};
    use skycremote_core::protocol::Message;

    let salt = b"the salt of this device".to_vec();
    let verifier = skycremote_core::crypto::code_verifier(&salt, "123456");
    let identity = HostIdentity {
        device_id: "the-device".into(),
        device_name: "The Device".into(),
        port: 21116,
        salt: salt.clone(),
        verifier,
    };

    let (server, server_address) = listen(0).await.expect("the port must open");
    let client_endpoint = connect(server_address).await.expect("the port must open");

    let accept_task = tokio::spawn(async move {
        accept(&server, Duration::from_secs(10))
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
    assert_eq!(host.verify_code, controller.verify_code);

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

    // A picture goes over the encrypted video channel.
    let picture = a_test_picture();
    let frame = Frame {
        image: picture.clone(),
        captured_at: Instant::now(),
    };
    let mut encoder =
        Encoder::new(EncoderConfig::for_screen(WIDTH, HEIGHT)).expect("the encoder must be built");
    let encoded = encoder.encode(&frame).expect("encodable").expect("a frame");
    let pieces = split_frame(&encoded, 1).expect("splittable");
    for piece in pieces {
        client_sender
            .send_video(piece)
            .expect("the piece must go out");
    }

    let mut assembler = FrameAssembler::new();
    assembler.set_picture_size(WIDTH, HEIGHT);
    let mut arrived = None;
    while arrived.is_none() {
        match server_receiver.recv().await {
            Incoming::Video(datagram) => {
                if let Some(picture) = assembler.push(&datagram).expect("no stale piece") {
                    arrived = Some(picture.data);
                }
            }
            Incoming::Control(_) => continue,
            Incoming::Closed => panic!("the connection closed too early"),
        }
    }

    assert_eq!(
        arrived.expect("the picture must come back"),
        encoded.data,
        "the picture must come back whole"
    );
}
