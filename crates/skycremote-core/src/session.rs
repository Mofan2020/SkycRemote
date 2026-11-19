//! One picture, cut into pieces small enough to go into a datagram, and
//! put back together on the other side.
//!
//! An H.264 keyframe of a desktop at 1080p is often 100 KB or more, and a
//! datagram carries about 1.2 KB.  So every picture is cut into pieces and
//! every piece carries a small header saying which picture it belongs to,
//! how many pieces the picture has, and where this piece sits.
//!
//! The pieces are not reliable.  A piece that is lost is simply lost: the
//! picture it belonged to is thrown away when the next picture starts
//! arriving, and the viewer shows the next one.  That is the right trade
//! for a desktop session -- a picture that arrives late is a picture of
//! something that has already changed.

use crate::encode::EncodedFrame;
use bytes::{BufMut, Bytes, BytesMut};
use std::collections::HashMap;
use std::time::Duration;

/// How much room a piece has for the picture, in bytes.
///
/// A datagram can hold about 1200 bytes before the IP, UDP and QUIC
/// headers push it past what a network will carry without fragmenting,
/// and the header takes the rest.
pub const PIECE_PAYLOAD: usize = 1150;

/// The mark that starts every header, so a stray datagram is recognised
/// and dropped instead of being read as a picture.
const MAGIC: [u8; 2] = [0x53, 0x43];

/// The flag that says this picture can be decoded on its own.
const FLAG_KEYFRAME: u8 = 0x01;

/// Size of the header in front of every piece.
pub const HEADER_SIZE: usize = 15;

/// The header in front of one piece of a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PieceHeader {
    /// Which picture this piece belongs to.  It goes up by one for every
    /// picture, so a piece of an older picture is easy to tell apart.
    pub frame_id: u32,
    /// How many pieces the picture has.
    pub total: u16,
    /// Where this piece sits, counting from zero.
    pub index: u16,
    /// When the picture should be shown, in milliseconds from the start
    /// of the stream.
    pub pts_ms: u32,
    /// Whether the picture can be decoded on its own.
    pub keyframe: bool,
}

impl PieceHeader {
    /// Write the header into bytes.
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[0..2].copy_from_slice(&MAGIC);
        out[2..6].copy_from_slice(&self.frame_id.to_be_bytes());
        out[6..8].copy_from_slice(&self.total.to_be_bytes());
        out[8..10].copy_from_slice(&self.index.to_be_bytes());
        out[10..14].copy_from_slice(&self.pts_ms.to_be_bytes());
        out[14] = if self.keyframe { FLAG_KEYFRAME } else { 0 };
        out
    }

    /// Read a header back.
    ///
    /// `None` means this is not one of ours, and the caller drops it.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_SIZE {
            return None;
        }
        if bytes[0..2] != MAGIC {
            return None;
        }
        Some(Self {
            frame_id: u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
            total: u16::from_be_bytes([bytes[6], bytes[7]]),
            index: u16::from_be_bytes([bytes[8], bytes[9]]),
            pts_ms: u32::from_be_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]),
            keyframe: bytes[14] & FLAG_KEYFRAME != 0,
        })
    }
}

/// The error of cutting a picture up.
#[derive(Debug, thiserror::Error)]
pub enum PieceError {
    /// The picture has more pieces than the header can count.
    #[error("the picture needs {needed} pieces, but a header can only count to {limit}")]
    TooManyPieces {
        /// How many pieces were needed.
        needed: usize,
        /// The most a header can count to.
        limit: u16,
    },

    /// The picture is empty.
    #[error("the picture has no bytes")]
    Empty,
}

/// Cut one encoded picture into pieces that fit into a datagram.
///
/// The pieces come back in order, each with its header in front, ready to
/// be handed to the transport one by one.
pub fn split_frame(frame: &EncodedFrame, frame_id: u32) -> Result<Vec<Bytes>, PieceError> {
    if frame.data.is_empty() {
        return Err(PieceError::Empty);
    }
    let total = frame.data.len().div_ceil(PIECE_PAYLOAD);
    if total > u16::MAX as usize {
        return Err(PieceError::TooManyPieces {
            needed: total,
            limit: u16::MAX,
        });
    }

    let pts_ms = frame.pts.as_millis().min(u32::MAX as u128) as u32;
    let mut pieces = Vec::with_capacity(total);

    for (index, chunk) in frame.data.chunks(PIECE_PAYLOAD).enumerate() {
        let header = PieceHeader {
            frame_id,
            total: total as u16,
            index: index as u16,
            pts_ms,
            keyframe: frame.keyframe,
        };
        let mut out = BytesMut::with_capacity(HEADER_SIZE + chunk.len());
        out.put_slice(&header.encode());
        out.put_slice(chunk);
        pieces.push(out.freeze());
    }

    Ok(pieces)
}

/// The error of putting a picture back together.
#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    /// A piece arrived for a picture that has already been given up on.
    #[error("the piece belongs to picture {0}, which was given up on")]
    StalePicture(u32),
}

/// One picture that is still waiting for pieces.
struct Pending {
    /// The pieces that have arrived, by their index.
    pieces: HashMap<u16, Bytes>,
    /// How many pieces the picture has.
    total: u16,
    /// When the picture should be shown.
    pts: Duration,
    /// Whether the picture can be decoded on its own.
    keyframe: bool,
    /// The width, taken from the first piece's picture.
    width: u32,
    /// The height.
    height: u32,
}

/// Puts pictures back together from the pieces that arrive.
///
/// Only one picture is assembled at a time.  A desktop changes constantly,
/// so a picture that is still missing pieces when the next one starts
/// arriving is out of date; waiting for it would make the viewer fall
/// further and further behind.
pub struct FrameAssembler {
    /// The picture being assembled, if any.
    pending: Option<(u32, Pending)>,
    /// The highest picture number seen, so a piece of an older picture can
    /// be recognised and dropped.
    newest: Option<u32>,
    /// How many pictures were thrown away because a piece never came.
    dropped: u64,
    /// The size of the picture the sender is producing, learned from the
    /// frames that arrive.
    width: u32,
    /// The height of that picture.
    height: u32,
}

impl Default for FrameAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameAssembler {
    /// A new assembler that has not seen a picture yet.
    pub fn new() -> Self {
        Self {
            pending: None,
            newest: None,
            dropped: 0,
            width: 0,
            height: 0,
        }
    }

    /// Feed one datagram in.
    ///
    /// Returns the picture when this datagram was the last piece it was
    /// waiting for, and `None` otherwise.  A datagram that is not one of
    /// ours, or that belongs to a picture older than the one being
    /// assembled, is ignored.
    pub fn push(&mut self, datagram: &[u8]) -> Result<Option<EncodedFrame>, AssembleError> {
        let Some(header) = PieceHeader::decode(datagram) else {
            return Ok(None);
        };
        if header.total == 0 || header.index >= header.total {
            return Ok(None);
        }

        // A piece of a picture older than the one being assembled is of no
        // use: the viewer has already moved on.
        if let Some(newest) = self.newest {
            if header.frame_id < newest {
                return Err(AssembleError::StalePicture(header.frame_id));
            }
        }

        let starting_new = match &self.pending {
            Some((id, _)) => *id != header.frame_id,
            None => true,
        };

        if starting_new {
            // The picture that was being assembled will never be finished.
            if self.pending.is_some() {
                self.dropped += 1;
            }
            self.newest = Some(header.frame_id);
            self.pending = Some((
                header.frame_id,
                Pending {
                    pieces: HashMap::with_capacity(header.total as usize),
                    total: header.total,
                    pts: Duration::from_millis(header.pts_ms as u64),
                    keyframe: header.keyframe,
                    width: self.width,
                    height: self.height,
                },
            ));
        }

        let payload = Bytes::copy_from_slice(&datagram[HEADER_SIZE..]);
        let Some((frame_id, pending)) = self.pending.as_mut() else {
            return Ok(None);
        };
        if *frame_id != header.frame_id {
            return Ok(None);
        }
        pending.pieces.insert(header.index, payload);

        if pending.pieces.len() < pending.total as usize {
            return Ok(None);
        }

        // Every piece is here.
        let (_, mut pending) = self.pending.take().expect("the picture is being assembled");
        let mut data = Vec::with_capacity(PIECE_PAYLOAD * pending.total as usize);
        for index in 0..pending.total {
            match pending.pieces.remove(&index) {
                Some(piece) => data.extend_from_slice(&piece),
                // It was counted, so it has to be there.
                None => return Ok(None),
            }
        }

        Ok(Some(EncodedFrame {
            data,
            pts: pending.pts,
            keyframe: pending.keyframe,
            width: pending.width,
            height: pending.height,
        }))
    }

    /// Tell the assembler how big the pictures are.
    ///
    /// It is called when the session starts, because the frame itself does
    /// not carry the size and a viewer needs it before the first picture
    /// can be shown.
    pub fn set_picture_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// How many pictures were thrown away because a piece never came.
    pub fn dropped_pictures(&self) -> u64 {
        self.dropped
    }

    /// Forget everything, for a new session on the same connection.
    pub fn reset(&mut self) {
        self.pending = None;
        self.newest = None;
        self.dropped = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_frame(size: usize, keyframe: bool) -> EncodedFrame {
        EncodedFrame {
            data: (0..size).map(|i| (i % 251) as u8).collect(),
            pts: Duration::from_millis(1234),
            keyframe,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn a_small_picture_is_one_piece() {
        let frame = a_frame(500, false);
        let pieces = split_frame(&frame, 7).expect("splittable");
        assert_eq!(pieces.len(), 1);
        let header = PieceHeader::decode(&pieces[0]).expect("a header");
        assert_eq!(header.frame_id, 7);
        assert_eq!(header.total, 1);
        assert_eq!(header.index, 0);
        assert!(!header.keyframe);
    }

    #[test]
    fn a_header_survives_a_round_trip() {
        let header = PieceHeader {
            frame_id: 4_000_000_000,
            total: 1234,
            index: 56,
            pts_ms: 987_654,
            keyframe: true,
        };
        let bytes = header.encode();
        assert_eq!(PieceHeader::decode(&bytes), Some(header));
    }

    #[test]
    fn bytes_that_are_not_ours_are_refused() {
        assert!(PieceHeader::decode(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(PieceHeader::decode(&[0x53]).is_none());
    }

    #[test]
    fn a_big_picture_comes_back_byte_for_byte() {
        let frame = a_frame(50_000, true);
        let pieces = split_frame(&frame, 1).expect("splittable");
        assert!(pieces.len() > 1, "a big picture must be split");

        let mut assembler = FrameAssembler::new();
        assembler.set_picture_size(1920, 1080);
        let mut done = None;
        for piece in &pieces {
            if let Some(f) = assembler.push(piece).expect("no stale piece") {
                done = Some(f);
            }
        }
        let frame_back = done.expect("the picture must come back");
        assert_eq!(frame_back.data, frame.data, "not one byte may be lost");
        assert_eq!(frame_back.pts, Duration::from_millis(1234));
        assert!(frame_back.keyframe);
    }

    #[test]
    fn pieces_that_arrive_out_of_order_still_come_back_right() {
        let frame = a_frame(10_000, false);
        let pieces = split_frame(&frame, 3).expect("splittable");
        let mut shuffled: Vec<_> = pieces.iter().rev().cloned().collect();
        shuffled.extend(std::iter::empty());

        let mut assembler = FrameAssembler::new();
        let mut done = None;
        for piece in &pieces.iter().rev().cloned().collect::<Vec<_>>() {
            if let Some(f) = assembler.push(piece).expect("no stale piece") {
                done = Some(f);
            }
        }
        let back = done.expect("the picture must come back");
        assert_eq!(back.data, frame.data);
    }

    #[test]
    fn a_picture_that_loses_a_piece_is_given_up_on() {
        let first = a_frame(10_000, true);
        let second = a_frame(10_000, false);
        let first_pieces = split_frame(&first, 10).expect("splittable");
        let second_pieces = split_frame(&second, 11).expect("splittable");

        let mut assembler = FrameAssembler::new();
        // Every piece of the first picture except the last one.
        for piece in &first_pieces[..first_pieces.len() - 1] {
            assert!(assembler.push(piece).expect("no stale piece").is_none());
        }
        // The next picture starts: the first one is out of date.
        let mut done = None;
        for piece in &second_pieces {
            if let Some(f) = assembler.push(piece).expect("no stale piece") {
                done = Some(f);
            }
        }
        let back = done.expect("the second picture must come back");
        assert_eq!(back.data, second.data);
        assert_eq!(assembler.dropped_pictures(), 1, "the first one was given up on");
    }

    #[test]
    fn a_piece_of_an_old_picture_is_refused() {
        let first = a_frame(5_000, true);
        let second = a_frame(5_000, true);
        let first_pieces = split_frame(&first, 20).expect("splittable");
        let second_pieces = split_frame(&second, 21).expect("splittable");

        let mut assembler = FrameAssembler::new();
        for piece in &second_pieces {
            assembler.push(piece).expect("no stale piece");
        }
        // Now a piece of picture 20 turns up late.
        let late = &first_pieces[0];
        assert!(matches!(
            assembler.push(late),
            Err(AssembleError::StalePicture(20))
        ));
    }

    #[test]
    fn a_picture_with_no_bytes_is_refused() {
        let mut frame = a_frame(10, false);
        frame.data.clear();
        assert!(matches!(split_frame(&frame, 1), Err(PieceError::Empty)));
    }

    #[test]
    fn a_datagram_that_is_only_a_header_is_ignored() {
        let mut assembler = FrameAssembler::new();
        assert!(assembler.push(b"hello").expect("ignored").is_none());
    }

    #[test]
    fn a_header_that_counts_zero_pieces_is_ignored() {
        let header = PieceHeader {
            frame_id: 1,
            total: 0,
            index: 0,
            pts_ms: 0,
            keyframe: false,
        };
        let mut assembler = FrameAssembler::new();
        assert!(assembler.push(&header.encode()).expect("ignored").is_none());
    }
}
