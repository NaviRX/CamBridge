#![allow(unsafe_op_in_unsafe_fn)]
use crate::{
    capability::{CaptureDevice, TransportCodec, xrgb_bandwidth_bps},
    runtime::{self, Running, SenderSettings, Shared},
    virtual_camera::VirtualCamera,
};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, atomic::Ordering, mpsc},
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            LibraryLoader::GetModuleHandleW,
            Registry::*,
        },
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};
const DEVICE: usize = 101;
const MODE: usize = 102;
const CODEC: usize = 103;
const TARGET: usize = 104;
const SEND: usize = 105;
const RECEIVE: usize = 106;
const STOP: usize = 107;
const REFRESH: usize = 108;
const CAMERA: usize = 109;
const STARTUP: usize = 110;
const QUALITY: usize = 111;
const PROBE: usize = 112;
const BITRATE: usize = 113;
const SCALE: usize = 114;
const FPS: usize = 115;
const TRAY: u32 = WM_APP + 1;
const WM_APP_EXIT: u32 = WM_APP + 2;
struct App {
    window: HWND,
    devices: Vec<CaptureDevice>,
    scan: Option<mpsc::Receiver<Result<Vec<CaptureDevice>, String>>>,
    state: Shared,
    running: Option<Running>,
    camera: Option<VirtualCamera>,
    controls: Vec<(usize, HWND)>,
    status: HWND,
    details: HWND,
    icon: HICON,
    font: HFONT,
    encoders: Vec<crate::mft::EncoderChoice>,
    codec_scan: Option<mpsc::Receiver<Vec<crate::mft::EncoderChoice>>>,
    native_codec: Option<TransportCodec>,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn text(window: HWND, s: &str) {
    let value = wide(s);
    let _ = SetWindowTextW(window, PCWSTR(value.as_ptr()));
}
unsafe fn read(window: HWND) -> String {
    let mut s = vec![0; GetWindowTextLengthW(window) as usize + 1];
    let len = GetWindowTextW(window, &mut s);
    String::from_utf16_lossy(&s[..len as usize])
}
impl App {
    fn control(&self, id: usize) -> HWND {
        self.controls.iter().find(|(key, _)| *key == id).unwrap().1
    }
    unsafe fn selection(&self, id: usize) -> usize {
        SendMessageW(self.control(id), CB_GETCURSEL, None, None)
            .0
            .max(0) as usize
    }
    unsafe fn combo(&self, id: usize, items: &[String]) {
        let h = self.control(id);
        SendMessageW(h, CB_RESETCONTENT, None, None);
        for item in items {
            let s = wide(item);
            SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(s.as_ptr() as isize)));
        }
        SendMessageW(h, CB_SETCURSEL, Some(WPARAM(0)), None);
    }
    unsafe fn scan(&mut self) {
        if self.scan.is_some() || self.running.is_some() {
            return;
        }
        runtime::status(&self.state, "장치와 입력 모드를 확인 중…");
        let (tx, rx) = mpsc::channel();
        self.scan = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(crate::windows_mf::enumerate_video_devices());
        });
    }
    unsafe fn modes(&mut self) {
        if let Some(device) = self.devices.get(self.selection(DEVICE)) {
            self.combo(
                MODE,
                &device.modes.iter().map(|m| m.label()).collect::<Vec<_>>(),
            );
            if let Some(e) = &device.probe_error {
                runtime::status(&self.state, e);
            }
            self.bandwidth();
        }
        self.reset_codecs();
    }
    unsafe fn reset_codecs(&mut self) {
        self.codec_scan = None;
        self.encoders.clear();
        self.native_codec = self
            .devices
            .get(self.selection(DEVICE))
            .and_then(|d| d.modes.get(self.selection(MODE)))
            .and_then(|m| crate::capability::TransportCodec::from_input(m.format))
            .filter(|c| !matches!(c, TransportCodec::Jpeg | TransportCodec::Xrgb8888));
        if self.selection(SCALE) != 0 || self.selection(FPS) != 0 {
            self.native_codec = None;
        }
        self.update_codecs();
    }
    unsafe fn update_codecs(&self) {
        let mut labels = vec![
            "JPEG / MJPEG (CPU 또는 원본)".into(),
            "XRGB8888 원본 · 고대역폭".into(),
        ];
        labels.extend(self.encoders.iter().map(|e| {
            format!(
                "{:?} · {} · {}",
                e.codec,
                if e.hardware { "하드웨어" } else { "CPU" },
                e.name
            )
        }));
        if let Some(codec) = self.native_codec {
            labels.push(format!("{codec:?} · 장치 비트스트림 원본"));
        }
        self.combo(CODEC, &labels);
    }
    unsafe fn bandwidth(&self) {
        if let Some(mode) = self
            .devices
            .get(self.selection(DEVICE))
            .and_then(|d| d.modes.get(self.selection(MODE)))
        {
            let b = xrgb_bandwidth_bps(mode.width, mode.height, mode.fps) as f64 / 1e9;
            text(
                self.details,
                &format!(
                    "입력: {} | XRGB 원본 예상 {:.2} Gbit/s (헤더 제외)",
                    mode.label(),
                    b
                ),
            );
        }
    }
    unsafe fn start(&mut self, send: bool) {
        if self.codec_scan.is_some() {
            runtime::status(&self.state, "코덱 검사 완료 후 시작하세요");
            return;
        }
        if self
            .running
            .as_ref()
            .is_some_and(|r| !r.done.load(Ordering::Relaxed))
        {
            return;
        }
        self.running = None;
        *self.state.lock().unwrap() = Default::default();
        self.state.lock().unwrap().local_camera = self.camera.is_some();
        if send {
            let Some(device) = self.devices.get(self.selection(DEVICE)) else {
                runtime::status(&self.state, "캡처 장치를 선택하세요");
                return;
            };
            let Some(mode) = device.modes.get(self.selection(MODE)) else {
                runtime::status(&self.state, "사용 가능한 입력 모드가 없습니다");
                return;
            };
            if !mode.verified_openable {
                runtime::status(&self.state, "드라이버가 열기를 거부한 입력 모드입니다");
                return;
            }
            let ip = read(self.control(TARGET));
            let target: SocketAddr = match format!("{ip}:45831").parse() {
                Ok(v) => v,
                Err(_) => {
                    runtime::status(&self.state, "수신 PC의 IPv4 주소를 입력하세요");
                    return;
                }
            };
            if !target.ip().is_loopback() && !crate::transport::private_lan(target.ip()) {
                runtime::status(&self.state, "수신 IP는 사설 LAN 주소여야 합니다");
                return;
            }
            let quality = read(self.control(QUALITY))
                .parse::<u8>()
                .unwrap_or(85)
                .clamp(1, 100);
            self.running = Some(runtime::start_sender(
                SenderSettings {
                    link: device.symbolic_link.clone(),
                    mode: mode.clone(),
                    target,
                    codec: if self.selection(CODEC) == 0 {
                        TransportCodec::Jpeg
                    } else if self.selection(CODEC) == 1 {
                        TransportCodec::Xrgb8888
                    } else if self.selection(CODEC) < self.encoders.len() + 2 {
                        self.encoders[self.selection(CODEC) - 2].codec
                    } else {
                        self.native_codec.unwrap_or(TransportCodec::Jpeg)
                    },
                    quality,
                    encoder: self
                        .selection(CODEC)
                        .checked_sub(2)
                        .and_then(|i| self.encoders.get(i))
                        .cloned(),
                    bitrate: read(self.control(BITRATE))
                        .parse::<u32>()
                        .unwrap_or(8)
                        .clamp(1, 200)
                        * 1_000_000,
                    output_divisor: [1, 2, 4][self.selection(SCALE).min(2)],
                    fps_limit: [0, 30, 15][self.selection(FPS).min(2)],
                },
                self.state.clone(),
            ));
            let _ = set_registry("Software\\CamBridge", "ReceiverIP", Some(&ip));
        } else {
            self.running = Some(runtime::start_receiver(self.state.clone()));
        }
        for id in [
            SEND, RECEIVE, REFRESH, DEVICE, MODE, CODEC, TARGET, QUALITY, SCALE, FPS, BITRATE,
            PROBE,
        ] {
            let _ = EnableWindow(self.control(id), false);
        }
    }
    unsafe fn tick(&mut self) {
        if let Some(choices) = self.codec_scan.as_ref().and_then(|r| r.try_recv().ok()) {
            self.codec_scan = None;
            for id in [DEVICE, MODE, SCALE, FPS, PROBE] {
                let _ = EnableWindow(self.control(id), true);
            }
            self.encoders = choices;
            self.update_codecs();
            runtime::status(
                &self.state,
                format!("{}개 인코더 시험 프레임 출력 확인", self.encoders.len()),
            );
        }
        if let Some(result) = self.scan.as_ref().and_then(|r| r.try_recv().ok()) {
            self.scan = None;
            match result {
                Ok(mut devices) => {
                    devices.retain(|d| !d.name.starts_with("CamBridge"));
                    devices.push(runtime::test_device());
                    self.devices = devices;
                    self.combo(
                        DEVICE,
                        &self
                            .devices
                            .iter()
                            .map(|d| d.name.clone())
                            .collect::<Vec<_>>(),
                    );
                    runtime::status(&self.state, format!("{}개 장치 검색됨", self.devices.len()));
                    self.modes();
                }
                Err(e) => {
                    self.devices = vec![runtime::test_device()];
                    self.combo(DEVICE, &[self.devices[0].name.clone()]);
                    self.modes();
                    runtime::status(&self.state, e);
                }
            }
        }
        if self
            .running
            .as_ref()
            .is_some_and(|r| r.done.load(Ordering::Relaxed))
        {
            self.running = None;
            for id in [
                SEND, RECEIVE, REFRESH, DEVICE, MODE, CODEC, TARGET, QUALITY, SCALE, FPS, BITRATE,
                PROBE,
            ] {
                let _ = EnableWindow(self.control(id), true);
            }
        }
        let s = self.state.lock().unwrap();
        text(
            self.status,
            &format!(
                "{}\r\n캡처 {} · 네트워크 인코딩 {} · 송수신 {} · {:.1} MB",
                s.status,
                s.captured,
                s.encoded,
                s.transferred,
                s.bytes as f64 / 1e6
            ),
        );
        drop(s);
        let _ = InvalidateRect(
            Some(self.window),
            Some(&RECT {
                left: 20,
                top: 300,
                right: 780,
                bottom: 740,
            }),
            false,
        );
    }
    unsafe fn draw(&self) {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(self.window, &mut ps);
        let rect = RECT {
            left: 20,
            top: 300,
            right: 780,
            bottom: 728,
        };
        FillRect(dc, &rect, HBRUSH(GetStockObject(BLACK_BRUSH).0));
        let frame = self.state.lock().unwrap().preview.clone();
        if let Some(rgb) = frame {
            let scale = (760.0 / rgb.width() as f64).min(428.0 / rgb.height() as f64);
            let width = (rgb.width() as f64 * scale) as i32;
            let height = (rgb.height() as f64 * scale) as i32;
            let pixels: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 0]).collect();
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: rgb.width() as i32,
                    biHeight: -(rgb.height() as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            StretchDIBits(
                dc,
                20 + (760 - width) / 2,
                300 + (428 - height) / 2,
                width,
                height,
                0,
                0,
                rgb.width() as i32,
                rgb.height() as i32,
                Some(pixels.as_ptr().cast()),
                &info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
        let _ = EndPaint(self.window, &ps);
    }
}
unsafe fn set_registry(path: &str, name: &str, value: Option<&str>) -> Result<(), String> {
    let p = wide(path);
    let n = wide(name);
    let mut key = HKEY::default();
    RegCreateKeyExW(
        HKEY_CURRENT_USER,
        PCWSTR(p.as_ptr()),
        None,
        None,
        REG_OPTION_NON_VOLATILE,
        KEY_SET_VALUE,
        None,
        &mut key,
        None,
    )
    .ok()
    .map_err(|e| e.to_string())?;
    let result = if let Some(value) = value {
        let data = wide(value);
        RegSetValueExW(
            key,
            PCWSTR(n.as_ptr()),
            None,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                data.as_ptr().cast(),
                data.len() * 2,
            )),
        )
        .ok()
    } else {
        RegDeleteValueW(key, PCWSTR(n.as_ptr())).ok()
    };
    let _ = RegCloseKey(key);
    result.map_err(|e| e.to_string())
}
unsafe fn get_registry(path: &str, name: &str) -> Option<String> {
    let p = wide(path);
    let n = wide(name);
    let mut data = [0u16; 2048];
    let mut size = 4096;
    RegGetValueW(
        HKEY_CURRENT_USER,
        PCWSTR(p.as_ptr()),
        PCWSTR(n.as_ptr()),
        RRF_RT_REG_SZ,
        None,
        Some(data.as_mut_ptr().cast()),
        Some(&mut size),
    )
    .ok()
    .ok()?;
    Some(String::from_utf16_lossy(
        &data[..(size as usize / 2).saturating_sub(1)],
    ))
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Win32 synchronously re-enters a window procedure during control updates.
    // Do not create a second mutable App reference in those nested messages.
    thread_local! {static HANDLING:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
    if HANDLING.with(|v| v.get()) {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HANDLING.with(|v| v.set(false));
        }
    }
    HANDLING.with(|v| v.set(true));
    let _reset = Reset;
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *pointer;
    match msg {
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            let notification = (wparam.0 >> 16) & 0xffff;
            match id {
                PROBE => {
                    if app.running.is_none()
                        && app.codec_scan.is_none()
                        && let Some(mode) = app
                            .devices
                            .get(app.selection(DEVICE))
                            .and_then(|d| d.modes.get(app.selection(MODE)))
                            .cloned()
                    {
                        let (tx, rx) = mpsc::channel();
                        app.codec_scan = Some(rx);
                        let divisor = [1, 2, 4][app.selection(SCALE).min(2)];
                        let width = (mode.width / divisor / 2 * 2).max(2);
                        let height = (mode.height / divisor / 2 * 2).max(2);
                        let fps_limit = [0, 30, 15][app.selection(FPS).min(2)];
                        let fps = if fps_limit > 0 && (fps_limit as f64) < mode.fps.as_f64() {
                            crate::capability::FrameRate::new(fps_limit, 1)
                        } else {
                            mode.fps
                        };
                        for id in [DEVICE, MODE, SCALE, FPS, PROBE] {
                            let _ = EnableWindow(app.control(id), false);
                        }
                        runtime::status(&app.state, "선택한 입력 크기로 CPU/GPU 인코더 시험 중…");
                        std::thread::spawn(move || {
                            let _ = tx.send(crate::mft::probe_encoders(width, height, fps));
                        });
                    }
                }
                SEND => app.start(true),
                RECEIVE => app.start(false),
                STOP => {
                    if let Some(r) = &app.running {
                        r.stop();
                        runtime::status(&app.state, "중지 중…");
                    }
                }
                REFRESH => app.scan(),
                DEVICE if notification == CBN_SELCHANGE as usize => app.modes(),
                MODE if notification == CBN_SELCHANGE as usize => {
                    app.reset_codecs();
                    app.bandwidth();
                }
                SCALE | FPS if notification == CBN_SELCHANGE as usize => {
                    app.reset_codecs();
                    app.bandwidth();
                }
                CODEC if notification == CBN_SELCHANGE as usize => app.bandwidth(),
                CAMERA => {
                    if app.camera.is_some() {
                        app.camera = None;
                        app.state.lock().unwrap().local_camera = false;
                        runtime::status(&app.state, "가상 카메라 끔");
                    } else {
                        match VirtualCamera::start(app.state.clone()) {
                            Ok(v) => {
                                app.camera = Some(v);
                                app.state.lock().unwrap().local_camera = true;
                                runtime::status(
                                    &app.state,
                                    "가상 카메라 켬 · OBS에서 CamBridge 선택",
                                );
                            }
                            Err(e) => {
                                runtime::status(&app.state, e);
                                SendMessageW(
                                    app.control(CAMERA),
                                    BM_SETCHECK,
                                    Some(WPARAM(0)),
                                    None,
                                );
                            }
                        }
                    }
                }
                STARTUP => {
                    let checked =
                        SendMessageW(app.control(STARTUP), BM_GETCHECK, None, None).0 != 0;
                    let value = std::env::current_exe()
                        .map(|p| format!("\"{}\" --tray", p.display()))
                        .unwrap_or_default();
                    if let Err(e) = set_registry(
                        "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                        "CamBridge",
                        if checked { Some(&value) } else { None },
                    ) {
                        runtime::status(&app.state, e);
                    }
                }
                201 => {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = SetForegroundWindow(hwnd);
                }
                202 => {
                    let _ = PostMessageW(Some(hwnd), WM_APP + 2, WPARAM(0), LPARAM(0));
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            app.tick();
            LRESULT(0)
        }
        WM_PAINT => {
            app.draw();
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONDBLCLK => {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = SetForegroundWindow(hwnd);
                }
                WM_RBUTTONUP => {
                    if let Ok(menu) = CreatePopupMenu() {
                        let _ = AppendMenuW(menu, MF_STRING, 201, w!("설정 열기"));
                        let _ = AppendMenuW(menu, MF_STRING, 202, w!("종료"));
                        let mut pt = POINT::default();
                        let _ = GetCursorPos(&mut pt);
                        let _ = SetForegroundWindow(hwnd);
                        let selected = TrackPopupMenu(
                            menu,
                            TPM_RIGHTBUTTON | TPM_RETURNCMD,
                            pt.x,
                            pt.y,
                            None,
                            hwnd,
                            None,
                        );
                        if selected.0 > 0 {
                            let _ = PostMessageW(
                                Some(hwnd),
                                WM_COMMAND,
                                WPARAM(selected.0 as usize),
                                LPARAM(0),
                            );
                        }
                        let _ = DestroyMenu(menu);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_EXIT | WM_DESTROY => {
            if let Some(r) = &app.running {
                r.stop();
            }
            app.camera = None;
            let data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            if msg == WM_APP_EXIT {
                let _ = DestroyWindow(hwnd);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
pub fn run() -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let instance = HINSTANCE(GetModuleHandleW(None).map_err(|e| e.to_string())?.0);
        let icon_path = std::env::current_exe()
            .unwrap()
            .with_file_name("CamBridge.ico");
        let path = wide(&icon_path.to_string_lossy());
        let icon = LoadImageW(
            None,
            PCWSTR(path.as_ptr()),
            IMAGE_ICON,
            32,
            32,
            LR_LOADFROMFILE,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap());
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            lpszClassName: w!("CamBridgeRustWindow"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap(),
            hIcon: icon,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut _),
            ..Default::default()
        };
        RegisterClassW(&class);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class.lpszClassName,
            w!("CamBridge Rust — 네트워크 카메라"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            818,
            845,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| e.to_string())?;
        let font = CreateFontW(
            -18,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Malgun Gothic"),
        );
        let mut app = Box::new(App {
            window,
            devices: vec![],
            scan: None,
            state: Arc::new(Mutex::new(Default::default())),
            running: None,
            camera: None,
            controls: vec![],
            status: HWND::default(),
            details: HWND::default(),
            icon,
            font,
            encoders: vec![],
            codec_scan: None,
            native_codec: None,
        });
        let mut add = |id: usize,
                       class: &str,
                       label: &str,
                       x: i32,
                       y: i32,
                       width: i32,
                       height: i32,
                       style: u32|
         -> HWND {
            let c = wide(class);
            let label = wide(label);
            let h = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(c.as_ptr()),
                PCWSTR(label.as_ptr()),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                x,
                y,
                width,
                height,
                Some(window),
                Some(HMENU(id as *mut _)),
                Some(instance),
                None,
            )
            .unwrap();
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            if id != 0 {
                app.controls.push((id, h));
            }
            h
        };
        add(0, "STATIC", "입력 장치", 20, 20, 110, 24, 0);
        add(
            DEVICE,
            "COMBOBOX",
            "",
            140,
            16,
            520,
            350,
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0,
        );
        add(REFRESH, "BUTTON", "새로 검색", 670, 16, 110, 30, 0);
        add(0, "STATIC", "입력 모드", 20, 60, 110, 24, 0);
        add(
            MODE,
            "COMBOBOX",
            "",
            140,
            56,
            640,
            400,
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0,
        );
        add(0, "STATIC", "전송 코덱", 20, 100, 110, 24, 0);
        add(
            CODEC,
            "COMBOBOX",
            "",
            140,
            96,
            280,
            160,
            CBS_DROPDOWNLIST as u32,
        );
        add(0, "STATIC", "JPEG 품질", 440, 100, 110, 24, 0);
        add(
            QUALITY,
            "EDIT",
            "85",
            560,
            96,
            60,
            28,
            WS_BORDER.0 | ES_NUMBER as u32,
        );
        add(0, "STATIC", "수신 IP", 20, 140, 110, 24, 0);
        add(
            TARGET,
            "EDIT",
            &get_registry("Software\\CamBridge", "ReceiverIP").unwrap_or("192.168.0.2".into()),
            140,
            136,
            280,
            28,
            WS_BORDER.0,
        );
        add(SEND, "BUTTON", "송신 시작", 440, 136, 100, 30, 0);
        add(RECEIVE, "BUTTON", "수신 시작", 550, 136, 100, 30, 0);
        add(STOP, "BUTTON", "중지", 660, 136, 120, 30, 0);
        add(
            CAMERA,
            "BUTTON",
            "가상 카메라 / OBS 호환 모드",
            20,
            178,
            360,
            30,
            BS_AUTOCHECKBOX as u32,
        );
        add(
            STARTUP,
            "BUTTON",
            "Windows 시작 시 실행",
            430,
            178,
            350,
            30,
            BS_AUTOCHECKBOX as u32,
        );
        add(PROBE, "BUTTON", "코덱 검사", 640, 96, 140, 28, 0);
        add(0, "STATIC", "인코딩 비트레이트 (Mbps)", 20, 736, 250, 24, 0);
        add(
            BITRATE,
            "EDIT",
            "8",
            280,
            732,
            80,
            25,
            WS_BORDER.0 | ES_NUMBER as u32,
        );
        app.details = add(0, "STATIC", "", 20, 218, 760, 24, 0);
        add(0, "STATIC", "출력 크기", 20, 780, 110, 24, 0);
        add(
            SCALE,
            "COMBOBOX",
            "",
            140,
            776,
            230,
            130,
            CBS_DROPDOWNLIST as u32,
        );
        add(0, "STATIC", "송신 FPS", 400, 780, 110, 24, 0);
        add(
            FPS,
            "COMBOBOX",
            "",
            510,
            776,
            270,
            130,
            CBS_DROPDOWNLIST as u32,
        );
        app.status = add(0, "STATIC", "", 20, 248, 760, 48, 0);
        app.combo(
            CODEC,
            &[
                "JPEG / MJPEG (CPU 또는 원본)".into(),
                "XRGB8888 원본 · 고대역폭".into(),
            ],
        );
        app.combo(
            SCALE,
            &[
                "입력과 동일".into(),
                "입력의 1/2 크기".into(),
                "입력의 1/4 크기".into(),
            ],
        );
        app.combo(
            FPS,
            &[
                "입력과 동일".into(),
                "최대 30 (입력 이하)".into(),
                "최대 15 (입력 이하)".into(),
            ],
        );
        if get_registry(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            "CamBridge",
        )
        .is_some()
        {
            SendMessageW(app.control(STARTUP), BM_SETCHECK, Some(WPARAM(1)), None);
        }
        SetWindowLongPtrW(window, GWLP_USERDATA, (&mut *app as *mut App) as isize);
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: TRAY,
            hIcon: icon,
            ..Default::default()
        };
        let tip = wide("CamBridge · 네트워크 카메라");
        data.szTip[..tip.len()].copy_from_slice(&tip);
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
        SetTimer(Some(window), 1, 100, None);
        app.scan();
        if !std::env::args().any(|a| a == "--tray") {
            let _ = ShowWindow(window, SW_SHOW);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        let _ = DeleteObject(HGDIOBJ(app.font.0));
        let _ = DestroyIcon(app.icon);
        drop(app);
        CoUninitialize();
        Ok(())
    }
}
