#![allow(unsafe_op_in_unsafe_fn)]
use crate::{
    capability::{FrameRate, TransportCodec},
    windows_mf::MfLifetime,
};
use std::{
    mem::ManuallyDrop,
    ptr,
    time::{Duration, Instant},
};
use windows::{
    Win32::{Media::MediaFoundation::*, System::Com::CoTaskMemFree},
    core::{GUID, Interface},
};

#[derive(Clone, Debug)]
pub struct EncoderChoice {
    pub name: String,
    pub codec: TransportCodec,
    pub hardware: bool,
    pub ordinal: usize,
}
pub struct Packet {
    pub bytes: Vec<u8>,
    pub timestamp: i64,
    pub keyframe: bool,
    pub config: Vec<u8>,
}
pub fn subtype(codec: TransportCodec) -> GUID {
    match codec {
        TransportCodec::H264 => MFVideoFormat_H264,
        TransportCodec::Hevc => MFVideoFormat_HEVC,
        TransportCodec::Av1 => MFVideoFormat_AV1,
        _ => MFVideoFormat_MJPG,
    }
}
unsafe fn activations(
    codec: TransportCodec,
    encode: bool,
    hardware: bool,
) -> Result<Vec<IMFActivate>, String> {
    let info = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype(codec),
    };
    let mut raw = ptr::null_mut();
    let mut count = 0;
    MFTEnumEx(
        if encode {
            MFT_CATEGORY_VIDEO_ENCODER
        } else {
            MFT_CATEGORY_VIDEO_DECODER
        },
        if hardware {
            MFT_ENUM_FLAG_HARDWARE
        } else {
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT
        },
        if encode { None } else { Some(&info) },
        if encode { Some(&info) } else { None },
        &mut raw,
        &mut count,
    )
    .map_err(|e| e.to_string())?;
    let mut list = Vec::new();
    if !raw.is_null() {
        for index in 0..count as usize {
            if let Some(a) = ptr::read(raw.add(index)) {
                list.push(a);
            }
        }
        CoTaskMemFree(Some(raw.cast()));
    }
    Ok(list)
}
unsafe fn media(
    format: GUID,
    w: u32,
    h: u32,
    fps: FrameRate,
) -> windows::core::Result<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, &format)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
    t.SetUINT64(
        &MF_MT_FRAME_RATE,
        ((fps.numerator as u64) << 32) | fps.denominator as u64,
    )?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    Ok(t)
}
pub struct Transform {
    mft: IMFTransform,
    event: Option<IMFMediaEventGenerator>,
    input_ready: u32,
    output_ready: u32,
    pub width: u32,
    pub height: u32,
    fps: FrameRate,
    encode: bool,
    _lifetime: MfLifetime,
}
impl Transform {
    pub fn encoder(
        choice: &EncoderChoice,
        w: u32,
        h: u32,
        fps: FrameRate,
        bitrate: u32,
    ) -> Result<Self, String> {
        Self::open(
            choice.codec,
            true,
            choice.hardware,
            choice.ordinal,
            w,
            h,
            fps,
            bitrate,
            &[],
        )
    }
    pub fn decoder(
        codec: TransportCodec,
        w: u32,
        h: u32,
        fps: FrameRate,
        config: &[u8],
    ) -> Result<Self, String> {
        let mut last = String::new();
        for hardware in [true, false] {
            let lifetime = MfLifetime::start()?;
            let count = unsafe { activations(codec, false, hardware) }?.len();
            drop(lifetime);
            for ordinal in 0..count {
                match Self::open(codec, false, hardware, ordinal, w, h, fps, 0, config) {
                    Ok(t) => return Ok(t),
                    Err(e) => last = e,
                }
            }
        }
        Err(format!("사용 가능한 {codec:?} 디코더 없음: {last}"))
    }
    #[allow(clippy::too_many_arguments)]
    fn open(
        codec: TransportCodec,
        encode: bool,
        hardware: bool,
        ordinal: usize,
        w: u32,
        h: u32,
        fps: FrameRate,
        bitrate: u32,
        config: &[u8],
    ) -> Result<Self, String> {
        let lifetime = MfLifetime::start()?;
        unsafe {
            let choices = activations(codec, encode, hardware)?;
            let a = choices.get(ordinal).ok_or("코덱이 변경되었습니다")?;
            let mft: IMFTransform = a.ActivateObject().map_err(|e| e.to_string())?;
            let configure = || -> windows::core::Result<Option<IMFMediaEventGenerator>> {
                let attrs = mft.GetAttributes()?;
                let asynchronous = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
                if asynchronous {
                    attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                }
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
                if let Ok(api) = mft.cast::<ICodecAPI>() {
                    use windows::Win32::System::Variant::VARIANT;
                    let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true));
                    if encode {
                        let _ = api.SetValue(
                            &CODECAPI_AVEncMPVGOPSize,
                            &VARIANT::from(fps.as_f64().ceil() as u32),
                        );
                    }
                }
                let compressed = media(subtype(codec), w, h, fps)?;
                if encode {
                    compressed.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
                    if codec == TransportCodec::H264 {
                        compressed.SetUINT32(&MF_MT_MPEG2_PROFILE, 77)?;
                    }
                }
                if !config.is_empty() {
                    compressed.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, config)?;
                }
                let raw = media(MFVideoFormat_NV12, w, h, fps)?;
                raw.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)?;
                if encode {
                    mft.SetOutputType(0, &compressed, 0)?;
                    mft.SetInputType(0, &raw, 0)?;
                } else {
                    mft.SetInputType(0, &compressed, 0)?;
                    mft.SetOutputType(0, &raw, 0)?;
                }
                mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
                if asynchronous {
                    Ok(Some(mft.cast()?))
                } else {
                    Ok(None)
                }
            };
            let event = configure().map_err(|e| e.to_string())?;
            Ok(Self {
                mft,
                event,
                input_ready: 0,
                output_ready: 0,
                width: w,
                height: h,
                fps,
                encode,
                _lifetime: lifetime,
            })
        }
    }
    unsafe fn events(&mut self) {
        if let Some(event) = &self.event {
            while let Ok(e) = event.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                match e.GetType().unwrap_or(0) {
                    v if v == METransformNeedInput.0 as u32 => self.input_ready += 1,
                    v if v == METransformHaveOutput.0 as u32 => self.output_ready += 1,
                    _ => {}
                }
            }
        }
    }
    pub fn submit(&mut self, data: &[u8], timestamp: i64) -> Result<Vec<Packet>, String> {
        unsafe {
            self.events();
            let mut packets = self.output()?;
            if self.event.is_some() {
                let end = Instant::now() + Duration::from_millis(250);
                while self.input_ready == 0 {
                    self.events();
                    packets.extend(self.output()?);
                    if Instant::now() > end {
                        return Err("코덱 입력 대기 시간 초과".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            let sample = MFCreateSample().map_err(|e| e.to_string())?;
            let buffer = MFCreateMemoryBuffer(data.len() as u32).map_err(|e| e.to_string())?;
            let mut p = ptr::null_mut();
            buffer.Lock(&mut p, None, None).map_err(|e| e.to_string())?;
            ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            buffer.Unlock().map_err(|e| e.to_string())?;
            buffer
                .SetCurrentLength(data.len() as u32)
                .map_err(|e| e.to_string())?;
            sample.AddBuffer(&buffer).map_err(|e| e.to_string())?;
            sample.SetSampleTime(timestamp).map_err(|e| e.to_string())?;
            sample
                .SetSampleDuration(
                    10_000_000i64 * self.fps.denominator as i64 / self.fps.numerator.max(1) as i64,
                )
                .map_err(|e| e.to_string())?;
            self.mft
                .ProcessInput(0, &sample, 0)
                .map_err(|e| e.to_string())?;
            if self.event.is_some() {
                self.input_ready -= 1;
            }
            packets.extend(self.output()?);
            Ok(packets)
        }
    }
    pub fn force_keyframe(&self) {
        unsafe {
            if let Ok(api) = self.mft.cast::<ICodecAPI>() {
                let _ = api.SetValue(
                    &CODECAPI_AVEncVideoForceKeyFrame,
                    &windows::Win32::System::Variant::VARIANT::from(1u32),
                );
            }
        }
    }
    pub fn output(&mut self) -> Result<Vec<Packet>, String> {
        unsafe {
            self.events();
            let mut result = Vec::new();
            for _ in 0..32 {
                if self.event.is_some() && self.output_ready == 0 {
                    break;
                }
                let info = self.mft.GetOutputStreamInfo(0).map_err(|e| e.to_string())?;
                let sample = if info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0 {
                    None
                } else {
                    let s = MFCreateSample().map_err(|e| e.to_string())?;
                    let b = MFCreateMemoryBuffer(if info.cbSize>0{info.cbSize}else{self.width*self.height*4})
                        .map_err(|e| e.to_string())?;
                    s.AddBuffer(&b).map_err(|e| e.to_string())?;
                    Some(s)
                };
                let mut output = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(sample),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0;
                let processed = self.mft.ProcessOutput(0, &mut output, &mut status);
                let sample = ManuallyDrop::take(&mut output[0].pSample);
                let _events = ManuallyDrop::take(&mut output[0].pEvents);
                match processed {
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        let mut selected = false;
                        for i in 0..64 {
                            let Ok(t) = self.mft.GetOutputAvailableType(0, i) else {
                                break;
                            };
                            if t.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                                self.mft
                                    .SetOutputType(0, &t, 0)
                                    .map_err(|e| e.to_string())?;
                                selected = true;
                                break;
                            }
                        }
                        if !selected {
                            return Err("디코더 NV12 출력 협상 실패".into());
                        }
                        continue;
                    }
                    Err(e) => return Err(e.to_string()),
                    Ok(()) => {}
                }
                if self.event.is_some() {
                    self.output_ready = self.output_ready.saturating_sub(1);
                }
                if let Some(sample) = sample {
                    let buffer = sample
                        .ConvertToContiguousBuffer()
                        .map_err(|e| e.to_string())?;
                    let mut p = ptr::null_mut();
                    let mut len = 0;
                    buffer
                        .Lock(&mut p, None, Some(&mut len))
                        .map_err(|e| e.to_string())?;
                    let mut bytes = std::slice::from_raw_parts(p, len as usize).to_vec();
                    buffer.Unlock().map_err(|e| e.to_string())?;
                    if !self.encode&&let Ok(surface)=buffer.cast::<IMF2DBuffer>(){
                        let size=surface.GetContiguousLength().map_err(|e|e.to_string())? as usize;
                        if size>crate::protocol::MAX_FRAME_BYTES{return Err("디코더 프레임 크기 초과".into());}
                        bytes.resize(size,0);surface.ContiguousCopyTo(&mut bytes).map_err(|e|e.to_string())?;
                    }
                    let mut config = Vec::new();
                    if self.encode
                        && let Ok(t) = self.mft.GetOutputCurrentType(0)
                        && let Ok(size) = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)
                        && size < 65536
                    {
                        config.resize(size as usize, 0);
                        let _ = t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut config, None);
                    }
                    result.push(Packet {
                        bytes,
                        timestamp: sample.GetSampleTime().unwrap_or(0),
                        keyframe: sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0,
                        config,
                    });
                }
            }
            Ok(result)
        }
    }
}
impl Drop for Transform {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(shutdown) = self.mft.cast::<IMFShutdown>() {
                let _ = shutdown.Shutdown();
            }
        }
    }
}
pub fn probe_encoders(w: u32, h: u32, fps: FrameRate) -> Vec<EncoderChoice> {
    let Ok(_lifetime) = MfLifetime::start() else {
        return vec![];
    };
    let mut supported = Vec::new();
    for codec in [
        TransportCodec::H264,
        TransportCodec::Hevc,
        TransportCodec::Av1,
    ] {
        for hardware in [false, true] {
            let Ok(list) = (unsafe { activations(codec, true, hardware) }) else {
                continue;
            };
            for (ordinal, activation) in list.iter().enumerate() {
                let name = unsafe {
                    let mut p = windows::core::PWSTR::null();
                    let mut n = 0;
                    let text = if activation
                        .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut n)
                        .is_ok()
                    {
                        String::from_utf16_lossy(std::slice::from_raw_parts(p.0, n as usize))
                    } else {
                        format!("{codec:?}")
                    };
                    CoTaskMemFree(Some(p.0.cast()));
                    text
                };
                let choice = EncoderChoice {
                    name,
                    codec,
                    hardware,
                    ordinal,
                };
                if let Ok(mut encoder) = Transform::encoder(&choice, w, h, fps, 8_000_000) {
                    let mut input = vec![128; w as usize * h as usize * 3 / 2];
                    input[..w as usize * h as usize].fill(64);
                    let mut produced = false;
                    for i in 0..8 {
                        match encoder.submit(
                            &input,
                            i * 10_000_000 * fps.denominator as i64 / fps.numerator.max(1) as i64,
                        ) {
                            Ok(p) if !p.is_empty() => {
                                produced = true;
                                break;
                            }
                            Ok(_) => std::thread::sleep(Duration::from_millis(10)),
                            Err(_) => break,
                        }
                    }
                    if produced {
                        supported.push(choice);
                    }
                }
            }
        }
    }
    supported
}
pub fn rgb_to_nv12(rgb: &image::RgbImage) -> Vec<u8> {
    let (w, h) = rgb.dimensions();
    let mut data = vec![0; w as usize * h as usize * 3 / 2];
    for y in 0..h {
        for x in 0..w {
            let p = rgb.get_pixel(x, y);
            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
            data[(y * w + x) as usize] =
                ((66 * r + 129 * g + 25 * b + 128) / 256 + 16).clamp(0, 255) as u8;
            if x % 2 == 0 && y % 2 == 0 && x + 1 < w && y + 1 < h {
                let i = (w * h + (y / 2) * w + x) as usize;
                data[i] = ((-38 * r - 74 * g + 112 * b + 128) / 256 + 128).clamp(0, 255) as u8;
                data[i + 1] = ((112 * r - 94 * g - 18 * b + 128) / 256 + 128).clamp(0, 255) as u8;
            }
        }
    }
    data
}
pub fn nv12_to_rgb(data: &[u8], w: u32, h: u32) -> Result<image::RgbImage, String> {
    if !w.is_multiple_of(2) || !h.is_multiple_of(2) || data.len() != w as usize * h as usize * 3 / 2
    {
        return Err("디코더 NV12 크기 불일치".into());
    }
    let mut rgb = image::RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let yy = data[(y * w + x) as usize] as i32 - 16;
            let i = (w * h + (y / 2) * w + (x / 2) * 2) as usize;
            let u = data[i] as i32 - 128;
            let v = data[i + 1] as i32 - 128;
            rgb.put_pixel(
                x,
                y,
                image::Rgb([
                    ((298 * yy + 409 * v + 128) >> 8).clamp(0, 255) as u8,
                    ((298 * yy - 100 * u - 208 * v + 128) >> 8).clamp(0, 255) as u8,
                    ((298 * yy + 516 * u + 128) >> 8).clamp(0, 255) as u8,
                ]),
            );
        }
    }
    Ok(rgb)
}
