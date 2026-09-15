use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PixelFormat {
    Mjpeg = 1,
    H264 = 2,
    Hevc = 3,
    Nv12 = 4,
    Yuy2 = 5,
    Xrgb8888 = 6,
    Av1 = 7,
    Unknown = 255,
}

impl PixelFormat {
    pub fn compressed(self) -> bool {
        matches!(self, Self::Mjpeg | Self::H264 | Self::Hevc | Self::Av1)
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mjpeg => "MJPEG",
            Self::H264 => "H.264",
            Self::Hevc => "H.265/HEVC",
            Self::Nv12 => "NV12",
            Self::Yuy2 => "YUY2",
            Self::Xrgb8888 => "XRGB8888",
            Self::Av1 => "AV1",
            Self::Unknown => "알 수 없음",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRate {
    pub numerator: u32,
    pub denominator: u32,
}

impl FrameRate {
    pub const fn new(numerator: u32, denominator: u32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    pub fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator.max(1) as f64
    }

    pub fn at_most(self, other: Self) -> bool {
        (self.numerator as u64 * other.denominator as u64)
            <= (other.numerator as u64 * self.denominator as u64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CaptureMode {
    pub width: u32,
    pub height: u32,
    pub fps: FrameRate,
    pub format: PixelFormat,
    pub native_type_index: u32,
    pub verified_openable: bool,
}

impl CaptureMode {
    pub fn label(&self) -> String {
        format!(
            "{}×{} {:.3} fps {}{}",
            self.width,
            self.height,
            self.fps.as_f64(),
            self.format,
            if self.verified_openable {
                " ✓"
            } else {
                " (미검증)"
            }
        )
    }

    pub fn allows_output(&self, width: u32, height: u32, fps: FrameRate) -> bool {
        width <= self.width && height <= self.height && fps.at_most(self.fps)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Webcam,
    CaptureDevice,
    VideoCaptureDevice,
}

#[derive(Debug, Clone)]
pub struct CaptureDevice {
    pub symbolic_link: String,
    pub name: String,
    pub kind: DeviceKind,
    pub modes: Vec<CaptureMode>,
    pub probe_error: Option<String>,
}

pub fn classify_device(name: &str, symbolic_link: &str) -> DeviceKind {
    let haystack = format!("{name} {symbolic_link}").to_ascii_lowercase();
    const CAPTURE_HINTS: &[&str] = &[
        "capture",
        "cam link",
        "elgato",
        "avermedia",
        "hdmi",
        "decklink",
    ];
    const CAMERA_HINTS: &[&str] = &["webcam", "camera", "facecam", "brio", "c920", "c922"];
    if CAPTURE_HINTS.iter().any(|hint| haystack.contains(hint)) {
        DeviceKind::CaptureDevice
    } else if CAMERA_HINTS.iter().any(|hint| haystack.contains(hint)) {
        DeviceKind::Webcam
    } else {
        // Media Foundation does not expose a normative webcam-vs-capture-card property.
        DeviceKind::VideoCaptureDevice
    }
}

pub fn exact_modes(mut advertised: Vec<CaptureMode>) -> Vec<CaptureMode> {
    advertised.sort_by_key(|m| {
        (
            m.width,
            m.height,
            m.fps.numerator,
            m.fps.denominator,
            m.format as u8,
            m.native_type_index,
        )
    });
    advertised.dedup_by(|a, b| {
        a.width == b.width && a.height == b.height && a.fps == b.fps && a.format == b.format
    });
    advertised
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportCodec {
    Jpeg,
    H264,
    Hevc,
    Av1,
    Xrgb8888,
}

impl TransportCodec {
    pub fn from_input(format: PixelFormat) -> Option<Self> {
        match format {
            PixelFormat::Mjpeg => Some(Self::Jpeg),
            PixelFormat::H264 => Some(Self::H264),
            PixelFormat::Hevc => Some(Self::Hevc),
            PixelFormat::Av1 => Some(Self::Av1),
            PixelFormat::Xrgb8888 => Some(Self::Xrgb8888),
            _ => None,
        }
    }

    pub const fn id(self) -> u8 {
        match self {
            Self::Jpeg => 1,
            Self::H264 => 2,
            Self::Hevc => 3,
            Self::Av1 => 4,
            Self::Xrgb8888 => 5,
        }
    }

    pub fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            1 => Self::Jpeg,
            2 => Self::H264,
            3 => Self::Hevc,
            4 => Self::Av1,
            5 => Self::Xrgb8888,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceleration {
    Cpu,
    D3d11Hardware,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecCapability {
    pub codec: TransportCodec,
    pub acceleration: Acceleration,
    pub encoder_name: String,
    pub can_encode: bool,
    pub can_decode: bool,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
}

pub fn available_encoders(
    capabilities: &[CodecCapability],
    codec: TransportCodec,
) -> Vec<&CodecCapability> {
    capabilities
        .iter()
        .filter(|c| c.codec == codec && c.can_encode)
        .collect()
}

pub fn xrgb_bandwidth_bps(width: u32, height: u32, fps: FrameRate) -> u64 {
    let bytes_per_frame = width as u128 * height as u128 * 4;
    let bytes_per_second = bytes_per_frame * fps.numerator as u128 / fps.denominator.max(1) as u128;
    (bytes_per_second.saturating_mul(8)).min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_modes_never_synthesizes_4k60() {
        let modes = exact_modes(vec![
            CaptureMode {
                width: 3840,
                height: 2160,
                fps: FrameRate::new(30, 1),
                format: PixelFormat::Nv12,
                native_type_index: 0,
                verified_openable: true,
            },
            CaptureMode {
                width: 1920,
                height: 1080,
                fps: FrameRate::new(60, 1),
                format: PixelFormat::Nv12,
                native_type_index: 1,
                verified_openable: true,
            },
        ]);
        assert!(
            !modes
                .iter()
                .any(|m| m.width == 3840 && m.fps == FrameRate::new(60, 1))
        );
    }

    #[test]
    fn output_is_capped_by_selected_input() {
        let input = CaptureMode {
            width: 1920,
            height: 1080,
            fps: FrameRate::new(30, 1),
            format: PixelFormat::Mjpeg,
            native_type_index: 0,
            verified_openable: true,
        };
        assert!(input.allows_output(1280, 720, FrameRate::new(30, 1)));
        assert!(!input.allows_output(1920, 1080, FrameRate::new(60, 1)));
        assert!(!input.allows_output(3840, 2160, FrameRate::new(30, 1)));
    }

    #[test]
    fn xrgb_4k60_warns_at_about_sixteen_gbps() {
        let bps = xrgb_bandwidth_bps(3840, 2160, FrameRate::new(60, 1));
        assert!(bps > 15_000_000_000 && bps < 17_000_000_000);
    }
}
