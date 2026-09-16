use std::{ptr, slice};

use windows::{
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        Media::MediaFoundation::{
            IMFActivate, IMFAttributes, IMFMediaSource, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_E_NO_MORE_TYPES,
            MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_SUBTYPE, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
            MF_VERSION, MFCreateAttributes, MFCreateSourceReaderFromMediaSource,
            MFEnumDeviceSources, MFSTARTUP_FULL, MFShutdown, MFStartup, MFVideoFormat_AV1,
            MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_MJPG, MFVideoFormat_NV12,
            MFVideoFormat_RGB32, MFVideoFormat_YUY2,
        },
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
    },
    core::{GUID, PWSTR},
};

use crate::capability::{
    CaptureDevice, CaptureMode, DeviceKind, FrameRate, PixelFormat, classify_device, exact_modes,
};

pub(crate) struct MfLifetime {
    com_initialized: bool,
}

impl MfLifetime {
    pub(crate) fn start() -> Result<Self, String> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let com_initialized = if result.is_ok() {
            true
        } else if result == RPC_E_CHANGED_MODE {
            false
        } else {
            return Err(format!("COM 초기화 실패: {result}"));
        };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|e| format!("MFStartup: {e}"))?;
        Ok(Self { com_initialized })
    }
}

impl Drop for MfLifetime {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
        if self.com_initialized {
            unsafe { CoUninitialize() };
        }
    }
}

fn allocated_string(attributes: &IMFAttributes, key: &GUID) -> Result<String, String> {
    let mut value = PWSTR::null();
    let mut length = 0u32;
    unsafe { attributes.GetAllocatedString(key, &mut value, &mut length) }
        .map_err(|e| format!("GetAllocatedString: {e}"))?;
    let string = if value.is_null() {
        String::new()
    } else {
        String::from_utf16_lossy(unsafe { slice::from_raw_parts(value.0, length as usize) })
    };
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    Ok(string)
}

fn unpack_pair(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

fn pixel_format(subtype: GUID) -> PixelFormat {
    if subtype == MFVideoFormat_MJPG {
        PixelFormat::Mjpeg
    } else if subtype == MFVideoFormat_H264 {
        PixelFormat::H264
    } else if subtype == MFVideoFormat_HEVC {
        PixelFormat::Hevc
    } else if subtype == MFVideoFormat_AV1 {
        PixelFormat::Av1
    } else if subtype == MFVideoFormat_NV12 {
        PixelFormat::Nv12
    } else if subtype == MFVideoFormat_YUY2 {
        PixelFormat::Yuy2
    } else if subtype == MFVideoFormat_RGB32 {
        PixelFormat::Xrgb8888
    } else {
        PixelFormat::Unknown
    }
}

fn modes_for(activate: &IMFActivate) -> Result<Vec<CaptureMode>, String> {
    let source: IMFMediaSource =
        unsafe { activate.ActivateObject() }.map_err(|e| format!("장치 활성화 실패: {e}"))?;
    let reader = unsafe { MFCreateSourceReaderFromMediaSource(&source, None) }
        .map_err(|e| format!("Source Reader 생성 실패: {e}"))?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut modes = Vec::new();
    for index in 0u32..4096 {
        let media_type = match unsafe { reader.GetNativeMediaType(stream, index) } {
            Ok(value) => value,
            Err(error) if error.code() == MF_E_NO_MORE_TYPES => break,
            Err(error) => {
                let _ = unsafe { source.Shutdown() };
                return Err(format!("네이티브 모드 {index} 조회 실패: {error}"));
            }
        };
        let size = match unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) } {
            Ok(v) => v,
            Err(_) => continue,
        };
        let rate = match unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) } {
            Ok(v) => v,
            Err(_) => continue,
        };
        let subtype = match unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) } {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (width, height) = unpack_pair(size);
        let (numerator, denominator) = unpack_pair(rate);
        if width == 0 || height == 0 || numerator == 0 || denominator == 0 {
            continue;
        }
        let verified_openable =
            unsafe { reader.SetCurrentMediaType(stream, None, &media_type) }.is_ok();
        modes.push(CaptureMode {
            width,
            height,
            fps: FrameRate::new(numerator, denominator),
            format: pixel_format(subtype),
            native_type_index: index,
            verified_openable,
        });
    }
    let _ = unsafe { source.Shutdown() };
    Ok(exact_modes(modes))
}

/// Enumerates all video-capture sources (webcams and capture cards) exposed by
/// Windows Media Foundation, preserving exact native type tuples.
pub fn enumerate_video_devices() -> Result<Vec<CaptureDevice>, String> {
    let _lifetime = MfLifetime::start()?;
    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 1) }
        .map_err(|e| format!("MFCreateAttributes: {e}"))?;
    let attributes = attributes.ok_or("Media Foundation attributes가 null입니다.")?;
    unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
    }
    .map_err(|e| format!("비디오 캡처 필터 설정 실패: {e}"))?;

    let mut raw: *mut Option<IMFActivate> = ptr::null_mut();
    let mut count = 0u32;
    unsafe { MFEnumDeviceSources(&attributes, &mut raw, &mut count) }
        .map_err(|e| format!("비디오 장치 검색 실패: {e}"))?;
    // MF allocates an array of owning COM interface pointers. Move every entry
    // into a Rust Vec before freeing the array itself so each reference is
    // released even when probing a later device fails.
    let mut activates = Vec::with_capacity(count as usize);
    if !raw.is_null() {
        for index in 0..count as usize {
            activates.push(unsafe { ptr::read(raw.add(index)) });
        }
        unsafe { CoTaskMemFree(Some(raw.cast())) };
    }
    let mut devices = Vec::with_capacity(count as usize);
    for activate in activates.iter().flatten() {
        let name = allocated_string(activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)?;
        let symbolic_link = allocated_string(
            activate,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
        )?;
        let kind = classify_device(&name, &symbolic_link);
        let (modes, probe_error) = match modes_for(activate) {
            Ok(modes) => (modes, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        devices.push(CaptureDevice {
            symbolic_link,
            name,
            kind,
            modes,
            probe_error,
        });
    }
    Ok(devices)
}

pub fn conservative_kind(name: &str, symbolic_link: &str) -> DeviceKind {
    classify_device(name, symbolic_link)
}

/// A reader lives entirely on its capture thread; native MJPEG samples are
/// retained unchanged for transport. Other modes request MF's RGB converter.
type CaptureResult = Result<Option<(Vec<u8>, i64, bool)>, String>;
pub struct CaptureSession {
    reader: windows::Win32::Media::MediaFoundation::IMFSourceReader,
    source: IMFMediaSource,
    pub native_jpeg: bool,
    pub native_codec: Option<crate::capability::TransportCodec>,
    pub config: Vec<u8>,
    pub width: u32,
    pub height: u32,
    stride: i32,
    events: std::sync::mpsc::Receiver<CaptureResult>,
    _lifetime: MfLifetime,
}

impl CaptureSession {
    pub fn open(
        link: &str,
        mode: &CaptureMode,
        transport: crate::capability::TransportCodec,
    ) -> Result<Self, String> {
        use windows::Win32::Media::MediaFoundation::*;
        let lifetime = MfLifetime::start()?;
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 3).map_err(|e| e.to_string())?;
            let attrs = attrs.unwrap();
            attrs
                .SetGUID(
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                )
                .map_err(|e| e.to_string())?;
            let link: Vec<u16> = link.encode_utf16().chain(Some(0)).collect();
            attrs
                .SetString(
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                    windows::core::PCWSTR(link.as_ptr()),
                )
                .map_err(|e| e.to_string())?;
            let source = MFCreateDeviceSource(&attrs).map_err(|e| e.to_string())?;
            let mut options = None;
            MFCreateAttributes(&mut options, 1).map_err(|e| e.to_string())?;
            let options = options.unwrap();
            let (tx, events) = std::sync::mpsc::sync_channel(2);
            let callback: IMFSourceReaderCallback = ReaderCallback { tx }.into();
            options
                .SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &callback)
                .map_err(|e| e.to_string())?;
            options
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(|e| e.to_string())?;
            let reader = match MFCreateSourceReaderFromMediaSource(&source, &options) {
                Ok(r) => r,
                Err(e) => {
                    let _ = source.Shutdown();
                    return Err(e.to_string());
                }
            };
            let configure = || -> windows::core::Result<(bool, i32, Option<crate::capability::TransportCodec>,Vec<u8>)> {
                let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
                let native = reader.GetNativeMediaType(stream, mode.native_type_index)?;
                let (w, h) = unpack_pair(native.GetUINT64(&MF_MT_FRAME_SIZE)?);
                let (n, d) = unpack_pair(native.GetUINT64(&MF_MT_FRAME_RATE)?);
                if (w, h, n, d, pixel_format(native.GetGUID(&MF_MT_SUBTYPE)?))
                    != (
                        mode.width,
                        mode.height,
                        mode.fps.numerator,
                        mode.fps.denominator,
                        mode.format,
                    )
                {
                    return Err(windows::core::Error::from_hresult(
                        windows::Win32::Foundation::E_INVALIDARG,
                    ));
                }
                reader.SetCurrentMediaType(stream, None, &native)?;
                let jpeg = mode.format == PixelFormat::Mjpeg;
                let native_codec = crate::capability::TransportCodec::from_input(mode.format).filter(|c|mode.format.compressed()&&(*c==transport||jpeg));
                if native_codec.is_none() {
                    let output = MFCreateMediaType()?;
                    output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
                    reader.SetCurrentMediaType(stream, None, &output)?;
                }
                let current = reader.GetCurrentMediaType(stream)?;
                let stride = current
                    .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                    .unwrap_or(mode.width * 4) as i32;
                let mut config=Vec::new();
                if let Ok(length)=current.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)&& length<=65536{config.resize(length as usize,0);current.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER,&mut config,None)?;}
                Ok((jpeg, stride,native_codec,config))
            };
            match configure() {
                Ok((native_jpeg, stride, native_codec, config)) => Ok(Self {
                    reader,
                    source,
                    native_jpeg,
                    native_codec,
                    config,
                    width: mode.width,
                    height: mode.height,
                    stride,
                    events,
                    _lifetime: lifetime,
                }),
                Err(e) => {
                    let _ = source.Shutdown();
                    Err(format!("입력 모드를 열 수 없습니다: {e}"))
                }
            }
        }
    }

    pub fn read(&self) -> Result<Option<(Vec<u8>, i64, bool)>, String> {
        use windows::Win32::Media::MediaFoundation::*;
        unsafe {
            self.reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    None,
                    None,
                    None,
                )
                .map_err(|e| e.to_string())?;
            let Some((data, timestamp, keyframe)) = self
                .events
                .recv_timeout(std::time::Duration::from_secs(3))
                .map_err(|_| "입력 신호 응답 없음 (3초)".to_string())??
            else {
                return Ok(None);
            };
            let keyframe = keyframe || self.native_jpeg;
            let result = if self.native_codec.is_some() {
                Ok(data.to_vec())
            } else {
                let pitch = self.stride.unsigned_abs() as usize;
                let row = self.width as usize * 4;
                if pitch < row || data.len() < pitch * self.height as usize {
                    Err("잘린 RGB 프레임".into())
                } else {
                    let mut pixels = vec![0; row * self.height as usize];
                    for y in 0..self.height as usize {
                        let sy = if self.stride < 0 {
                            self.height as usize - 1 - y
                        } else {
                            y
                        };
                        pixels[y * row..(y + 1) * row]
                            .copy_from_slice(&data[sy * pitch..sy * pitch + row]);
                    }
                    Ok(pixels)
                }
            };
            result.map(|bytes| Some((bytes, timestamp, keyframe)))
        }
    }
}

use windows::Win32::Media::MediaFoundation::{
    IMFMediaEvent, IMFSample, IMFSourceReaderCallback, IMFSourceReaderCallback_Impl,
};
#[windows::core::implement(IMFSourceReaderCallback)]
struct ReaderCallback {
    tx: std::sync::mpsc::SyncSender<CaptureResult>,
}
impl IMFSourceReaderCallback_Impl for ReaderCallback_Impl {
    fn OnReadSample(
        &self,
        status: windows::core::HRESULT,
        _stream: u32,
        flags: u32,
        timestamp: i64,
        sample: windows::core::Ref<IMFSample>,
    ) -> windows::core::Result<()> {
        use windows::Win32::Media::MediaFoundation::*;
        let result = (|| -> Result<Option<(Vec<u8>, i64, bool)>, String> {
            status.ok().map_err(|e| e.to_string())?;
            if flags
                & (MF_SOURCE_READERF_ERROR.0
                    | MF_SOURCE_READERF_ENDOFSTREAM.0
                    | MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0) as u32
                != 0
            {
                return Err(format!("장치 종료 또는 입력 형식 변경 ({flags:#x})"));
            }
            let Some(sample) = sample.as_ref() else {
                return Ok(None);
            };
            unsafe {
                let buffer = sample
                    .ConvertToContiguousBuffer()
                    .map_err(|e| e.to_string())?;
                let mut p = std::ptr::null_mut();
                let mut length = 0;
                buffer
                    .Lock(&mut p, None, Some(&mut length))
                    .map_err(|e| e.to_string())?;
                if length as usize > crate::protocol::MAX_FRAME_BYTES {
                    let _ = buffer.Unlock();
                    return Err("캡처 프레임 크기 제한 초과".into());
                }
                let bytes = std::slice::from_raw_parts(p, length as usize).to_vec();
                let _ = buffer.Unlock();
                let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
                Ok(Some((bytes, timestamp, key)))
            }
        })();
        let _ = self.tx.try_send(result);
        Ok(())
    }
    fn OnFlush(&self, _stream: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnEvent(
        &self,
        _stream: u32,
        _event: windows::core::Ref<IMFMediaEvent>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        unsafe {
            let _ = self.source.Shutdown();
        }
    }
}
