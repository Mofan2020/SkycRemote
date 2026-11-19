//! Tests for the encode module.
//!
//! These run against the software backend, which is the one that is always
//! compiled.  They check the things that would silently corrupt a stream —
//! a bad configuration accepted, a frame of the wrong size encoded anyway,
//! a timestamp that goes backwards — rather than asserting exact bytes,
//! which would only be testing the C library.

use std::time::{Duration, Instant};

use image::{Rgba, RgbaImage};

use super::*;

/// A frame of the given size, filled with something that is not uniform so
/// the encoder has real work to do and does not just skip it.
fn test_frame(width: u32, height: u32) -> Frame {
    let mut image = RgbaImage::new(width, height);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = Rgba([
            (x % 256) as u8,
            (y % 256) as u8,
            ((x + y) % 256) as u8,
            255,
        ]);
    }

    Frame {
        image,
        captured_at: Instant::now(),
    }
}

#[test]
fn odd_dimensions_are_refused_because_h264_cannot_represent_them() {
    // 4:2:0 chroma is half resolution in both directions.  Accepting an odd
    // size and rounding it would make the encoded picture a different size
    // from the frames the caller keeps feeding in.
    let config = EncoderConfig {
        width: 641,
        height: 480,
        ..EncoderConfig::for_screen(641, 480)
    };

    match config.validate() {
        Err(EncodeError::InvalidSettings(message)) => {
            assert!(message.contains("even"), "unhelpful message: {message}");
        }
        other => panic!("an odd width must be refused, got {other:?}"),
    }
}

#[test]
fn a_zero_sized_or_zero_rate_configuration_is_refused() {
    let no_size = EncoderConfig {
        width: 0,
        height: 0,
        ..EncoderConfig::for_screen(0, 0)
    };
    assert!(matches!(
        no_size.validate(),
        Err(EncodeError::InvalidSettings(_))
    ));

    let no_rate = EncoderConfig {
        fps: 0,
        ..EncoderConfig::for_screen(64, 48)
    };
    assert!(matches!(
        no_rate.validate(),
        Err(EncodeError::InvalidSettings(_))
    ));

    let no_bits = EncoderConfig {
        bitrate_bps: 0,
        ..EncoderConfig::for_screen(64, 48)
    };
    assert!(matches!(
        no_bits.validate(),
        Err(EncodeError::InvalidSettings(_))
    ));
}

#[test]
fn a_sane_configuration_is_accepted() {
    assert!(EncoderConfig::for_screen(1920, 1080).validate().is_ok());
    // A small picture is still a picture.
    assert!(EncoderConfig::for_screen(16, 16).validate().is_ok());
}

#[test]
fn the_first_picture_is_a_keyframe_in_annex_b() {
    let encoder_config = EncoderConfig {
        width: 64,
        height: 48,
        fps: 30,
        bitrate_bps: 500_000,
        keyframe_interval: 30,
    };

    let mut encoder = Encoder::new(encoder_config).expect("the software encoder must start");
    assert_eq!(encoder.backend(), Backend::Software);

    let frame = test_frame(64, 48);
    let encoded = encoder
        .encode(&frame)
        .expect("encoding the first frame must not fail")
        .expect("the first frame of a stream is never skipped");

    // Annex-B: the stream must open with a start code.
    assert!(
        encoded.data.starts_with(&[0, 0, 0, 1]) || encoded.data.starts_with(&[0, 0, 1]),
        "the stream does not begin with an Annex-B start code: {:02x?}",
        &encoded.data[..8.min(encoded.data.len())]
    );

    assert!(
        encoded.keyframe,
        "the first picture has to be decodable on its own"
    );
    assert_eq!(encoded.width, 64);
    assert_eq!(encoded.height, 48);
}

#[test]
fn a_frame_of_the_wrong_size_is_refused_instead_of_corrupting_the_stream() {
    let mut encoder = Encoder::new(EncoderConfig::for_screen(64, 48)).expect("encoder");

    let wrong = test_frame(63, 47);
    match encoder.encode(&wrong) {
        Err(EncodeError::FrameSizeMismatch {
            expected_width,
            expected_height,
            actual_width,
            actual_height,
        }) => {
            assert_eq!((expected_width, expected_height), (64, 48));
            assert_eq!((actual_width, actual_height), (63, 47));
        }
        other => panic!("a resized frame must be refused, got {other:?}"),
    }
}

#[test]
fn timestamps_never_go_backwards() {
    // Two frames captured inside the same instant, and then one whose
    // timestamp appears to be earlier still (a clock that stepped back).
    // The encoder must not hand the decoder a timestamp that is not
    // greater than the last one, because some decoders treat that as a
    // discontinuity and drop everything until the next keyframe.
    let start = Instant::now();
    let mut encoder = Encoder::new(EncoderConfig {
        width: 32,
        height: 32,
        fps: 30,
        bitrate_bps: 200_000,
        keyframe_interval: 15,
    })
    .expect("encoder");

    let mut last: Option<Duration> = None;
    for _ in 0..4 {
        let frame = Frame {
            image: RgbaImage::from_pixel(32, 32, Rgba([10, 20, 30, 255])),
            // Deliberately the same instant every time.
            captured_at: start,
        };

        if let Some(encoded) = encoder.encode(&frame).expect("encode") {
            if let Some(previous) = last {
                assert!(
                    encoded.pts > previous,
                    "timestamp went backwards: {previous:?} then {:?}",
                    encoded.pts
                );
            }
            last = Some(encoded.pts);
        }
    }

    assert!(last.is_some() || encoder.flush().expect("flush").is_empty());
}

#[test]
fn a_hardware_encoder_is_refused_with_an_explanation() {
    let result = Encoder::with_hardware(
        HardwareEncoder::Vaapi,
        EncoderConfig::for_screen(64, 48),
    );

    match result {
        Err(EncodeError::Unsupported(message)) => {
            assert!(
                message.contains("not implemented") || message.contains("does not exist"),
                "the refusal has to say why: {message}"
            );
        }
        other => panic!("a hardware encoder must not silently fall back: {other:?}"),
    }
}

#[test]
fn flushing_a_software_stream_produces_nothing_left_to_send() {
    // The software backend does not hold pictures back, so a flush must be
    // empty rather than duplicating the last frame.
    let mut encoder = Encoder::new(EncoderConfig::for_screen(32, 32)).expect("encoder");
    let _ = encoder.encode(&test_frame(32, 32)).expect("encode");

    let flushed = encoder.flush().expect("flush");
    assert!(
        flushed.is_empty(),
        "the software backend buffered {} unexpected picture(s)",
        flushed.len()
    );
}

#[test]
fn the_platform_hardware_encoder_is_the_one_the_platform_can_have() {
    // Advertising a backend the machine cannot have only produces a failed
    // connection later, so the answer has to match the platform.
    let native = HardwareEncoder::for_this_platform();

    #[cfg(target_os = "macos")]
    assert_eq!(native, Some(HardwareEncoder::VideoToolbox));

    #[cfg(target_os = "windows")]
    assert_eq!(native, Some(HardwareEncoder::Nvenc));

    #[cfg(target_os = "linux")]
    assert_eq!(native, Some(HardwareEncoder::Vaapi));
}
