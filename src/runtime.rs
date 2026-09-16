use crate::{
    capability::{CaptureMode, TransportCodec},
    protocol::{FLAG_KEYFRAME, FrameHeader, VERSION},
    reassembly::Reassembler,
    transport::{ReceiverHello, VIDEO_PORT, fragments, parse_fragment},
    windows_mf::CaptureSession,
};
use image::{RgbImage, codecs::jpeg::JpegEncoder};
use std::{
    collections::HashMap,
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct LiveState {
    pub status: String,
    pub preview: Option<Arc<RgbImage>>,
    pub captured: u64,
    pub encoded: u64,
    pub transferred: u64,
    pub bytes: u64,
    pub receiver: bool,
}
pub type Shared = Arc<Mutex<LiveState>>;
pub fn status(state: &Shared, text: impl Into<String>) {
    state.lock().unwrap().status = text.into();
}
pub struct Running {
    pub stop: Arc<AtomicBool>,
    pub done: Arc<AtomicBool>,
}
impl Running {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
pub struct SenderSettings {
    pub link: String,
    pub mode: CaptureMode,
    pub target: SocketAddr,
    pub codec: TransportCodec,
    pub quality: u8,
    pub encoder: Option<crate::mft::EncoderChoice>,
    pub bitrate: u32,
    pub output_divisor: u32,
    pub fps_limit: u32,
}
impl SenderSettings {
    pub fn output(&self) -> (u32, u32, crate::capability::FrameRate) {
        let divisor = self.output_divisor.clamp(1, 4);
        let w = (self.mode.width / divisor / 2 * 2).max(2);
        let h = (self.mode.height / divisor / 2 * 2).max(2);
        let fps = if self.fps_limit > 0 && (self.fps_limit as f64) < self.mode.fps.as_f64() {
            crate::capability::FrameRate::new(self.fps_limit, 1)
        } else {
            self.mode.fps
        };
        (w, h, fps)
    }
}

struct Source {
    camera: Option<CaptureSession>,
    width: u32,
    height: u32,
    native_jpeg: bool,
    native_codec: Option<TransportCodec>,
    config: Vec<u8>,
    started: Instant,
    fps: f64,
}
impl Source {
    fn open(settings: &SenderSettings) -> Result<Self, String> {
        let (w, h, fps) = settings.output();
        let capture_codec = if (w, h, fps)
            != (settings.mode.width, settings.mode.height, settings.mode.fps)
            && settings.encoder.is_some()
        {
            TransportCodec::Xrgb8888
        } else {
            settings.codec
        };
        let camera = if settings.link == "cambridge:test-pattern" {
            None
        } else {
            Some(CaptureSession::open(
                &settings.link,
                &settings.mode,
                capture_codec,
            )?)
        };
        Ok(Self {
            width: settings.mode.width,
            height: settings.mode.height,
            native_jpeg: camera.as_ref().is_some_and(|c| c.native_jpeg),
            native_codec: camera.as_ref().and_then(|c| c.native_codec),
            config: camera
                .as_ref()
                .map(|c| c.config.clone())
                .unwrap_or_default(),
            camera,
            started: Instant::now(),
            fps: settings.mode.fps.as_f64(),
        })
    }
    fn read(&self) -> Result<Option<(Vec<u8>, i64, bool)>, String> {
        if let Some(camera) = &self.camera {
            return camera.read();
        }
        thread::sleep(Duration::from_secs_f64(1.0 / self.fps.max(1.0)));
        let phase = (self.started.elapsed().as_millis() / 10) as u32;
        let mut data = vec![0; self.width as usize * self.height as usize * 4];
        for y in 0..self.height {
            for x in 0..self.width {
                let i = ((y * self.width + x) * 4) as usize;
                data[i] = (x.wrapping_add(phase) % 256) as u8;
                data[i + 1] = (y % 256) as u8;
                data[i + 2] = 180;
            }
        }
        Ok(Some((
            data,
            (self.started.elapsed().as_nanos() / 100) as i64,
            true,
        )))
    }
}
pub fn test_device() -> crate::capability::CaptureDevice {
    use crate::capability::*;
    CaptureDevice {
        symbolic_link: "cambridge:test-pattern".into(),
        name: "테스트 패턴 (실제 장치 아님)".into(),
        kind: DeviceKind::VideoCaptureDevice,
        probe_error: None,
        modes: vec![CaptureMode {
            width: 640,
            height: 480,
            fps: FrameRate::new(30, 1),
            format: PixelFormat::Xrgb8888,
            native_type_index: 0,
            verified_openable: true,
        }],
    }
}

pub fn jpeg_decode(data: &[u8]) -> Result<RgbImage, String> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(data), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map(|i| i.into_rgb8())
        .map_err(|e| e.to_string())
}
pub fn rgb_from_xrgb(data: &[u8], w: u32, h: u32) -> Result<RgbImage, String> {
    if w == 0 || h == 0 || w > 8192 || h > 8192 || data.len() != w as usize * h as usize * 4 {
        return Err("잘못된 XRGB 프레임 크기".into());
    }
    let rgb = data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect();
    RgbImage::from_raw(w, h, rgb).ok_or("잘못된 RGB 프레임".into())
}
pub fn encode_jpeg(rgb: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    JpegEncoder::new_with_quality(&mut data, quality)
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    Ok(data)
}

pub fn start_sender(settings: SenderSettings, state: Shared) -> Running {
    spawn(move |stop| sender(settings, state, stop))
}
fn spawn(work: impl FnOnce(Arc<AtomicBool>) + Send + 'static) -> Running {
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let d = done.clone();
    thread::spawn(move || {
        work(s);
        d.store(true, Ordering::Relaxed);
    });
    Running { stop, done }
}
fn sender(settings: SenderSettings, state: Shared, stop: Arc<AtomicBool>) {
    let (output_width, output_height, output_fps) = settings.output();
    if !settings
        .mode
        .allows_output(output_width, output_height, output_fps)
    {
        status(&state, "출력은 선택한 입력 모드를 넘을 수 없습니다");
        return;
    }
    let passthrough_size =
        output_width == settings.mode.width && output_height == settings.mode.height;
    if settings.encoder.is_none()
        && matches!(
            settings.codec,
            TransportCodec::H264 | TransportCodec::Hevc | TransportCodec::Av1
        )
        && (!passthrough_size || output_fps != settings.mode.fps)
    {
        status(
            &state,
            "압축 원본 전달은 원본 크기와 FPS를 선택하세요. 변환은 인코더가 필요합니다",
        );
        return;
    }
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            status(&state, e.to_string());
            return;
        }
    };
    if let Err(e) = socket.set_nonblocking(true) {
        status(&state, e.to_string());
        return;
    }
    let mut id = 0;
    let mut last_ping = Instant::now() - Duration::from_secs(2);
    let mut seen = None;
    while !stop.load(Ordering::Relaxed) {
        status(&state, "장치 연결 중…");
        let session = match Source::open(&settings) {
            Ok(s) => s,
            Err(e) => {
                status(&state, format!("장치 열기 실패 · 2초 후 재시도: {e}"));
                sleep_stop(&stop, Duration::from_secs(2));
                continue;
            }
        };
        let mut last_frame = Instant::now();
        let mut next_timestamp = i64::MIN;
        let mut encoder: Option<crate::mft::Transform> = None;
        let mut last_keyframe = Instant::now() - Duration::from_secs(2);
        let mut local_decoder: Option<crate::mft::Transform> = None;
        let mut last_preview = Instant::now() - Duration::from_secs(1);
        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now.duration_since(last_ping) > Duration::from_secs(1) {
                let _ = socket.send_to(b"CBP2", settings.target);
                last_ping = now;
            }
            let mut ack = [0u8; 1024];
            while let Ok((n, peer)) = socket.recv_from(&mut ack) {
                if peer == settings.target {
                    if n==4&&&ack[..4]==b"CBK2"{if let Some(e)=&encoder{e.force_keyframe();}continue;}
                    match ReceiverHello::decode(&ack[..n]) {
                        Ok(h) if h.codecs.contains(&settings.codec) => seen = Some(now),
                        Ok(_) => status(&state, "수신자가 선택한 전송 코덱을 지원하지 않습니다"),
                        Err(e) => status(&state, format!("프로토콜 오류: {e:?}")),
                    }
                }
            }
            let active = seen.is_some_and(|t| now.duration_since(t) < Duration::from_secs(3));
            state.lock().unwrap().receiver = active;
            let (data, timestamp, keyframe) = match session.read() {
                Ok(Some(f)) => f,
                Ok(None) => {
                    if last_frame.elapsed() > Duration::from_secs(3) {
                        status(&state, "프레임 없음 · 입력 신호 재연결 중");
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => {
                    status(&state, format!("장치/신호 변경 · 재연결: {e}"));
                    break;
                }
            };
            last_frame = Instant::now();
            state.lock().unwrap().captured += 1;
            if session
                .native_codec
                .is_some_and(|c| c != TransportCodec::Jpeg)
            {
                if active {
                    id += 1;
                    let mut payload = Vec::with_capacity(data.len() + session.config.len() + 4);
                    payload.extend_from_slice(&(session.config.len() as u32).to_le_bytes());
                    payload.extend_from_slice(&session.config);
                    payload.extend_from_slice(&data);
                    let header = FrameHeader {
                        codec: settings.codec,
                        flags: if keyframe { FLAG_KEYFRAME } else { 0 },
                        width: session.width,
                        height: session.height,
                        fps_numerator: settings.mode.fps.numerator,
                        fps_denominator: settings.mode.fps.denominator,
                        frame_id: id,
                        timestamp_100ns: timestamp,
                        payload_len: payload.len() as u32,
                    };
                    if send_packet(&socket, settings.target, &header, &payload).is_ok() {
                        let mut s = state.lock().unwrap();
                        s.transferred += 1;
                        s.bytes += payload.len() as u64;
                        s.status = format!("{:?} 압축 원본 전달 · 재인코딩 없음", settings.codec);
                    }
                }
                // Decode a separate copy only for local preview / OBS output.
                if local_decoder.is_none() && keyframe {
                    local_decoder = crate::mft::Transform::decoder(
                        settings.codec,
                        session.width,
                        session.height,
                        settings.mode.fps,
                        &session.config,
                    )
                    .ok();
                }
                if let Some(decoder) = &mut local_decoder {
                    match decoder.submit(&data, timestamp) {
                        Ok(frames) => {
                            if let Some(frame) = frames.last()
                                && let Ok(rgb) = crate::mft::nv12_to_rgb(
                                    &frame.bytes,
                                    session.width,
                                    session.height,
                                )
                            {
                                state.lock().unwrap().preview = Some(Arc::new(rgb));
                            }
                        }
                        Err(_) => local_decoder = None,
                    }
                }
                continue;
            }
            let needs_preview = last_preview.elapsed() > Duration::from_millis(66);
            let needs_rgb = needs_preview
                || (active
                    && !(session.native_jpeg
                        && settings.codec == TransportCodec::Jpeg
                        && passthrough_size));
            let rgb = if needs_rgb {
                match if session.native_jpeg {
                    jpeg_decode(&data)
                } else {
                    rgb_from_xrgb(&data, session.width, session.height)
                } {
                    Ok(f) => Some(Arc::new(f)),
                    Err(e) => {
                        status(&state, e);
                        continue;
                    }
                }
            } else {
                None
            };
            if needs_preview {
                state.lock().unwrap().preview = rgb.clone();
                last_preview = Instant::now();
            }
            if !active {
                encoder = None;
                status(&state, "수신자 대기 · 로컬 프리뷰 동작 · 송신 인코딩 0");
                continue;
            }
            if let Some(choice) = &settings.encoder {
                if timestamp < next_timestamp {
                    continue;
                }
                next_timestamp = timestamp
                    + 10_000_000i64 * output_fps.denominator as i64
                        / output_fps.numerator.max(1) as i64;
                let rgb = if !passthrough_size {
                    rgb.map(|r| {
                        Arc::new(image::imageops::resize(
                            &*r,
                            output_width,
                            output_height,
                            image::imageops::FilterType::Triangle,
                        ))
                    })
                } else {
                    rgb
                };
                if encoder.is_none() {
                    match crate::mft::Transform::encoder(
                        choice,
                        output_width,
                        output_height,
                        output_fps,
                        settings.bitrate,
                    ) {
                        Ok(value) => encoder = Some(value),
                        Err(e) => {
                            status(&state, format!("인코더 시작 실패: {e}"));
                            return;
                        }
                    }
                }
                state.lock().unwrap().encoded += 1;
                if last_keyframe.elapsed() >= Duration::from_secs(1) {
                    encoder.as_ref().unwrap().force_keyframe();
                    last_keyframe = Instant::now();
                }
                let pixels = crate::mft::rgb_to_nv12(rgb.as_ref().unwrap());
                match encoder.as_mut().unwrap().submit(&pixels, timestamp) {
                    Ok(packets) => {
                        for packet in packets {
                            id += 1;
                            let mut payload =
                                Vec::with_capacity(4 + packet.config.len() + packet.bytes.len());
                            payload.extend_from_slice(&(packet.config.len() as u32).to_le_bytes());
                            payload.extend_from_slice(&packet.config);
                            payload.extend_from_slice(&packet.bytes);
                            let header = FrameHeader {
                                codec: settings.codec,
                                flags: if packet.keyframe { FLAG_KEYFRAME } else { 0 },
                                width: output_width,
                                height: output_height,
                                fps_numerator: output_fps.numerator,
                                fps_denominator: output_fps.denominator,
                                frame_id: id,
                                timestamp_100ns: packet.timestamp,
                                payload_len: payload.len() as u32,
                            };
                            if let Err(e) = send_packet(&socket, settings.target, &header, &payload)
                            {
                                status(&state, e.to_string());
                            } else {
                                let mut s = state.lock().unwrap();
                                s.transferred += 1;
                                s.bytes += payload.len() as u64;
                                s.status =
                                    format!("송신 중 · {:?} · {}", settings.codec, choice.name);
                            }
                        }
                    }
                    Err(e) => {
                        status(&state, format!("인코더 오류: {e}"));
                        break;
                    }
                }
                continue;
            }
            if timestamp < next_timestamp {
                continue;
            }
            next_timestamp = timestamp
                + 10_000_000i64 * output_fps.denominator as i64
                    / output_fps.numerator.max(1) as i64;
            let rgb = if !passthrough_size {
                rgb.map(|r| {
                    Arc::new(image::imageops::resize(
                        &*r,
                        output_width,
                        output_height,
                        image::imageops::FilterType::Triangle,
                    ))
                })
            } else {
                rgb
            };
            let payload = match settings.codec {
                TransportCodec::Jpeg if session.native_jpeg && passthrough_size => data,
                TransportCodec::Jpeg => {
                    state.lock().unwrap().encoded += 1;
                    match encode_jpeg(rgb.as_ref().unwrap(), settings.quality) {
                        Ok(b) => b,
                        Err(e) => {
                            status(&state, e);
                            continue;
                        }
                    }
                }
                TransportCodec::Xrgb8888 => rgb
                    .as_ref()
                    .unwrap()
                    .pixels()
                    .flat_map(|p| [p[2], p[1], p[0], 0])
                    .collect(),
                _ => {
                    status(&state, "이 빌드에 연결되지 않은 전송 코덱");
                    return;
                }
            };
            id += 1;
            let header = FrameHeader {
                codec: settings.codec,
                flags: FLAG_KEYFRAME,
                width: output_width,
                height: output_height,
                fps_numerator: output_fps.numerator,
                fps_denominator: output_fps.denominator,
                frame_id: id,
                timestamp_100ns: timestamp,
                payload_len: payload.len() as u32,
            };
            match fragments(&header, &payload).and_then(|packets| {
                for packet in packets {
                    socket.send_to(&packet, settings.target)?;
                }
                Ok(())
            }) {
                Ok(()) => {
                    let mut s = state.lock().unwrap();
                    s.transferred += 1;
                    s.bytes += payload.len() as u64;
                    s.status = format!(
                        "송신 중 · {} · {}×{} · {}",
                        settings.target,
                        session.width,
                        session.height,
                        if session.native_jpeg && settings.codec == TransportCodec::Jpeg {
                            "MJPEG 원본 전달"
                        } else {
                            "변환 전송"
                        }
                    );
                }
                Err(e) => status(&state, format!("네트워크 전송 오류: {e}")),
            }
        }
        sleep_stop(&stop, Duration::from_secs(1));
    }
    status(&state, "중지됨");
}
fn send_packet(
    socket: &UdpSocket,
    target: SocketAddr,
    header: &FrameHeader,
    payload: &[u8],
) -> std::io::Result<()> {
    for packet in fragments(header, payload)? {
        socket.send_to(&packet, target)?;
    }
    Ok(())
}
fn sleep_stop(stop: &AtomicBool, duration: Duration) {
    let end = Instant::now() + duration;
    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(50));
    }
}
pub fn start_receiver(state: Shared) -> Running {
    spawn(move |stop| receiver(state, stop))
}
fn receiver(state: Shared, stop: Arc<AtomicBool>) {
    let socket = match UdpSocket::bind(("0.0.0.0", VIDEO_PORT)) {
        Ok(s) => s,
        Err(e) => {
            status(&state, format!("수신 포트 {VIDEO_PORT} 열기 실패: {e}"));
            return;
        }
    };
    receive_socket(state, stop, socket, true);
}
fn receive_socket(state: Shared, stop: Arc<AtomicBool>, socket: UdpSocket, probe: bool) {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
    let mut codecs = vec![TransportCodec::Jpeg, TransportCodec::Xrgb8888];
    for codec in if probe {
        vec![
            TransportCodec::H264,
            TransportCodec::Hevc,
            TransportCodec::Av1,
        ]
    } else {
        vec![]
    } {
        if crate::mft::Transform::decoder(
            codec,
            640,
            480,
            crate::capability::FrameRate::new(30, 1),
            &[],
        )
        .is_ok()
        {
            codecs.push(codec);
        }
    }
    let hello = ReceiverHello {
        version: VERSION,
        codecs,
    }
    .encode();
    let mut assembler = Reassembler::default();
    let mut headers: HashMap<u64, (FrameHeader, Instant)> = HashMap::new();
    let mut packet = [0u8; 65536];
    let mut peer = None;
    let mut last_frame = Instant::now();
    let mut latest_id = None;
    let mut decoder: Option<(TransportCodec, u32, u32, crate::mft::Transform)> = None;
    let mut waiting_keyframe = true;
    status(
        &state,
        format!("수신 대기 · UDP {VIDEO_PORT} · 송신 앱에 이 PC의 IP 입력"),
    );
    while !stop.load(Ordering::Relaxed) {
        let (n, from) = match socket.recv_from(&mut packet) {
            Ok(v) => v,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if last_frame.elapsed() > Duration::from_secs(3) {
                    state.lock().unwrap().receiver = false;
                    status(&state, "영상 없음 · 송신자/입력 신호 대기");
                }
                continue;
            }
            Err(e) => {
                status(&state, e.to_string());
                break;
            }
        };
        if !from.ip().is_loopback() && !crate::transport::private_lan(from.ip()) {
            continue;
        }
        if n == 4 && &packet[..4] == b"CBP2" {
            if peer.is_none() || peer == Some(from) || last_frame.elapsed() > Duration::from_secs(3)
            {
                if peer != Some(from) {
                    assembler = Reassembler::default();
                    headers.clear();
                    latest_id = None;
                    decoder=None;waiting_keyframe=true;
                }
                peer = Some(from);
                let _ = socket.send_to(&hello, from);
            }
            continue;
        }
        if peer != Some(from) {
            continue;
        }
        let (fragment, header) = match parse_fragment(&packet[..n]) {
            Ok(v) => v,
            Err(e) => {
                status(&state, e.to_string());
                continue;
            }
        };
        let now = Instant::now();
        headers.retain(|_, (_, t)| now.duration_since(*t) < Duration::from_millis(750));
        if let Some(h) = header {
            if headers.len() > 6 {
                headers.clear();
            }
            headers.insert(fragment.frame_id, (h, now));
        }
        if let Ok(Some(data)) = assembler.push(
            fragment.frame_id,
            fragment.index,
            fragment.count,
            fragment.expected_frame_bytes as usize,
            fragment.data,
            now,
        ) {
            let Some((h, _)) = headers.remove(&fragment.frame_id) else {
                continue;
            };
            if latest_id.is_some_and(|id| h.frame_id <= id) {
                continue;
            }
            let rgb = match h.codec {
                TransportCodec::Jpeg => jpeg_decode(&data),
                TransportCodec::Xrgb8888 => rgb_from_xrgb(&data, h.width, h.height),
                codec => {
                    if data.len() < 4 {
                        continue;
                    }
                    let length = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
                    if length > 65536 || length + 4 > data.len() {
                        status(&state, "잘못된 코덱 설정 정보");
                        continue;
                    }
                    if decoder
                        .as_ref()
                        .is_none_or(|(c, w, hg, _)| (*c, *w, *hg) != (codec, h.width, h.height))
                    {
                        decoder = None;
                        waiting_keyframe = true;
                    }
                    if latest_id.is_some_and(|id| h.frame_id > id + 1) {
                        waiting_keyframe = true;
                        decoder = None;
                    }
                    if waiting_keyframe && !h.keyframe() {
                        let _=socket.send_to(b"CBK2",from);
                        continue;
                    }
                    if decoder.is_none() {
                        match crate::mft::Transform::decoder(
                            codec,
                            h.width,
                            h.height,
                            crate::capability::FrameRate::new(h.fps_numerator, h.fps_denominator),
                            &data[4..4 + length],
                        ) {
                            Ok(d) => decoder = Some((codec, h.width, h.height, d)),
                            Err(e) => {
                                status(&state, e);
                                continue;
                            }
                        }
                    }
                    waiting_keyframe = false;
                    match decoder
                        .as_mut()
                        .unwrap()
                        .3
                        .submit(&data[4 + length..], h.timestamp_100ns)
                    {
                        Ok(frames) => {
                            let Some(frame) = frames.last() else {
                                latest_id = Some(h.frame_id);
                                continue;
                            };
                            crate::mft::nv12_to_rgb(&frame.bytes, h.width, h.height)
                        }
                        Err(e) => {
                            decoder = None;
                            waiting_keyframe = true;
                            Err(e)
                        }
                    }
                }
            };
            match rgb {
                Ok(rgb) if rgb.width() == h.width && rgb.height() == h.height => {
                    latest_id = Some(h.frame_id);
                    last_frame = now;
                    let mut s = state.lock().unwrap();
                    s.preview = Some(Arc::new(rgb));
                    s.transferred += 1;
                    s.bytes += data.len() as u64;
                    s.receiver = true;
                    s.status = format!(
                        "수신 중 · {from} · {}×{} · {:?}",
                        h.width, h.height, h.codec
                    );
                }
                Ok(_) => status(&state, "JPEG 크기가 프로토콜과 일치하지 않습니다"),
                Err(e) => status(&state, e),
            }
        }
    }
    status(&state, "중지됨");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jpeg_actual_round_trip() {
        let image = RgbImage::from_pixel(64, 48, image::Rgb([190, 40, 15]));
        let jpeg = encode_jpeg(&image, 90).unwrap();
        let decoded = jpeg_decode(&jpeg).unwrap();
        assert_eq!(decoded.dimensions(), (64, 48));
        assert!(decoded.get_pixel(20, 20)[0] > 180);
    }
    #[test]
    fn xrgb_rejects_truncated_frame() {
        assert!(rgb_from_xrgb(&[0; 3], 1, 1).is_err());
    }
    #[test]
    fn live_udp_jpeg_and_no_receiver_gating() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = socket.local_addr().unwrap();
        let tx: Shared = Arc::new(Mutex::new(Default::default()));
        let rx: Shared = Arc::new(Mutex::new(Default::default()));
        let mode = CaptureMode {
            width: 64,
            height: 48,
            ..test_device().modes.remove(0)
        };
        let sender = start_sender(
            SenderSettings {
                link: "cambridge:test-pattern".into(),
                mode,
                target,
                codec: TransportCodec::Jpeg,
                quality: 85,
                encoder: None,
                bitrate: 8_000_000,
                output_divisor: 1,
                fps_limit: 0,
            },
            tx.clone(),
        );
        thread::sleep(Duration::from_millis(200));
        assert!(tx.lock().unwrap().captured > 0);
        assert_eq!(tx.lock().unwrap().encoded, 0);
        assert!(tx.lock().unwrap().preview.is_some());
        let receiving = rx.clone();
        let receiver = spawn(move |stop| receive_socket(receiving, stop, socket, false));
        let deadline = Instant::now() + Duration::from_secs(4);
        while rx.lock().unwrap().transferred == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        sender.stop();
        receiver.stop();
        assert!(
            rx.lock().unwrap().transferred > 0,
            "{}",
            rx.lock().unwrap().status
        );
        assert!(tx.lock().unwrap().encoded > 0);
        assert_eq!(
            rx.lock().unwrap().preview.as_ref().unwrap().dimensions(),
            (64, 48)
        );
    }
}
