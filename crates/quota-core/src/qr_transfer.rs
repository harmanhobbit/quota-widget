//! Chunk a sealed credential bundle into scannable QR frames, and reassemble
//! them back on the scanning side.
//!
//! This is the desktop→phone transport ADR-0008 describes: the same sealed
//! bytes [`crate::seal::seal`] produces for the file export, cut into pieces
//! small enough to render as one or more QR codes and reassembled by whatever
//! scans them. Pure chunking/reassembly logic only — QR rendering (the
//! `qrcode` crate) and camera scanning are the only other things this module
//! touches; sockets, cameras and screens stay in the platform adapters.
//!
//! Each frame carries a 4-byte session id, freshly random per render, so a
//! scan in progress against one export cannot be corrupted by frames from an
//! unrelated or restarted one — [`FrameCollector`] simply restarts collection
//! when it sees a session id it hasn't seen before.

use base64::{engine::general_purpose::STANDARD, Engine};
use qrcode::{render::svg, EcLevel, QrCode};
use std::collections::HashMap;
use thiserror::Error;

const MAGIC: [u8; 2] = *b"QW";
const SESSION_LEN: usize = 4;
/// Raw (pre-base64) payload bytes per frame. Combined with the 8-byte header
/// this keeps each frame's QR text under ~420 characters, which `qrcode`
/// comfortably encodes at a scannable module size under `EcLevel::L`.
const CHUNK_SIZE: usize = 300;
/// Upper bound on how many frames one transfer will render. Above this the
/// desktop refuses to render an oversized, unreliable code sequence and
/// directs the user elsewhere (issue #156's acceptance criteria).
const MAX_FRAMES: usize = 20;
const HEADER_LEN: usize = MAGIC.len() + SESSION_LEN + 1 + 1;

/// A random session id, distinguishing one render/scan attempt from another.
pub type SessionId = [u8; SESSION_LEN];

/// `sealed` needs more frames than this transport supports.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("this transfer needs {frames_needed} QR codes, but only {max_frames} are supported")]
pub struct TooLarge {
    pub frames_needed: usize,
    pub max_frames: usize,
}

/// Split `sealed` into frame payloads, each a base64 string ready to render as
/// a QR code's content (or to feed straight into [`FrameCollector::accept`]
/// for testing). Base64 keeps every frame plain ASCII: a phone's barcode
/// scanner hands decoded QR content back as a `String`, and text is the only
/// representation guaranteed not to mangle the frame on that trip.
pub fn encode_frames(sealed: &[u8], session: SessionId) -> Result<Vec<String>, TooLarge> {
    let chunks: Vec<&[u8]> = if sealed.is_empty() {
        vec![&[]]
    } else {
        sealed.chunks(CHUNK_SIZE).collect()
    };
    if chunks.len() > MAX_FRAMES || chunks.len() > u8::MAX as usize {
        return Err(TooLarge {
            frames_needed: chunks.len(),
            max_frames: MAX_FRAMES,
        });
    }
    let total = chunks.len() as u8;
    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let mut frame = Vec::with_capacity(HEADER_LEN + chunk.len());
            frame.extend_from_slice(&MAGIC);
            frame.extend_from_slice(&session);
            frame.push(total);
            frame.push(index as u8);
            frame.extend_from_slice(chunk);
            STANDARD.encode(frame)
        })
        .collect())
}

/// Render `sealed`, sealed under a passphrase, as one SVG QR code per frame.
pub fn render_frames_svg(sealed: &[u8], session: SessionId) -> Result<Vec<String>, TooLarge> {
    let frames = encode_frames(sealed, session)?;
    Ok(frames
        .iter()
        .map(|text| {
            let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L)
                .expect("a base64 frame text always fits within QR capacity below MAX_FRAMES");
            code.render::<svg::Color>().min_dimensions(280, 280).build()
        })
        .collect())
}

/// One frame failed to parse as ours. The caller (a scan loop) should simply
/// keep scanning — a stray unrelated QR code in frame is expected, not fatal.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("not a Quota Widget QR transfer frame")]
    NotOurs,
    #[error("frame index {index} is out of range for {total} total frames")]
    OutOfRange { index: u8, total: u8 },
}

/// Progress after accepting one frame: how many distinct frames have been
/// collected out of how many the session needs, and whether that means
/// collection is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FrameStatus {
    pub have: u8,
    pub total: u8,
    pub complete: bool,
}

/// Reassembles frames scanned in any order (and possibly duplicated) back
/// into the original sealed bytes. A frame belonging to a session id other
/// than the one currently in progress restarts collection under the new
/// session — the scanner has no way to tell a stale QR code left over from a
/// previous attempt from a fresh one, so the newest session always wins.
#[derive(Debug, Default)]
pub struct FrameCollector {
    session: Option<SessionId>,
    total: u8,
    frames: HashMap<u8, Vec<u8>>,
}

impl FrameCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one scanned frame's decoded text. Returns the resulting progress,
    /// or a [`FrameError`] for text that isn't a well-formed frame of ours —
    /// which the caller should treat as "keep scanning", not a fatal error.
    pub fn accept(&mut self, text: &str) -> Result<FrameStatus, FrameError> {
        let bytes = STANDARD
            .decode(text.trim())
            .map_err(|_| FrameError::NotOurs)?;
        if bytes.len() < HEADER_LEN || bytes[0..2] != MAGIC {
            return Err(FrameError::NotOurs);
        }
        let session: SessionId = bytes[2..2 + SESSION_LEN]
            .try_into()
            .expect("slice length matches SESSION_LEN");
        let total = bytes[2 + SESSION_LEN];
        let index = bytes[2 + SESSION_LEN + 1];
        let chunk = bytes[HEADER_LEN..].to_vec();
        if total == 0 || index >= total {
            return Err(FrameError::OutOfRange { index, total });
        }

        if self.session != Some(session) {
            self.session = Some(session);
            self.total = total;
            self.frames.clear();
        }
        self.frames.insert(index, chunk);
        Ok(self
            .status()
            .expect("a frame was just accepted into a session"))
    }

    /// The current progress, without accepting a new frame. `None` before
    /// any frame has arrived.
    pub fn status(&self) -> Option<FrameStatus> {
        self.session?;
        let have = self.frames.len() as u8;
        Some(FrameStatus {
            have,
            total: self.total,
            complete: have == self.total,
        })
    }

    /// The sealed bytes, once every frame of the current session has been
    /// collected. `None` while still in progress or before any frame arrived.
    pub fn assemble(&self) -> Option<Vec<u8>> {
        if self.session.is_none() || self.frames.len() as u8 != self.total {
            return None;
        }
        let mut out = Vec::new();
        for index in 0..self.total {
            out.extend_from_slice(self.frames.get(&index)?);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_sealed(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 256) as u8).collect()
    }

    #[test]
    fn round_trips_through_shuffled_frames() {
        let sealed = sample_sealed(1000);
        let session = [1, 2, 3, 4];
        let mut frames = encode_frames(&sealed, session).unwrap();
        // Reverse to prove order-independence rather than relying on insertion
        // order happening to match.
        frames.reverse();

        let mut collector = FrameCollector::new();
        let mut last_status = None;
        for frame in &frames {
            last_status = Some(collector.accept(frame).unwrap());
        }
        assert!(last_status.unwrap().complete);
        assert_eq!(collector.assemble().unwrap(), sealed);
    }

    #[test]
    fn single_frame_round_trips() {
        let sealed = sample_sealed(10);
        let session = [9, 9, 9, 9];
        let frames = encode_frames(&sealed, session).unwrap();
        assert_eq!(frames.len(), 1);

        let mut collector = FrameCollector::new();
        assert!(collector.accept(&frames[0]).unwrap().complete);
        assert_eq!(collector.assemble().unwrap(), sealed);
    }

    #[test]
    fn empty_sealed_bytes_round_trip() {
        let session = [0, 0, 0, 1];
        let frames = encode_frames(&[], session).unwrap();
        assert_eq!(frames.len(), 1);
        let mut collector = FrameCollector::new();
        assert!(collector.accept(&frames[0]).unwrap().complete);
        assert_eq!(collector.assemble().unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn duplicate_frames_do_not_confuse_progress() {
        let sealed = sample_sealed(1000);
        let session = [1, 2, 3, 4];
        let frames = encode_frames(&sealed, session).unwrap();
        assert!(frames.len() > 1, "test needs a multi-frame transfer");

        let mut collector = FrameCollector::new();
        collector.accept(&frames[0]).unwrap();
        let status = collector.accept(&frames[0]).unwrap();
        assert_eq!(
            status,
            FrameStatus {
                have: 1,
                total: frames.len() as u8,
                complete: false,
            }
        );
    }

    #[test]
    fn oversized_input_is_refused_at_encode_time() {
        let sealed = sample_sealed(CHUNK_SIZE * (MAX_FRAMES + 1));
        let err = encode_frames(&sealed, [0; 4]).unwrap_err();
        assert_eq!(
            err,
            TooLarge {
                frames_needed: MAX_FRAMES + 1,
                max_frames: MAX_FRAMES,
            }
        );
    }

    #[test]
    fn largest_supported_size_still_encodes() {
        let sealed = sample_sealed(CHUNK_SIZE * MAX_FRAMES);
        let frames = encode_frames(&sealed, [0; 4]).unwrap();
        assert_eq!(frames.len(), MAX_FRAMES);
    }

    #[test]
    fn malformed_frame_is_rejected_without_poisoning_progress() {
        let sealed = sample_sealed(1000);
        let session = [1, 2, 3, 4];
        let frames = encode_frames(&sealed, session).unwrap();
        assert!(frames.len() > 1, "test needs a multi-frame transfer");

        let mut collector = FrameCollector::new();
        collector.accept(&frames[0]).unwrap();
        // A stray, unrelated QR code the camera happened to pick up.
        let err = collector.accept("not a real frame at all").unwrap_err();
        assert_eq!(err, FrameError::NotOurs);
        // Progress from the real frame already accepted is untouched.
        assert_eq!(
            collector.accept(&frames[1]).unwrap(),
            FrameStatus {
                have: 2,
                total: frames.len() as u8,
                complete: frames.len() == 2,
            }
        );
    }

    #[test]
    fn a_new_session_restarts_collection() {
        let sealed_a = sample_sealed(1000);
        let session_a = [1, 1, 1, 1];
        let frames_a = encode_frames(&sealed_a, session_a).unwrap();
        assert!(frames_a.len() > 1, "test needs a multi-frame transfer");

        let sealed_b = sample_sealed(20);
        let session_b = [2, 2, 2, 2];
        let frames_b = encode_frames(&sealed_b, session_b).unwrap();

        let mut collector = FrameCollector::new();
        collector.accept(&frames_a[0]).unwrap();
        // A fresh render on the desktop starts a new session; the scanner has
        // no way to know the old one is abandoned except seeing a new id.
        let status = collector.accept(&frames_b[0]).unwrap();
        assert!(status.complete);
        assert_eq!(collector.assemble().unwrap(), sealed_b);
    }

    #[test]
    fn render_svg_produces_one_scannable_frame_per_chunk() {
        let sealed = sample_sealed(700);
        let svgs = render_frames_svg(&sealed, [7; 4]).unwrap();
        assert_eq!(svgs.len(), encode_frames(&sealed, [7; 4]).unwrap().len());
        for svg in &svgs {
            assert!(svg.contains("<svg"));
        }
    }
}
