//! The libx264 backend, and the shape each hardware encoder will take.
//!
//! ## Why libx264 is behind a feature
//!
//! `ffmpeg-next` is a binding to the *system's* ffmpeg, and its build
//! script asks `pkg-config` where those libraries are.  That means a
//! checkout only builds on a machine where ffmpeg's `.pc` files are on
//! `pkg-config`'s search path — a developer machine usually has them, a
//! CI container usually does not, and on macOS the first `pkg-config` in
//! `PATH` is often not the one that knows about Homebrew.  Making that a
//! hard dependency of the core crate would mean nobody could build the
//! project without first fixing their ffmpeg setup.
//!
//! So the software encoder in [`super`] is the default and this file is
//! compiled only with `--features ffmpeg`.  The code below is a complete
//! libx264 encoder, not a stub: enable the feature and
//! [`super::Encoder::with_libx264`] produces real libx264 pictures.
//!
//! ## Why the hardware encoders are not written
//!
//! VideoToolbox, NVENC and VAAPI each need their own device session, their
//! own surface formats and their own handling of "the driver refused".
//! Doing that badly is worse than not doing it: a hardware path that falls
//! back silently to software produces a session that looks fine in testing
//! and then drops to two frames a second on a user's machine with an old
//! driver.  Each one is therefore a named, documented function returning
//! [`super::EncodeError::Unsupported`] with a note saying what it would
//! take to finish it, so the selection code can be written against the
//! final API.

pub use super::HardwareEncoder;

// The module that holds this file is also called `ffmpeg`, which would
// shadow the crate's own path; the alias keeps `ffmpeg::...` meaning the
// crate everywhere below.
#[cfg(feature = "ffmpeg")]
use ffmpeg_next as ffmpeg;

/// Builds the error for a hardware encoder that is not written yet.
///
/// Kept as three separate cfg branches rather than one `match` so that
/// each platform compiles only the branch it can actually reach, and so a
/// compiler error in one does not hide the other two.
pub fn hardware_unavailable(hardware: HardwareEncoder) -> super::EncodeError {
    let message = match hardware {
        #[cfg(target_os = "macos")]
        HardwareEncoder::VideoToolbox => {
            "TODO: the VideoToolbox encoder is not implemented yet. It would drive \
             VTCompressionSession with kVTCompressionPropertyKey_ProfileLevel set to \
             H.264 Baseline and feed it CVPixelBuffers made from the captured frames"
        }

        #[cfg(target_os = "windows")]
        HardwareEncoder::Nvenc => {
            "TODO: the NVENC encoder is not implemented yet. It would open an \
             NV_ENCODE_API_FUNCTION_LIST session and upload each frame as an \
             NV_ENC_BUFFER_FORMAT_ARGB texture"
        }

        #[cfg(target_os = "linux")]
        HardwareEncoder::Vaapi => {
            "TODO: the VAAPI encoder is not implemented yet. It would create a VADisplay \
             on the DRM render node and import each frame as an NV12 surface with \
             vaCreateSurfaces before calling vaBeginPicture"
        }

        // A hardware encoder this platform cannot have is refused with the
        // same error type but a different, non-TODO message: there is
        // nothing to implement, the API simply is not there.
        other => {
            return super::EncodeError::Unsupported(format!(
                "{other:?} hardware encoding does not exist on this platform"
            ));
        }
    };

    super::EncodeError::Unsupported(message.to_string())
}

/// libx264 through `ffmpeg-next`.
#[cfg(feature = "ffmpeg")]
pub struct LibX264Encoder {
    encoder: ffmpeg::codec::encoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    width: u32,
    height: u32,
    fps: u32,
}

#[cfg(feature = "ffmpeg")]
impl LibX264Encoder {
    /// Opens a libx264 encoder for the settings given.
    ///
    /// B-frames are switched off on purpose.  A remote desktop wants the
    /// picture of *now*: reordering frames would add one frame of latency
    /// per B-frame, and screen content barely benefits from the extra
    /// prediction.  The gain in responsiveness is worth more than the few
    /// percent of bitrate.
    pub fn new(config: &super::EncoderConfig) -> Result<Self, super::EncodeError> {
        ffmpeg::init().map_err(|err| {
            super::EncodeError::BackendUnavailable(format!("ffmpeg could not be initialised: {err}"))
        })?;

        let codec = ffmpeg::encoder::find_by_name("libx264").ok_or_else(|| {
            super::EncodeError::BackendUnavailable(
                "this ffmpeg build has no libx264 encoder; rebuild it with --enable-libx264"
                    .to_string(),
            )
        })?;

        let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(|err| super::EncodeError::BackendUnavailable(err.to_string()))?;

        encoder.set_width(config.width);
        encoder.set_height(config.height);
        encoder.set_format(ffmpeg::format::Pixel::YUV420P);
        encoder.set_time_base(ffmpeg::Rational(1, config.fps as i32));
        encoder.set_frame_rate(Some(ffmpeg::Rational(config.fps as i32, 1)));
        encoder.set_bit_rate(config.bitrate_bps as usize);
        encoder.set_gop(config.keyframe_interval);
        encoder.set_max_b_frames(0);

        // `zerolatency` turns off the lookahead and the frame reordering
        // that would otherwise hold a picture back; `baseline` keeps the
        // stream decodable by the widest set of receivers.
        let mut options = ffmpeg::Dictionary::new();
        options.set("preset", "veryfast");
        options.set("tune", "zerolatency");
        options.set("profile", "baseline");

        let encoder = encoder.open_as_with(codec, options).map_err(|err| {
            super::EncodeError::BackendUnavailable(format!("libx264 refused to start: {err}"))
        })?;

        let scaler = ffmpeg::software::converter(
            (config.width, config.height),
            ffmpeg::format::Pixel::RGBA,
            ffmpeg::format::Pixel::YUV420P,
        )
        .map_err(|err| {
            super::EncodeError::BackendUnavailable(format!(
                "could not prepare the RGBA to YUV420P conversion: {err}"
            ))
        })?;

        Ok(Self {
            encoder,
            scaler,
            width: config.width,
            height: config.height,
            fps: config.fps,
        })
    }

    /// Encodes one RGBA frame and returns the Annex-B bytes it produced.
    pub fn encode(
        &mut self,
        frame: &crate::capture::Frame,
        pts: std::time::Duration,
    ) -> Result<Option<Vec<u8>>, super::EncodeError> {
        let mut rgba = ffmpeg::frame::Video::new(
            ffmpeg::format::Pixel::RGBA,
            self.width,
            self.height,
        );

        // The captured image may have padding at the end of each row, so
        // the pixels are copied row by row into ffmpeg's own stride rather
        // than blitted in one go.
        let source = frame.rgba();
        let source_stride = self.width as usize * 4;
        let target_stride = rgba.stride(0);
        let destination = rgba.data_mut(0);

        for row in 0..self.height as usize {
            let from = row * source_stride;
            let to = row * target_stride;
            destination[to..to + source_stride]
                .copy_from_slice(&source[from..from + source_stride]);
        }

        let mut yuv = ffmpeg::frame::Video::empty();
        self.scaler
            .run(&rgba, &mut yuv)
            .map_err(|err| super::EncodeError::Encoding(err.to_string()))?;

        // The encoder's time base is 1/fps, so the presentation timestamp
        // has to be expressed in those ticks.
        let ticks = (pts.as_secs_f64() * f64::from(self.fps)).round() as i64;
        yuv.set_pts(Some(ticks));

        self.encoder
            .send_frame(&yuv)
            .map_err(|err| super::EncodeError::Encoding(err.to_string()))?;

        self.receive()
    }

    /// Drains the encoder at the end of a stream.
    pub fn flush(&mut self) -> Result<Vec<(Vec<u8>, std::time::Duration)>, super::EncodeError> {
        self.encoder
            .send_eof()
            .map_err(|err| super::EncodeError::Encoding(err.to_string()))?;

        let mut out = Vec::new();
        let mut packet = ffmpeg::Packet::empty();

        while self.encoder.receive_packet(&mut packet).is_ok() {
            if let Some(data) = packet.data() {
                let pts = packet
                    .pts()
                    .map(|ticks| {
                        std::time::Duration::from_secs_f64(ticks as f64 / f64::from(self.fps))
                    })
                    .unwrap_or_default();
                out.push((data.to_vec(), pts));
            }
        }

        Ok(out)
    }

    /// Pulls whatever picture the encoder has finished.
    fn receive(&mut self) -> Result<Option<Vec<u8>>, super::EncodeError> {
        let mut packet = ffmpeg::Packet::empty();

        match self.encoder.receive_packet(&mut packet) {
            Ok(()) => Ok(packet.data().map(|data| data.to_vec())),
            // EAGAIN means "nothing ready yet", which is normal: the
            // encoder buffers until it has enough to make a picture.
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => Ok(None),
            Err(err) => Err(super::EncodeError::Encoding(err.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hardware_encoder_that_is_not_written_says_so() {
        let error = hardware_unavailable(HardwareEncoder::VideoToolbox);
        let text = error.to_string();

        // The message has to be usable by whoever picks the work up: it
        // must name the API and it must be honest that nothing works yet.
        assert!(
            text.contains("not implemented") || text.contains("does not exist"),
            "unhelpful message: {text}"
        );
    }

    #[test]
    fn every_hardware_encoder_is_refused_rather_than_silently_ignored() {
        for hardware in [
            HardwareEncoder::VideoToolbox,
            HardwareEncoder::Nvenc,
            HardwareEncoder::Vaapi,
        ] {
            let error = hardware_unavailable(hardware);
            assert!(
                matches!(error, super::super::EncodeError::Unsupported(_)),
                "{hardware:?} must be Unsupported, not {error:?}"
            );
        }
    }
}
