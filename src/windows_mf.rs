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

struct MfLifetime {
    com_initialized: bool,
}

impl MfLifetime {
    fn start() -> Result<Self, String> {
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
