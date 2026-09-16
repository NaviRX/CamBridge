use crate::runtime::Shared;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        Media::MediaFoundation::*,
        Storage::FileSystem::{PIPE_ACCESS_OUTBOUND, WriteFile},
        System::Pipes::*,
    },
    core::w,
};

pub struct VirtualCamera {
    camera: IMFVirtualCamera,
    stop: Arc<AtomicBool>,
}
impl VirtualCamera {
    pub fn start(state: Shared) -> Result<Self, String> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| e.to_string())?;
            let camera = match MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                w!("CamBridge"),
                w!("{1483AAF4-E019-46C7-BFDC-322077BA141F}"),
                None,
            ) {
                Ok(v) => v,
                Err(e) => {
                    let _ = MFShutdown();
                    return Err(format!("가상 카메라 설치 확인 필요: {e}"));
                }
            };
            if let Err(e) = camera.Start(None) {
                let _ = MFShutdown();
                return Err(format!("가상 카메라 시작 실패: {e}"));
            }
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let state_v2 = state.clone();
            let stop_v2 = stop.clone();
            thread::spawn(move || pipe_worker(state_v2, stop_v2, true));
            thread::spawn(move || pipe_worker(state, worker_stop, false));
            Ok(Self { camera, stop })
        }
    }
}
impl Drop for VirtualCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        unsafe {
            let _ = self.camera.Stop();
            let _ = self.camera.Shutdown();
            let _ = MFShutdown();
        }
    }
}
fn pipe_worker(state: Shared, stop: Arc<AtomicBool>, v2: bool) {
    unsafe {
        // Legacy DLL accepts fixed-size bottom-up BGRX frames. Network input retains
        // its native size; only this compatibility output is resized to 1080p.
        let pipe = CreateNamedPipeW(
            if v2 {
                w!("\\\\.\\pipe\\CamBridge.Video.v2")
            } else {
                w!("\\\\.\\pipe\\CamBridge.Video.v1")
            },
            PIPE_ACCESS_OUTBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            1920 * 1080 * 4 + 4,
            0,
            0,
            None,
        );
        if pipe == INVALID_HANDLE_VALUE {
            return;
        }
        let mut connected = false;
        let mut last = None;
        while !stop.load(Ordering::Relaxed) {
            if !connected {
                match ConnectNamedPipe(pipe, None) {
                    Ok(()) => connected = true,
                    Err(e)
                        if e.code()
                            == windows::Win32::Foundation::ERROR_PIPE_CONNECTED.to_hresult() =>
                    {
                        connected = true
                    }
                    _ => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                }
            }
            let preview = state.lock().unwrap().preview.clone();
            if let Some(rgb) = preview {
                if last.as_ref().is_some_and(|old| Arc::ptr_eq(old, &rgb)) {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                let scaled = if v2 {
                    (*rgb).clone()
                } else {
                    image::imageops::resize(
                        &*rgb,
                        1920,
                        1080,
                        image::imageops::FilterType::Triangle,
                    )
                };
                let mut bytes =
                    Vec::with_capacity(scaled.width() as usize * scaled.height() as usize * 4 + 12);
                if v2 {
                    bytes.extend_from_slice(&scaled.width().to_le_bytes());
                    bytes.extend_from_slice(&scaled.height().to_le_bytes());
                }
                bytes.extend_from_slice(&(scaled.width() * scaled.height() * 4).to_le_bytes());
                for y in (0..scaled.height() as usize).rev() {
                    for p in scaled.rows().nth(y).unwrap() {
                        bytes.extend_from_slice(&[p[2], p[1], p[0], 0]);
                    }
                }
                let mut offset = 0;
                while offset < bytes.len() && !stop.load(Ordering::Relaxed) {
                    let mut written = 0;
                    match WriteFile(pipe, Some(&bytes[offset..]), Some(&mut written), None) {
                        Ok(()) if written > 0 => offset += written as usize,
                        Ok(()) => thread::sleep(Duration::from_millis(2)),
                        Err(_) => {
                            let _ = DisconnectNamedPipe(pipe);
                            connected = false;
                            break;
                        }
                    }
                }
                last = Some(rgb);
            } else {
                thread::sleep(Duration::from_millis(30));
            }
        }
        let _ = DisconnectNamedPipe(pipe);
        let _ = CloseHandle(pipe);
    }
}
