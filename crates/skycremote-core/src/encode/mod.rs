//! Turning captured frames into H.264.
//!
//! The wire format is **Annex-B**: every NAL unit is preceded by a
//! `00 00 01` or `00 00 00 01` start code.  Annex-B was chosen over AVCC
//! because the receiver has to be able to start decoding at any keyframe
//! it happens to join on, and in Annex-B the parameter sets and the slices
//! are all self-delimiting — there is no separate length prefix table to
//! keep in sync, and no out-of-band extradata blob to lose across a
//! reconnect.  The cost is a handful of extra bytes per frame, which is
//! nothing next to a screen's worth of picture.
//!
//! ## Backends
//!
//! [`Encoder::new`] uses a self-contained software encoder (Cisco's
//! OpenH264, compiled from vendored C source).  It has no system
//! dependencies, so `cargo build` works on a machine that has never heard
//! of ffmpeg.  [`Encoder::with_libx264`] uses libx264 through `ffmpeg-next`
//! and is compiled only with the `ffmpeg` feature, because `ffmpeg-next`
//! needs the system's ffmpeg development files to be findable by
//! `pkg-config` at build time.
//!
//! Hardware encoders are listed in [`HardwareEncoder`] but not implemented;
//! see the note on that type.

pub mod ffmpeg;

#[cfg(test)]
mod tests;

use std::time::Duration;

use thiserror::Error;

use crate::capture::Frame;

/// Everything that can go wrong while encoding.
#[derive(Debug, Error)]
pub enum EncodeError {
    /// The requested settings cannot describe a video.  This is a
    /// programming error rather than a runtime condition, so it carries a
    /// sentence that says which field was wrong.
    #[error("invalid encoder settings: {0}")]
    InvalidSettings(String),

    /// The backend refused to start.  Distinguished from
    /// [`EncodeError::InvalidSettings`] because the settings may be fine
    /// and the machine may simply lack the codec.
    #[error("the encoder could not be started: {0}")]
    BackendUnavailable(String),

    /// A frame arrived that does not match the size the encoder was
    /// configured for.  Encoding it anyway would corrupt the stream, so
    /// it is refused and the caller is expected to rebuild the encoder
    /// when the screen resolution changes.
    #[error("the frame is {actual_width}x{actual_height} but the encoder was configured for {expected_width}x{expected_height}")]
    FrameSizeMismatch {
        /// The size the encoder was built for.
        expected_width: u32,
        /// The size the encoder was built for.
        expected_height: u32,
        /// The size of the frame that arrived.
        actual_width: u32,
        /// The size of the frame that arrived.
        actual_height: u32,
    },

    /// The backend failed mid-stream.  A retry with a freshly built
    /// encoder is the usual remedy.
    #[error("the encoder failed: {0}")]
    Encoding(String),

    /// This build or this machine cannot do what was asked — an
    /// unimplemented hardware path, or a feature that was not compiled in.
    #[error("{0}")]
    Unsupported(String),
}

/// The settings a stream is encoded with.
///
/// These are fixed for the life of one [`Encoder`]; changing resolution or
/// bitrate means building a new one.  Reconfiguring in place would let the
/// encoder and the decoder disagree about the picture size for a frame or
/// two, which is exactly when a viewer shows garbage.
#[derive(Debug, Clone, Copy)]
pub struct EncoderConfig {
    /// Picture width in pixels.  Must be even: H.264 4:2:0 stores colour
    /// at half resolution in both directions, so an odd dimension cannot
    /// be represented.
    pub width: u32,
    /// Picture height in pixels.  Must be even, for the same reason.
    pub height: u32,
    /// Target frames per second.  Used for rate control, not for pacing —
    /// [`crate::capture::FramePacer`] is what decides which frames to
    /// spend bits on.
    pub fps: u32,
    /// Target bitrate in bits per second.
    pub bitrate_bps: u32,
    /// How often to force a keyframe, in frames.  A viewer that joins or
    /// loses packets waits at most this long for a fresh one.
    pub keyframe_interval: u32,
}

impl EncoderConfig {
    /// A configuration with settings that suit a desktop at 1080p.
    ///
    /// Screen content compresses well and changes little between frames,
    /// so 2 Mbit/s at 30 fps is a reasonable default; a caller with a
    /// different link should override the bitrate.
    pub fn for_screen(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            fps: 30,
            bitrate_bps: 2_000_000,
            keyframe_interval: 60,
        }
    }

    /// Checks that these settings describe a video that can exist.
    ///
    /// Called before any backend is started so that a bad configuration
    /// produces one clear sentence rather than whatever cryptic complaint
    /// the C library would have made.
    pub fn validate(&self) -> Result<(), EncodeError> {
        if self.width == 0 || self.height == 0 {
            return Err(EncodeError::InvalidSettings(format!(
                "the picture is {}x{}; both dimensions must be at least 1",
                self.width, self.height
            )));
        }

        // 4:2:0 chroma is stored at half resolution in each direction, so
        // an odd dimension has no valid representation.  Rounding silently
        // would make the encoded picture a different size from the frames
        // the caller keeps feeding in.
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(EncodeError::InvalidSettings(format!(
                "the picture is {}x{}; H.264 4:2:0 needs both dimensions even",
                self.width, self.height
            )));
        }

        if self.fps == 0 {
            return Err(EncodeError::InvalidSettings(
                "the frame rate must be at least 1".to_string(),
            ));
        }

        if self.bitrate_bps == 0 {
            return Err(EncodeError::InvalidSettings(
                "the bitrate must be at least 1 bit per second".to_string(),
            ));
        }

        if self.keyframe_interval == 0 {
            return Err(EncodeError::InvalidSettings(
                "the keyframe interval must be at least 1 frame".to_string(),
            ));
        }

        Ok(())
    }
}

/// One encoded picture, ready to be sent.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// Annex-B bytes: start-code delimited NAL units, concatenated.
    pub data: Vec<u8>,
    /// When this picture should be shown, measured from the start of the
    /// stream.  It is the capture timestamp, not the time it was encoded,
    /// so a slow encoder does not make the video drift late.
    pub pts: Duration,
    /// Whether this picture can be decoded on its own.  The sender marks
    /// these so a receiver that just joined knows where it may start.
    pub keyframe: bool,
    /// The picture width, repeated here so a receiver can size its surface
    /// from any single frame rather than waiting for a second one.
    pub width: u32,
    /// The picture height.
    pub height: u32,
}

/// A hardware H.264 encoder this project intends to support.
///
/// Every one of these is a declared intention, not a working encoder:
/// asking for one returns [`EncodeError::Unsupported`] and no pixels are
/// produced.  They are named here so the selection logic, the settings
/// UI and the peer capability negotiation can be written once and stay
/// correct as each is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareEncoder {
    /// Apple's VideoToolbox, on macOS.
    VideoToolbox,
    /// NVIDIA's NVENC, on Windows and Linux.
    Nvenc,
    /// The Video Acceleration API, on Linux.
    Vaapi,
}

impl HardwareEncoder {
    /// The hardware encoder that is native to the current platform, if any.
    ///
    /// This is what a capability negotiation should advertise: offering a
    /// backend the machine cannot have only produces a failed connection
    /// later.
    pub fn for_this_platform() -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            Some(Self::VideoToolbox)
        }

        #[cfg(target_os = "windows")]
        {
            Some(Self::Nvenc)
        }

        #[cfg(target_os = "linux")]
        {
            Some(Self::Vaapi)
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            None
        }
    }
}

/// The encoding backend a session is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The self-contained software encoder.
    Software,
    /// libx264 through `ffmpeg-next`; only in builds with the `ffmpeg`
    /// feature.
    LibX264,
}

/// Encodes captured frames into an H.264 Annex-B stream.
///
/// One encoder produces one stream at one resolution.  Feed it frames in
/// capture order; it returns zero or one pictures per frame (an encoder
/// may buffer or skip), each stamped with the time its source frame was
/// captured.
pub struct Encoder {
    /// The concrete implementation behind this handle.
    inner: Inner,
    /// The settings this encoder was built for, kept so frames of the
    /// wrong size can be refused with a useful message.
    config: EncoderConfig,
    /// How many frames the backend produced so far, used to keep
    /// timestamps monotonic when the caller's clock jumps backwards.
    frames_emitted: u64,
    /// The timestamp of the last picture, so a duplicate or backwards
    /// timestamp can be corrected instead of confusing the decoder.
    last_pts: Option<Duration>,
}

enum Inner {
    /// OpenH264, compiled from vendored C; always available.
    Software(software::SoftwareEncoder),
    /// libx264 via ffmpeg-next; present only with the `ffmpeg` feature.
    #[cfg(feature = "ffmpeg")]
    LibX264(ffmpeg::LibX264Encoder),
}

// The encoder handle itself is not useful to print -- the C library behind
// it has no printable form -- but the settings are, and they are what a
// failing test wants to see.  So the debug form carries the settings and
// the counters and leaves the backend out.
impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("config", &self.config)
            .field("frames_emitted", &self.frames_emitted)
            .field("last_pts", &self.last_pts)
            .finish()
    }
}

impl Encoder {
    /// Builds an encoder with the settings given.
    ///
    /// Uses the self-contained software backend so that a fresh checkout
    /// builds and runs on a machine with no ffmpeg installed.  Call
    /// [`Encoder::with_libx264`] to get libx264 instead.
    pub fn new(config: EncoderConfig) -> Result<Self, EncodeError> {
        config.validate()?;

        let software = software::SoftwareEncoder::new(&config)?;

        Ok(Self {
            inner: Inner::Software(software),
            config,
            frames_emitted: 0,
            last_pts: None,
        })
    }

    /// Builds an encoder backed by libx264 through `ffmpeg-next`.
    ///
    /// libx264 costs fewer bits than OpenH264 for the same picture, which
    /// matters on a thin link, but it needs the system's ffmpeg libraries,
    /// so it is opt-in at build time.
    #[cfg(feature = "ffmpeg")]
    pub fn with_libx264(config: EncoderConfig) -> Result<Self, EncodeError> {
        config.validate()?;

        let backend = ffmpeg::LibX264Encoder::new(&config)?;

        Ok(Self {
            inner: Inner::LibX264(backend),
            config,
            frames_emitted: 0,
            last_pts: None,
        })
    }

    /// Asks for a hardware encoder.
    ///
    /// Always fails today: no hardware path is implemented yet.  It exists
    /// so that the call sites and the capability negotiation can be
    /// written against the final shape of the API.
    pub fn with_hardware(
        hardware: HardwareEncoder,
        config: EncoderConfig,
    ) -> Result<Self, EncodeError> {
        config.validate()?;

        Err(ffmpeg::hardware_unavailable(hardware))
    }

    /// The settings this encoder runs with.
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Which backend is behind this handle.
    pub fn backend(&self) -> Backend {
        match self.inner {
            Inner::Software(_) => Backend::Software,
            #[cfg(feature = "ffmpeg")]
            Inner::LibX264(_) => Backend::LibX264,
        }
    }

    /// Encodes one captured frame.
    ///
    /// Returns `None` when the backend decided to skip this frame — an
    /// encoder under a tight bitrate budget drops pictures rather than
    /// exceeding it.  A skipped frame is not an error: the next one will
    /// carry the newer picture.
    ///
    /// The timestamp comes from the frame's own capture time, so pictures
    /// stay in step with the remote clock even when encoding is slow.
    pub fn encode(&mut self, frame: &Frame) -> Result<Option<EncodedFrame>, EncodeError> {
        if frame.width() != self.config.width || frame.height() != self.config.height {
            return Err(EncodeError::FrameSizeMismatch {
                expected_width: self.config.width,
                expected_height: self.config.height,
                actual_width: frame.width(),
                actual_height: frame.height(),
            });
        }

        let pts = self.stamp(frame);

        let produced = match &mut self.inner {
            Inner::Software(encoder) => encoder.encode(frame, pts)?,
            #[cfg(feature = "ffmpeg")]
            Inner::LibX264(encoder) => encoder.encode(frame, pts)?,
        };

        let Some(data) = produced else {
            return Ok(None);
        };

        if data.is_empty() {
            return Ok(None);
        }

        let keyframe = starts_with_keyframe_nals(&data);

        self.frames_emitted += 1;

        Ok(Some(EncodedFrame {
            data,
            pts,
            keyframe,
            width: self.config.width,
            height: self.config.height,
        }))
    }

    /// Pushes any picture the backend is still holding out.
    ///
    /// Encoders buffer a frame or two to look ahead.  At the end of a
    /// session those pictures would otherwise be lost, so they are flushed
    /// here.  Calling it more than once is harmless.
    pub fn flush(&mut self) -> Result<Vec<EncodedFrame>, EncodeError> {
        let width = self.config.width;
        let height = self.config.height;

        let pictures = match &mut self.inner {
            Inner::Software(encoder) => encoder.flush()?,
            #[cfg(feature = "ffmpeg")]
            Inner::LibX264(encoder) => encoder.flush()?,
        };

        let mut out = Vec::with_capacity(pictures.len());
        for (data, pts) in pictures {
            if data.is_empty() {
                continue;
            }
            let keyframe = starts_with_keyframe_nals(&data);
            out.push(EncodedFrame {
                data,
                pts,
                keyframe,
                width,
                height,
            });
        }

        Ok(out)
    }

    /// The timestamp to stamp on the picture made from `frame`.
    ///
    /// The capture time is used, but it is held monotonic: a wall clock
    /// that steps backwards, or two frames captured inside the same
    /// instant, would otherwise hand the encoder a timestamp that is not
    /// greater than the last one and some decoders treat that as a
    /// discontinuity and drop everything until the next keyframe.
    fn stamp(&mut self, frame: &Frame) -> Duration {
        let captured = frame.captured_at.elapsed();

        let interval = Duration::from_secs_f64(1.0 / f64::from(self.config.fps.max(1)));

        let pts = match self.last_pts {
            Some(previous) if captured <= previous => previous + interval,
            _ => captured,
        };

        self.last_pts = Some(pts);
        pts
    }
}

/// Whether a chunk of Annex-B bytes begins with a keyframe.
///
/// A keyframe is an IDR or a plain I slice; the NAL unit type is the low
/// five bits of the byte after the start code.  Looking only at the first
/// NAL is enough because an encoder always emits the slice type it starts
/// with, and this is a hint for the sender, not a decoder.
fn starts_with_keyframe_nals(data: &[u8]) -> bool {
    let Some(payload) = skip_start_code(data) else {
        return false;
    };

    let Some(&header) = payload.first() else {
        return false;
    };

    let nal_type = header & 0x1f;

    // 5 is an IDR slice, 7 is SPS and 8 is PPS.  A stream that opens with
    // parameter sets always has them ahead of an IDR, so seeing either
    // means a decoder can start here.
    matches!(nal_type, 5 | 7 | 8)
}

/// Returns the bytes after a leading Annex-B start code, if there is one.
fn skip_start_code(data: &[u8]) -> Option<&[u8]> {
    if data.starts_with(&[0, 0, 0, 1]) {
        Some(&data[4..])
    } else if data.starts_with(&[0, 0, 1]) {
        Some(&data[3..])
    } else {
        None
    }
}

/// The software backend, and the RGBA-to-YUV conversion it needs.
mod software {
    use super::{EncoderConfig, EncodeError};
    use crate::capture::Frame;

    use openh264::encoder::{
        BitRate, Encoder as OpenH264Encoder, EncoderConfig as OpenH264Config, FrameRate,
        IntraFramePeriod, RateControlMode, UsageType,
    };
    use openh264::formats::{RgbaSliceU8, YUVBuffer};
    use openh264::OpenH264API;

    /// A thin wrapper that gives OpenH264 the settings this module's
    /// configuration describes and hands back plain bytes.
    pub(super) struct SoftwareEncoder {
        encoder: OpenH264Encoder,
        width: usize,
        height: usize,
    }

    impl SoftwareEncoder {
        pub(super) fn new(config: &EncoderConfig) -> Result<Self, EncodeError> {
            let native = OpenH264Config::new()
                .bitrate(BitRate::from_bps(config.bitrate_bps))
                .max_frame_rate(FrameRate::from_hz(config.fps as f32))
                // Screen content, not a camera: this tunes the encoder's
                // motion search for large static areas with sharp text,
                // which is what a desktop actually looks like.
                .usage_type(UsageType::ScreenContentRealTime)
                .rate_control_mode(RateControlMode::Bitrate)
                .intra_frame_period(IntraFramePeriod::from_num_frames(
                    config.keyframe_interval,
                ));

            let encoder = OpenH264Encoder::with_api_config(OpenH264API::from_source(), native)
                .map_err(|err| EncodeError::BackendUnavailable(err.to_string()))?;

            Ok(Self {
                encoder,
                width: config.width as usize,
                height: config.height as usize,
            })
        }

        pub(super) fn encode(
            &mut self,
            frame: &Frame,
            _pts: std::time::Duration,
        ) -> Result<Option<Vec<u8>>, EncodeError> {
            let rgba = RgbaSliceU8::new(frame.rgba(), (self.width, self.height));
            let yuv = YUVBuffer::from_rgba8_source(rgba);

            let bitstream = self
                .encoder
                .encode(&yuv)
                .map_err(|err| EncodeError::Encoding(err.to_string()))?;

            let bytes = bitstream.to_vec();

            // OpenH264 reports a skipped frame as an empty bitstream.
            if bytes.is_empty() {
                return Ok(None);
            }

            Ok(Some(bytes))
        }

        pub(super) fn flush(&mut self) -> Result<Vec<(Vec<u8>, std::time::Duration)>, EncodeError> {
            // OpenH264 does not hold pictures back: every `encode` call
            // either produces a picture or reports that it skipped one, so
            // there is nothing left in a buffer to push out.
            Ok(Vec::new())
        }
    }
}

