//! scrcpy 4.x stream framing.
//!
//! Every item on the video and audio sockets is prefixed with a 12-byte header.
//! The most significant bit of the first byte says which kind it is.
//!
//! Session header (video stream only) — carries the frame size and nothing else:
//!
//! ```text
//!  byte 0   byte 1   byte 2   byte 3   byte 4 .. 7    byte 8 .. 11
//! 10000000 00000000 00000000 0000000R    width           height
//! ^                                ^
//! session flag                     client-resized flag
//! ```
//!
//! Media header — followed by `size` bytes of packet data:
//!
//! ```text
//!  byte 0                                   byte 7   byte 8 .. 11
//! 0CK..... <------------ PTS ---------------------->     size
//! ^^^
//! ||`- key frame
//! |`-- config packet
//! `--- media packet (0)
//! ```
//!
//! scrcpy 3.x had no session header: the frame size was part of the video
//! stream header, and the config and key-frame flags sat one bit higher.

use anyhow::{Context, Result, bail};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::Read;
use std::net::TcpStream;

/// Packet header size: 8 bytes PTS+flags + 4 bytes length
const PACKET_HEADER_SIZE: usize = 12;

/// The largest a single packet may claim to be.
///
/// The length arrives from the socket as a 32-bit number and was used as one,
/// so a wrong one asked for up to four gigabytes before the read that would
/// have failed got the chance. Upstream does not bound this either; it is
/// bounded here because a number that large is not a frame at any bitrate
/// scrcpy will encode at — sixty-four megabytes is several seconds of the
/// highest, arriving as one packet.
const MAX_PACKET_BYTES: usize = 64 << 20;

/// PTS flags. scrcpy 4.0 moved config and key-frame down one bit to free the
/// most significant bit for the session flag.
const PACKET_FLAG_SESSION: u64 = 1 << 63;
const PACKET_FLAG_CONFIG: u64 = 1 << 62;
const PACKET_FLAG_KEY_FRAME: u64 = 1 << 61;
const PACKET_PTS_MASK: u64 = PACKET_FLAG_KEY_FRAME - 1;

/// Set in the session header when the size changed because the client asked for
/// it, rather than because the device rotated or the app resized.
const SESSION_FLAG_CLIENT_RESIZED: u8 = 1;

/// Codec IDs (ASCII values matching scrcpy server)
const CODEC_ID_H264: u32 = 0x68323634; // "h264"
const CODEC_ID_H265: u32 = 0x68323635; // "h265"
const CODEC_ID_AV1: u32 = 0x00617631;  // "av1"
const CODEC_ID_VP8: u32 = 0x00767038;  // "vp8"
const CODEC_ID_VP9: u32 = 0x00767039;  // "vp9"
const CODEC_ID_OPUS: u32 = 0x6f707573; // "opus"
const CODEC_ID_AAC: u32 = 0x00616163;  // "aac"
const CODEC_ID_FLAC: u32 = 0x666c6163; // "flac"
const CODEC_ID_RAW: u32 = 0x00726177;  // "raw"

/// Codec type for the pipeline
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CodecType {
    H264,
    H265,
    AV1,
    Vp8,
    Vp9,
    Opus,
    Aac,
    Flac,
    Raw,
}

/// A raw packet from the demuxer
#[derive(Debug)]
pub struct DemuxPacket {
    pub data: Vec<u8>,
    pub pts: Option<i64>, // None for config packets
    pub is_key_frame: bool,
    pub is_config: bool,
}

/// The frame size the video stream switches to from here on.
#[derive(Debug, Clone, Copy)]
pub struct SessionMeta {
    pub width: u32,
    pub height: u32,
    pub client_resized: bool,
}

/// One item off a stream socket.
#[derive(Debug)]
pub enum StreamItem {
    Packet(DemuxPacket),
    Session(SessionMeta),
}

/// Initial video stream info
#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub codec: CodecType,
    pub width: u32,
    pub height: u32,
}

/// Initial audio stream info
#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub codec: CodecType,
}

/// Read the codec ID from the stream
pub fn read_codec_id(stream: &mut TcpStream) -> Result<Option<CodecType>> {
    let raw = stream.read_u32::<BigEndian>()
        .context("Failed to read codec ID")?;

    match raw {
        0 => {
            log::warn!("Stream disabled by device");
            Ok(None)
        }
        1 => bail!("Stream configuration error on device"),
        CODEC_ID_H264 => Ok(Some(CodecType::H264)),
        CODEC_ID_H265 => Ok(Some(CodecType::H265)),
        CODEC_ID_AV1 => Ok(Some(CodecType::AV1)),
        CODEC_ID_VP8 => Ok(Some(CodecType::Vp8)),
        CODEC_ID_VP9 => Ok(Some(CodecType::Vp9)),
        CODEC_ID_OPUS => Ok(Some(CodecType::Opus)),
        CODEC_ID_AAC => Ok(Some(CodecType::Aac)),
        CODEC_ID_FLAC => Ok(Some(CodecType::Flac)),
        CODEC_ID_RAW => Ok(Some(CodecType::Raw)),
        other => bail!("Unknown codec ID: 0x{:08x}", other),
    }
}

/// Read the video stream header: the codec id, then the first session header.
///
/// In scrcpy 3.x the size followed the codec id as eight bare bytes. From 4.0 it
/// arrives as a session header instead, and can arrive again later whenever the
/// size changes.
pub fn read_video_header(stream: &mut TcpStream) -> Result<Option<VideoInfo>> {
    let Some(codec) = read_codec_id(stream)? else {
        return Ok(None);
    };

    let mut header = [0u8; PACKET_HEADER_SIZE];
    stream
        .read_exact(&mut header)
        .context("Failed to read the video session header")?;

    if !is_session_header(&header) {
        bail!(
            "Expected a session header to open the video stream, got a media packet. \
             Is the server older than 4.0?"
        );
    }

    let session = parse_session_header(&header);
    if session.width == 0 || session.height == 0 {
        bail!("Invalid session video size: {}x{}", session.width, session.height);
    }

    log::info!("Video: {:?} {}x{}", codec, session.width, session.height);
    Ok(Some(VideoInfo {
        codec,
        width: session.width,
        height: session.height,
    }))
}

/// Read the audio stream header (codec only)
pub fn read_audio_header(stream: &mut TcpStream) -> Result<Option<AudioInfo>> {
    match read_codec_id(stream)? {
        None => Ok(None),
        Some(codec) => {
            log::info!("Audio: {:?}", codec);
            Ok(Some(AudioInfo { codec }))
        }
    }
}

/// Is this 12-byte header a session header rather than a media packet?
fn is_session_header(header: &[u8; PACKET_HEADER_SIZE]) -> bool {
    header[0] & 0x80 != 0
}

fn parse_session_header(header: &[u8; PACKET_HEADER_SIZE]) -> SessionMeta {
    SessionMeta {
        width: u32::from_be_bytes(header[4..8].try_into().unwrap()),
        height: u32::from_be_bytes(header[8..12].try_into().unwrap()),
        client_resized: header[3] & SESSION_FLAG_CLIENT_RESIZED != 0,
    }
}

/// Read one item from a stream.
///
/// Returns `None` on end of stream (connection closed).
pub fn read_stream_item<R: Read>(reader: &mut R) -> Result<Option<StreamItem>> {
    let mut header = [0u8; PACKET_HEADER_SIZE];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    if is_session_header(&header) {
        return Ok(Some(StreamItem::Session(parse_session_header(&header))));
    }

    let pts_flags = u64::from_be_bytes(header[0..8].try_into().unwrap());
    let len = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;

    if len == 0 {
        bail!("Received packet with zero length");
    }
    if len > MAX_PACKET_BYTES {
        bail!("Packet claims {len} bytes, which is not a frame");
    }

    // Read packet data
    let mut data = vec![0u8; len];
    reader.read_exact(&mut data)
        .context("Failed to read packet data")?;

    debug_assert_eq!(pts_flags & PACKET_FLAG_SESSION, 0);
    let is_config = (pts_flags & PACKET_FLAG_CONFIG) != 0;
    let is_key_frame = (pts_flags & PACKET_FLAG_KEY_FRAME) != 0;
    let pts = if is_config {
        None
    } else {
        Some((pts_flags & PACKET_PTS_MASK) as i64)
    };

    Ok(Some(StreamItem::Packet(DemuxPacket {
        data,
        pts,
        is_key_frame,
        is_config,
    })))
}

#[cfg(test)]
mod tests {

    /// A length from the socket is a claim, not a fact. It used to be handed
    /// straight to an allocation, so a wrong number asked for up to four
    /// gigabytes before the read that would have failed got the chance. Zero
    /// was already refused; the other end was not.
    #[test]
    fn a_packet_that_claims_more_than_a_frame_is_refused() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&0u64.to_be_bytes()); // pts, no flags
        wire.extend_from_slice(&u32::MAX.to_be_bytes());
        let error = read_stream_item(&mut wire.as_slice()).expect_err("it must be refused");
        assert!(
            error.to_string().contains("not a frame"),
            "refused for the wrong reason: {error}"
        );
    }

    /// One the size of a real frame still reads.
    #[test]
    fn a_packet_the_size_of_a_frame_still_reads() {
        let payload = vec![7u8; 4096];
        let mut wire = Vec::new();
        wire.extend_from_slice(&1234u64.to_be_bytes());
        wire.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        wire.extend_from_slice(&payload);
        match read_stream_item(&mut wire.as_slice()).expect("it reads") {
            Some(StreamItem::Packet(packet)) => {
                assert_eq!(packet.data.len(), payload.len());
                assert_eq!(packet.pts, Some(1234));
            }
            other => panic!("that is not a packet: {other:?}"),
        }
    }
    use super::*;

    fn read_one(bytes: &[u8]) -> StreamItem {
        let mut cursor = std::io::Cursor::new(bytes.to_vec());
        read_stream_item(&mut cursor)
            .expect("read failed")
            .expect("unexpected end of stream")
    }

    fn session_bytes(width: u32, height: u32, client_resized: bool) -> Vec<u8> {
        let flags: u32 = 0x8000_0000 | u32::from(client_resized);
        let mut bytes = flags.to_be_bytes().to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

    fn packet_bytes(pts_flags: u64, payload: &[u8]) -> Vec<u8> {
        let mut bytes = pts_flags.to_be_bytes().to_vec();
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn reads_a_session_header() {
        match read_one(&session_bytes(1080, 2340, false)) {
            StreamItem::Session(session) => {
                assert_eq!((session.width, session.height), (1080, 2340));
                assert!(!session.client_resized);
            }
            other => panic!("expected a session header, got {other:?}"),
        }
    }

    #[test]
    fn reads_the_client_resized_flag() {
        match read_one(&session_bytes(720, 1280, true)) {
            StreamItem::Session(session) => assert!(session.client_resized),
            other => panic!("expected a session header, got {other:?}"),
        }
    }

    #[test]
    fn reads_a_key_frame_packet() {
        let bytes = packet_bytes(PACKET_FLAG_KEY_FRAME | 1234, &[1, 2, 3]);
        match read_one(&bytes) {
            StreamItem::Packet(packet) => {
                assert!(packet.is_key_frame);
                assert!(!packet.is_config);
                assert_eq!(packet.pts, Some(1234));
                assert_eq!(packet.data, vec![1, 2, 3]);
            }
            other => panic!("expected a packet, got {other:?}"),
        }
    }

    #[test]
    fn reads_a_config_packet() {
        let bytes = packet_bytes(PACKET_FLAG_CONFIG, &[0xde, 0xad]);
        match read_one(&bytes) {
            StreamItem::Packet(packet) => {
                assert!(packet.is_config);
                assert!(!packet.is_key_frame);
                assert_eq!(packet.pts, None);
            }
            other => panic!("expected a packet, got {other:?}"),
        }
    }

    /// scrcpy 3.x put the config flag in the bit that 4.x uses to mark a session
    /// header, so a 3.x stream is misread rather than rejected. This is why the
    /// client pins an exact server version.
    #[test]
    fn a_scrcpy_3_config_packet_now_looks_like_a_session_header() {
        let legacy_config_flag = 1u64 << 63;
        let bytes = packet_bytes(legacy_config_flag, &[0xde, 0xad]);
        assert!(matches!(read_one(&bytes), StreamItem::Session(_)));
    }

    #[test]
    fn reports_end_of_stream() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        assert!(read_stream_item(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn rejects_a_zero_length_packet() {
        let bytes = packet_bytes(0, &[]);
        let mut cursor = std::io::Cursor::new(bytes);
        assert!(read_stream_item(&mut cursor).is_err());
    }
}
