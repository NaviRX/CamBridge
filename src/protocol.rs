use crate::capability::TransportCodec;

pub const MAGIC: [u8; 4] = *b"CBR2";
pub const VERSION: u16 = 2;
pub const HEADER_LEN: usize = 48;
pub const FLAG_KEYFRAME: u16 = 1;
pub const FLAG_CONFIG: u16 = 2;
pub const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    pub codec: TransportCodec,
    pub flags: u16,
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub frame_id: u64,
    pub timestamp_100ns: i64,
    pub payload_len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    TooShort,
    BadMagic,
    VersionMismatch(u16),
    UnsupportedCodec(u8),
    InvalidDimensions,
    FrameTooLarge,
}

impl FrameHeader {
    pub fn encode(&self) -> Result<[u8; HEADER_LEN], ProtocolError> {
        if self.width == 0
            || self.height == 0
            || self.width > 8192
            || self.height > 8192
            || self.fps_numerator == 0
            || self.fps_denominator == 0
        {
            return Err(ProtocolError::InvalidDimensions);
        }
        if self.payload_len as usize > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge);
        }
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&MAGIC);
        out[4..6].copy_from_slice(&VERSION.to_le_bytes());
        out[6] = self.codec.id();
        out[8..10].copy_from_slice(&self.flags.to_le_bytes());
        out[12..16].copy_from_slice(&self.width.to_le_bytes());
        out[16..20].copy_from_slice(&self.height.to_le_bytes());
        out[20..24].copy_from_slice(&self.fps_numerator.to_le_bytes());
        out[24..28].copy_from_slice(&self.fps_denominator.to_le_bytes());
        out[28..36].copy_from_slice(&self.frame_id.to_le_bytes());
        out[36..44].copy_from_slice(&self.timestamp_100ns.to_le_bytes());
        out[44..48].copy_from_slice(&self.payload_len.to_le_bytes());
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < HEADER_LEN {
            return Err(ProtocolError::TooShort);
        }
        if bytes[0..4] != MAGIC {
            return Err(ProtocolError::BadMagic);
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if version != VERSION {
            return Err(ProtocolError::VersionMismatch(version));
        }
        let codec =
            TransportCodec::from_id(bytes[6]).ok_or(ProtocolError::UnsupportedCodec(bytes[6]))?;
        let value = Self {
            codec,
            flags: u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
            width: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            height: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            fps_numerator: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            fps_denominator: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            frame_id: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
            timestamp_100ns: i64::from_le_bytes(bytes[36..44].try_into().unwrap()),
            payload_len: u32::from_le_bytes(bytes[44..48].try_into().unwrap()),
        };
        if value.width == 0
            || value.height == 0
            || value.width > 8192
            || value.height > 8192
            || value.fps_numerator == 0
            || value.fps_denominator == 0
        {
            return Err(ProtocolError::InvalidDimensions);
        }
        if value.payload_len as usize > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge);
        }
        Ok(value)
    }

    pub fn keyframe(&self) -> bool {
        self.flags & FLAG_KEYFRAME != 0
    }
    pub fn codec_config(&self) -> bool {
        self.flags & FLAG_CONFIG != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_keeps_codec_timing_and_keyframe() {
        let h = FrameHeader {
            codec: TransportCodec::Hevc,
            flags: FLAG_KEYFRAME | FLAG_CONFIG,
            width: 3840,
            height: 2160,
            fps_numerator: 60000,
            fps_denominator: 1001,
            frame_id: 42,
            timestamp_100ns: 7_654_321,
            payload_len: 9_000_000,
        };
        assert_eq!(FrameHeader::decode(&h.encode().unwrap()).unwrap(), h);
    }
    #[test]
    fn mismatched_version_is_explicit() {
        let mut bytes = FrameHeader {
            codec: TransportCodec::Jpeg,
            flags: 0,
            width: 1,
            height: 1,
            fps_numerator: 1,
            fps_denominator: 1,
            frame_id: 0,
            timestamp_100ns: 0,
            payload_len: 0,
        }
        .encode()
        .unwrap();
        bytes[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert_eq!(
            FrameHeader::decode(&bytes),
            Err(ProtocolError::VersionMismatch(99))
        );
    }
}
