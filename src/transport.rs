use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use crate::{
    capability::TransportCodec,
    protocol::{FrameHeader, HEADER_LEN, VERSION},
    reassembly::UDP_PAYLOAD,
};

pub const VIDEO_PORT: u16 = 45_831;
pub const DISCOVERY_PORT: u16 = 45_832;
pub const HELLO_MAGIC: [u8; 4] = *b"CBH2";
pub const FRAGMENT_MAGIC: [u8; 4] = *b"CBF2";
pub const FRAGMENT_HEADER_LEN: usize = 28;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverHello {
    pub version: u16,
    pub codecs: Vec<TransportCodec>,
}

impl ReceiverHello {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(7 + self.codecs.len());
        bytes.extend_from_slice(&HELLO_MAGIC);
        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.push(self.codecs.len().min(u8::MAX as usize) as u8);
        bytes.extend(
            self.codecs
                .iter()
                .take(u8::MAX as usize)
                .map(|codec| codec.id()),
        );
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, HandshakeError> {
        if bytes.len() < 7 || bytes[0..4] != HELLO_MAGIC {
            return Err(HandshakeError::Malformed);
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if version != VERSION {
            return Err(HandshakeError::VersionMismatch {
                peer: version,
                local: VERSION,
            });
        }
        let count = bytes[6] as usize;
        if bytes.len() != 7 + count {
            return Err(HandshakeError::Malformed);
        }
        let mut codecs = Vec::with_capacity(count);
        for id in &bytes[7..] {
            codecs.push(TransportCodec::from_id(*id).ok_or(HandshakeError::UnsupportedCodec(*id))?);
        }
        Ok(Self { version, codecs })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    Malformed,
    VersionMismatch { peer: u16, local: u16 },
    UnsupportedCodec(u8),
    NoCommonCodec,
}

pub fn negotiate(
    preferred: &[TransportCodec],
    peer: &ReceiverHello,
) -> Result<TransportCodec, HandshakeError> {
    preferred
        .iter()
        .copied()
        .find(|codec| peer.codecs.contains(codec))
        .ok_or(HandshakeError::NoCommonCodec)
}

pub struct ReceiverLease {
    endpoint: Option<SocketAddr>,
    last_seen: Option<Instant>,
    ttl: Duration,
}

impl Default for ReceiverLease {
    fn default() -> Self {
        Self {
            endpoint: None,
            last_seen: None,
            ttl: Duration::from_secs(6),
        }
    }
}

impl ReceiverLease {
    pub fn update(&mut self, endpoint: SocketAddr, now: Instant) -> bool {
        if !private_lan(endpoint.ip()) {
            return false;
        }
        self.endpoint = Some(endpoint);
        self.last_seen = Some(now);
        true
    }
    pub fn active(&self, now: Instant) -> Option<SocketAddr> {
        match (self.endpoint, self.last_seen) {
            (Some(endpoint), Some(seen)) if now.duration_since(seen) < self.ttl => Some(endpoint),
            _ => None,
        }
    }
}

pub fn private_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private(),
        IpAddr::V6(_) => false,
    }
}

#[derive(Debug, Clone)]
pub struct Fragment<'a> {
    pub frame_id: u64,
    pub index: u32,
    pub count: u32,
    pub expected_frame_bytes: u32,
    pub data: &'a [u8],
}

pub fn fragments(header: &FrameHeader, payload: &[u8]) -> Result<Vec<Vec<u8>>, io::Error> {
    if payload.len() != header.payload_len as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload length mismatch",
        ));
    }
    let wire_header = header
        .encode()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e:?}")))?;
    let first_capacity = UDP_PAYLOAD.saturating_sub(FRAGMENT_HEADER_LEN + HEADER_LEN);
    let other_capacity = UDP_PAYLOAD.saturating_sub(FRAGMENT_HEADER_LEN);
    let remaining = payload.len().saturating_sub(first_capacity);
    let count = 1 + remaining.div_ceil(other_capacity);
    let mut result = Vec::with_capacity(count);
    let mut offset = 0usize;
    for index in 0..count {
        let capacity = if index == 0 {
            first_capacity
        } else {
            other_capacity
        };
        let end = (offset + capacity).min(payload.len());
        let mut packet = Vec::with_capacity(FRAGMENT_HEADER_LEN + HEADER_LEN + end - offset);
        packet.extend_from_slice(&FRAGMENT_MAGIC);
        packet.extend_from_slice(&VERSION.to_le_bytes());
        packet.extend_from_slice(&(if index == 0 { 1u16 } else { 0u16 }).to_le_bytes());
        packet.extend_from_slice(&header.frame_id.to_le_bytes());
        packet.extend_from_slice(&(index as u32).to_le_bytes());
        packet.extend_from_slice(&(count as u32).to_le_bytes());
        packet.extend_from_slice(&header.payload_len.to_le_bytes());
        if index == 0 {
            packet.extend_from_slice(&wire_header);
        }
        packet.extend_from_slice(&payload[offset..end]);
        offset = end;
        result.push(packet);
    }
    Ok(result)
}

pub fn parse_fragment(packet: &[u8]) -> Result<(Fragment<'_>, Option<FrameHeader>), io::Error> {
    if packet.len() < FRAGMENT_HEADER_LEN || packet[0..4] != FRAGMENT_MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad fragment"));
    }
    let version = u16::from_le_bytes(packet[4..6].try_into().unwrap());
    if version != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("protocol version {version}, expected {VERSION}"),
        ));
    }
    let has_header = u16::from_le_bytes(packet[6..8].try_into().unwrap()) & 1 != 0;
    let frame_id = u64::from_le_bytes(packet[8..16].try_into().unwrap());
    let index = u32::from_le_bytes(packet[16..20].try_into().unwrap());
    let count = u32::from_le_bytes(packet[20..24].try_into().unwrap());
    let expected_frame_bytes = u32::from_le_bytes(packet[24..28].try_into().unwrap());
    let mut offset = 28;
    let header = if has_header {
        if packet.len() < offset + HEADER_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated frame header",
            ));
        }
        let value = FrameHeader::decode(&packet[offset..offset + HEADER_LEN])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}")))?;
        offset += HEADER_LEN;
        Some(value)
    } else {
        None
    };
    Ok((
        Fragment {
            frame_id,
            index,
            count,
            expected_frame_bytes,
            data: &packet[offset..],
        },
        header,
    ))
}

pub fn discovery_socket() -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::FLAG_KEYFRAME;
    #[test]
    fn handshake_rejects_version_mismatch() {
        let mut hello = ReceiverHello {
            version: VERSION,
            codecs: vec![TransportCodec::H264],
        }
        .encode();
        hello[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert_eq!(
            ReceiverHello::decode(&hello),
            Err(HandshakeError::VersionMismatch {
                peer: 99,
                local: VERSION
            })
        );
    }
    #[test]
    fn fragments_stay_under_mtu_and_keep_header() {
        let payload = vec![7u8; 9000];
        let header = FrameHeader {
            codec: TransportCodec::H264,
            flags: FLAG_KEYFRAME,
            width: 3840,
            height: 2160,
            fps_numerator: 60,
            fps_denominator: 1,
            frame_id: 55,
            timestamp_100ns: 9,
            payload_len: payload.len() as u32,
        };
        let packets = fragments(&header, &payload).unwrap();
        assert!(packets.iter().all(|p| p.len() <= UDP_PAYLOAD));
        let (first, decoded) = parse_fragment(&packets[0]).unwrap();
        assert_eq!(first.frame_id, 55);
        assert_eq!(decoded, Some(header));
    }
}
