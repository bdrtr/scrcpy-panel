use anyhow::{Context, Result, bail};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::Read;
use std::net::TcpStream;

/// Packet header size: 8 bytes PTS+flags + 4 bytes length
const PACKET_HEADER_SIZE: usize = 12;

/// PTS flags
const PACKET_FLAG_CONFIG: u64 = 1 << 63;
const PACKET_FLAG_KEY_FRAME: u64 = 1 << 62;
const PACKET_PTS_MASK: u64 = PACKET_FLAG_KEY_FRAME - 1;

/// Codec IDs (ASCII values matching scrcpy server)
const CODEC_ID_H264: u32 = 0x68323634; // "h264"
const CODEC_ID_H265: u32 = 0x68323635; // "h265"
const CODEC_ID_AV1: u32 = 0x00617631;  // "av1"
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
    Opus,
    Aac,
    Flac,
    Raw,
}

impl CodecType {
    pub fn is_video(&self) -> bool {
        matches!(self, CodecType::H264 | CodecType::H265 | CodecType::AV1)
    }
}

/// A raw packet from the demuxer
#[derive(Debug)]
pub struct DemuxPacket {
    pub data: Vec<u8>,
    pub pts: Option<i64>, // None for config packets
    pub is_key_frame: bool,
    pub is_config: bool,
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
        CODEC_ID_OPUS => Ok(Some(CodecType::Opus)),
        CODEC_ID_AAC => Ok(Some(CodecType::Aac)),
        CODEC_ID_FLAC => Ok(Some(CodecType::Flac)),
        CODEC_ID_RAW => Ok(Some(CodecType::Raw)),
        other => bail!("Unknown codec ID: 0x{:08x}", other),
    }
}

/// Read the initial video size (sent only for video streams)
pub fn read_video_size(stream: &mut TcpStream) -> Result<(u32, u32)> {
    let width = stream.read_u32::<BigEndian>().context("Failed to read width")?;
    let height = stream.read_u32::<BigEndian>().context("Failed to read height")?;
    Ok((width, height))
}

/// Read the video stream header (codec + size)
pub fn read_video_header(stream: &mut TcpStream) -> Result<Option<VideoInfo>> {
    match read_codec_id(stream)? {
        None => Ok(None),
        Some(codec) => {
            let (width, height) = read_video_size(stream)?;
            log::info!("Video: {:?} {}x{}", codec, width, height);
            Ok(Some(VideoInfo { codec, width, height }))
        }
    }
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

/// Read a single packet from the stream
///
/// Returns None on end-of-stream (connection closed)
pub fn read_packet(stream: &mut TcpStream) -> Result<Option<DemuxPacket>> {
    read_packet_from(stream)
}

/// Read a single packet from a buffered reader (for pipeline mode)
pub fn read_packet_buf<R: Read>(reader: &mut R) -> Result<Option<DemuxPacket>> {
    read_packet_from(reader)
}

/// Internal: read a packet from any Read impl
fn read_packet_from<R: Read>(reader: &mut R) -> Result<Option<DemuxPacket>> {
    // Read 12-byte header
    let mut header = [0u8; PACKET_HEADER_SIZE];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let pts_flags = u64::from_be_bytes(header[0..8].try_into().unwrap());
    let len = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;

    if len == 0 {
        bail!("Received packet with zero length");
    }

    // Read packet data
    let mut data = vec![0u8; len];
    reader.read_exact(&mut data)
        .context("Failed to read packet data")?;

    let is_config = (pts_flags & PACKET_FLAG_CONFIG) != 0;
    let is_key_frame = (pts_flags & PACKET_FLAG_KEY_FRAME) != 0;
    let pts = if is_config {
        None
    } else {
        Some((pts_flags & PACKET_PTS_MASK) as i64)
    };

    Ok(Some(DemuxPacket {
        data,
        pts,
        is_key_frame,
        is_config,
    }))
}
