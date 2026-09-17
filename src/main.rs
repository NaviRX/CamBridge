#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use cambridge::{
    capability::{FrameRate, xrgb_bandwidth_bps},
    protocol::VERSION,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None | Some("--tray") => {
            if let Err(error) = cambridge::desktop::run() {
                eprintln!("{error}");
            }
        }
        Some("--protocol-version") => println!("{VERSION}"),
        Some("--probe-codecs") => {
            for choice in cambridge::mft::probe_encoders(640, 480, FrameRate::new(30, 1)) {
                println!("{choice:?}");
            }
        }
        Some("--xrgb-bandwidth") if args.len() == 5 => {
            let w: u32 = args[2].parse().expect("width");
            let h: u32 = args[3].parse().expect("height");
            let fps: u32 = args[4].parse().expect("fps");
            let bps = xrgb_bandwidth_bps(w, h, FrameRate::new(fps, 1));
            println!(
                "{:.2} Gbit/s ({:.2} GB/s)",
                bps as f64 / 1e9,
                bps as f64 / 8e9
            );
        }
        Some("--list-devices") =>
        {
            #[cfg(target_os = "windows")]
            match cambridge::windows_mf::enumerate_video_devices() {
                Ok(devices) => {
                    for device in devices {
                        println!("{} [{:?}]", device.name, device.kind);
                        if let Some(error) = device.probe_error {
                            println!("  열기 확인 실패: {error}");
                        }
                        for mode in device.modes {
                            println!("  {}", mode.label());
                        }
                    }
                }
                Err(error) => {
                    eprintln!("Media Foundation 장치 검색 실패: {error}");
                    std::process::exit(2);
                }
            }
        }
        _ => {
            println!(
                "CamBridge Rust v{} — protocol v{}",
                env!("CARGO_PKG_VERSION"),
                VERSION
            );
            println!("  --list-devices");
            println!("  --protocol-version");
            println!("  --xrgb-bandwidth WIDTH HEIGHT FPS");
        }
    }
}
