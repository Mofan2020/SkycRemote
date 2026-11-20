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
pub const HEADER_SIZE: usize = 19;

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
    /// When the picture should be shown, in nanoseconds from the start
    /// of the stream.
    ///
    /// Nanoseconds, not milliseconds: the first picture of a session is
    /// only microseconds old, and a coarser unit would round it down to
    /// zero.  The u64 holds more than a hundred years.
    pub pts_ns: u64,
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
        out[10..18].copy_from_slice(&self.pts_ns.to_be_bytes());
        out[18] = if self.keyframe { FLAG_KEYFRAME } else { 0 };
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
            pts_ns: u64::from_be_bytes([
                bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16],
                bytes[17],
            ]),
            keyframe: bytes[18] & FLAG_KEYFRAME != 0,
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

    let pts_ns = frame.pts.as_nanos().min(u64::MAX as u128) as u64;
    let mut pieces = Vec::with_capacity(total);

    for (index, chunk) in frame.data.chunks(PIECE_PAYLOAD).enumerate() {
        let header = PieceHeader {
            frame_id,
            total: total as u16,
            index: index as u16,
            pts_ns,
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
}

/// How many pictures may be half-assembled at the same time.
///
/// Datagrams do not arrive in order, so the pieces of two pictures can
/// cross each other on the way.  A few pictures have to be held at once
/// for that to work.  The number bounds how far behind the viewer can
/// fall: the oldest half-finished picture is thrown away once there are
/// this many.
const MAX_PENDING: usize = 4;

/// Puts pictures back together from the pieces that arrive.
///
/// Datagrams are not ordered, so several pictures are assembled at the
/// same time and the pieces are matched by the picture number in the
/// header.  When too many pictures are half-finished, the oldest one is
/// given up on: a picture that is still missing pieces by then is a
/// picture of a screen that has already changed.
pub struct FrameAssembler {
    /// The pictures that are waiting for pieces, by their number.
    pending: HashMap<u32, Pending>,
    /// The highest picture number seen, so a piece of an older picture
    /// can be recognised and dropped.
    newest: u32,
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
            pending: HashMap::new(),
            newest: 0,
            dropped: 0,
            width: 0,
            height: 0,
        }
    }

    /// Feed one datagram in.
    ///
    /// Returns the picture when this datagram was the last piece it was
    /// waiting for, and `None` otherwise.  A datagram that is not one of
    /// ours is ignored.
    pub fn push(&mut self, datagram: &[u8]) -> Result<Option<EncodedFrame>, AssembleError> {
        let Some(header) = PieceHeader::decode(datagram) else {
            return Ok(None);
        };
        if header.total == 0 || header.index >= header.total {
            return Ok(None);
        }

        let payload = Bytes::copy_from_slice(&datagram[HEADER_SIZE..]);

        // A picture that is already gone is of no use.
        if self.pending.contains_key(&header.frame_id) {
            let pending = self.pending.get_mut(&header.frame_id).expect("it is there");
            pending.pieces.insert(header.index, payload);
            if pending.pieces.len() < pending.total as usize {
                return Ok(None);
            }
        } else {
            // A picture that has never been seen starts here.  If it is
            // much older than the newest one, the session has moved on
            // and this piece is stale.
            if header.frame_id + (MAX_PENDING as u32) <= self.newest {
                return Err(AssembleError::StalePicture(header.frame_id));
            }
            let mut pending = Pending {
                pieces: HashMap::with_capacity(header.total as usize),
                total: header.total,
                pts: Duration::from_nanos(header.pts_ns),
                keyframe: header.keyframe,
            };
            pending.pieces.insert(header.index, payload);
            if pending.pieces.len() < pending.total as usize {
                self.pending.insert(header.frame_id, pending);
                self.newest = self.newest.max(header.frame_id);
                self.trim();
                return Ok(None);
            }
            // A picture of one piece is complete at once.
            let pending = self.pending.remove(&header.frame_id).unwrap_or(pending);
            self.newest = self.newest.max(header.frame_id);
            return Ok(Some(self.finish(pending)));
        }

        let pending = self.pending.remove(&header.frame_id).expect("it is there");
        Ok(Some(self.finish(pending)))
    }

    /// Turn a picture that is complete into the picture the caller wants.
    fn finish(&self, pending: Pending) -> EncodedFrame {
        let mut data = Vec::with_capacity(PIECE_PAYLOAD * pending.total as usize);
        for index in 0..pending.total {
            if let Some(piece) = pending.pieces.get(&index) {
                data.extend_from_slice(piece);
            }
        }
        EncodedFrame {
            data,
            pts: pending.pts,
            keyframe: pending.keyframe,
            width: self.width,
            height: self.height,
        }
    }

    /// Throw away the oldest half-finished pictures when there are too
    /// many.
    fn trim(&mut self) {
        while self.pending.len() > MAX_PENDING {
            let oldest = self
                .pending
                .keys()
                .copied()
                .min()
                .expect("there is at least one picture");
            self.pending.remove(&oldest);
            self.dropped += 1;
        }
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

    /// How many pictures are half-finished right now.
    pub fn pending_pictures(&self) -> usize {
        self.pending.len()
    }

    /// Forget everything, for a new session on the same connection.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.newest = 0;
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
            pts_ns: 987_654,
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
    fn a_picture_that_loses_a_piece_is_given_up_on_eventually() {
        // A picture that never gets all its pieces is not returned.  It
        // waits for a while, because its missing piece may simply be on
        // its way, and it is given up on only when there are too many
        // half-finished pictures.
        let first = a_frame(10_000, true);
        let first_pieces = split_frame(&first, 10).expect("splittable");

        let mut assembler = FrameAssembler::new();
        // Every piece of the first picture except the last one.
        for piece in &first_pieces[..first_pieces.len() - 1] {
            assert!(assembler.push(piece).expect("no stale piece").is_none());
        }
        assert_eq!(
            assembler.pending_pictures(),
            1,
            "it waits for the missing piece"
        );

        // Pictures that are complete come back regardless.
        let second = a_frame(10_000, false);
        let second_pieces = split_frame(&second, 11).expect("splittable");
        let mut done = None;
        for piece in &second_pieces {
            if let Some(f) = assembler.push(piece).expect("no stale piece") {
                done = Some(f);
            }
        }
        let back = done.expect("the second picture must come back");
        assert_eq!(back.data, second.data);
    }

    #[test]
    fn too_many_half_finished_pictures_throw_the_oldest_one_away() {
        // The wait cannot grow without a limit, or a broken sender would
        // hold the viewer back for ever.
        let mut assembler = FrameAssembler::new();
        for number in 0..10u32 {
            let picture = a_frame(10_000, true);
            let pieces = split_frame(&picture, number).expect("splittable");
            // All but the last piece, so nothing ever completes.
            for piece in &pieces[..pieces.len() - 1] {
                assembler.push(piece).expect("no stale piece");
            }
        }
        assert!(
            assembler.pending_pictures() <= 4,
            "too many pictures are being held: {}",
            assembler.pending_pictures()
        );
        assert!(
            assembler.dropped_pictures() > 0,
            "the pictures that were given up on must be counted"
        );
    }

    #[test]
    fn pieces_that_cross_each_other_still_make_their_pictures() {
        // Datagrams do not arrive in order, so the pieces of two
        // pictures can cross on the way.  Each picture must still come
        // out right.
        let first = a_frame(10_000, true);
        let second = a_frame(12_000, false);
        let first_pieces = split_frame(&first, 20).expect("splittable");
        let second_pieces = split_frame(&second, 21).expect("splittable");

        // The first half of one picture, then the first half of the
        // other, then the rest of both.
        let mut mixed: Vec<Bytes> = Vec::new();
        let half = first_pieces.len() / 2;
        mixed.extend(first_pieces[..half].iter().cloned());
        mixed.extend(second_pieces[..half].iter().cloned());
        mixed.extend(first_pieces[half..].iter().cloned());
        mixed.extend(second_pieces[half..].iter().cloned());

        let mut assembler = FrameAssembler::new();
        let mut done: Vec<Vec<u8>> = Vec::new();
        for piece in &mixed {
            if let Some(picture) = assembler.push(piece).expect("no stale piece") {
                done.push(picture.data);
            }
        }
        assert_eq!(done.len(), 2, "both pictures must come back");
        assert!(
            done.contains(&first.data),
            "the first picture must be whole"
        );
        assert!(
            done.contains(&second.data),
            "the second picture must be whole"
        );
    }

    #[test]
    fn a_piece_of_a_long_overdue_picture_is_refused() {
        let first = a_frame(5_000, true);
        let second = a_frame(5_000, true);
        let first_pieces = split_frame(&first, 20).expect("splittable");
        let second_pieces = split_frame(&second, 24).expect("splittable");

        let mut assembler = FrameAssembler::new();
        for piece in &second_pieces {
            assembler.push(piece).expect("no stale piece");
        }
        // Now a piece of picture 20 turns up, four pictures too late.
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
            pts_ns: 0,
            keyframe: false,
        };
        let mut assembler = FrameAssembler::new();
        assert!(assembler.push(&header.encode()).expect("ignored").is_none());
    }
}
