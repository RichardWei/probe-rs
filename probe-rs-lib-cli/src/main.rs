use std::env;
use std::ffi::{CStr, CString, c_char};
use std::io::{self, Write};
use std::path::PathBuf;

use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

// English comments: minimal CLI using libloading to call probe_rs_lib.dll

#[derive(Clone, Copy)]
enum Protocol {
    Auto,
    Swd,
    Jtag,
}

type ProgressCb = unsafe extern "C" fn(i32, f32, *const c_char, i32);

struct Ffi {
    pr_last_error: unsafe extern "C" fn(*mut c_char, usize) -> usize,
    pr_probe_count: unsafe extern "C" fn() -> u32,
    pr_probe_info: unsafe extern "C" fn(
        u32,
        *mut c_char,
        usize,
        *mut u16,
        *mut u16,
        *mut c_char,
        usize,
    ) -> i32,
    pr_probe_features: unsafe extern "C" fn(u32, *mut u32, *mut u32) -> i32,
    pr_probe_check_target: unsafe extern "C" fn(u32) -> i32,
    pr_session_open_auto: unsafe extern "C" fn(*const c_char, u32, i32) -> u64,
    pr_session_open_with_probe: unsafe extern "C" fn(*const c_char, *const c_char, u32, i32) -> u64,
    pr_session_close: unsafe extern "C" fn(u64) -> i32,
    pr_set_progress_callback: unsafe extern "C" fn(ProgressCb),
    pr_clear_progress_callback: unsafe extern "C" fn(),
    // removed unused getters to eliminate dead_code warnings and keep CLI lean
    pr_flash_auto: unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        u64,
        u32,
        i32,
        i32,
        i32,
        u32,
        i32,
    ) -> i32,
    pr_set_programmer_type_code: unsafe extern "C" fn(i32) -> i32,
    pr_programmer_type_is_supported_code: unsafe extern "C" fn(i32) -> i32,
    pr_programmer_type_from_string: unsafe extern "C" fn(*const c_char, *mut i32) -> i32,
}

fn load_ffi(dll_path: &str) -> Ffi {
    unsafe {
        let dll_c = CString::new(dll_path).unwrap();
        let h = LoadLibraryA(dll_c.as_ptr() as *const u8);
        if h.is_null() {
            panic!("LoadLibraryA failed");
        }
        let load = |name: &str| {
            let name_c = CString::new(name).unwrap();
            let p = GetProcAddress(h, name_c.as_ptr() as *const u8);
            if p.is_none() {
                panic!("GetProcAddress failed for {}", name);
            }
            p.unwrap()
        };
        Ffi {
            pr_last_error: std::mem::transmute(load("pr_last_error")),
            pr_probe_count: std::mem::transmute(load("pr_probe_count")),
            pr_probe_info: std::mem::transmute(load("pr_probe_info")),
            pr_probe_features: std::mem::transmute(load("pr_probe_features")),
            pr_probe_check_target: std::mem::transmute(load("pr_probe_check_target")),
            pr_session_open_auto: std::mem::transmute(load("pr_session_open_auto")),
            pr_session_open_with_probe: std::mem::transmute(load("pr_session_open_with_probe")),
            pr_session_close: std::mem::transmute(load("pr_session_close")),
            pr_set_progress_callback: std::mem::transmute(load("pr_set_progress_callback")),
            pr_clear_progress_callback: std::mem::transmute(load("pr_clear_progress_callback")),
            pr_flash_auto: std::mem::transmute(load("pr_flash_auto")),
            pr_set_programmer_type_code: std::mem::transmute(load("pr_set_programmer_type_code")),
            pr_programmer_type_is_supported_code: std::mem::transmute(load(
                "pr_programmer_type_is_supported_code",
            )),
            pr_programmer_type_from_string: std::mem::transmute(load(
                "pr_programmer_type_from_string",
            )),
        }
    }
}

fn print_last_error(ffi: &Ffi) {
    unsafe {
        let need = (ffi.pr_last_error)(std::ptr::null_mut(), 0);
        if need > 0 {
            let mut buf = vec![0u8; need];
            (ffi.pr_last_error)(buf.as_mut_ptr() as *mut c_char, buf.len());
            eprintln!(
                "probe-rs-lib error: {}",
                String::from_utf8_lossy(&buf[..need - 1])
            );
        }
    }
}

// English comments: split parsing into a testable function; keep public API unchanged
fn parse_args_from<I: Iterator<Item = String>>(
    mut args: I,
) -> (
    Option<String>,
    Option<String>,
    Option<PathBuf>,
    Protocol,
    u32,
    Option<String>,
    Option<u64>,
    String,
    bool,
    bool,
    bool,
    Option<String>,
) {
    // English comments: very simple argument parser without external crates
    let mut chip = None;
    let mut probe = None;
    let mut file = None;
    let mut protocol = Protocol::Auto;
    let mut speed = 4000u32;
    let mut op = None; // list|check|flash
    let mut base = None; // for bin
    let mut dll_hint = String::new();
    let mut verify = true;
    let mut preverify = false;
    let mut chip_erase = true;
    let mut programmer_type: Option<String> = None;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--chip" => chip = args.next(),
            "--probe" => probe = args.next(),
            "--file" => file = args.next().map(PathBuf::from),
            "--protocol" => match args.next().as_deref() {
                Some("swd") => protocol = Protocol::Swd,
                Some("jtag") => protocol = Protocol::Jtag,
                _ => protocol = Protocol::Auto,
            },
            "--speed" => speed = args.next().and_then(|v| v.parse().ok()).unwrap_or(speed),
            "--format" => {
                let _ = args.next(); /* deprecated: ignored */
            }
            "--op" => op = args.next(),
            "--base" => {
                base = args.next().and_then(|v| {
                    let s = v.trim();
                    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                        u64::from_str_radix(hex, 16).ok()
                    } else if let Some(bin) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B"))
                    {
                        u64::from_str_radix(bin, 2).ok()
                    } else if let Some(oct) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O"))
                    {
                        u64::from_str_radix(oct, 8).ok()
                    } else {
                        u64::from_str_radix(s, 10).ok()
                    }
                });
            }
            "--dll" => dll_hint = args.next().unwrap_or_default(),
            "--programmer-type" => programmer_type = args.next(),
            "--verify" => verify = true,
            "--no-verify" => verify = false,
            "--preverify" => preverify = true,
            "--no-preverify" => preverify = false,
            "--chip-erase" => chip_erase = true,
            "--no-chip-erase" => chip_erase = false,
            "--help" => {
                println!(
                    "Usage: --chip <name> --programmer-type <type> [--probe VID:PID[:SERIAL]] [--file <path>] [--protocol swd|jtag] [--speed KHZ] [--op list|check|flash] [--base 0xADDR] [--dll <path>] [--verify|--no-verify] [--preverify|--no-preverify] [--chip-erase|--no-chip-erase]\nSupported programmer types: cmsis-dap, stlink, jlink, ftdi, esp-usb-jtag, wch-link, sifli-uart, glasgow, ch347-usb-jtag"
                );
                std::process::exit(0);
            }
            _ => {}
        }
    }
    (
        chip,
        probe,
        file,
        protocol,
        speed,
        op,
        base,
        dll_hint,
        verify,
        preverify,
        chip_erase,
        programmer_type,
    )
}

fn parse_args() -> (
    Option<String>,
    Option<String>,
    Option<PathBuf>,
    Protocol,
    u32,
    Option<String>,
    Option<u64>,
    String,
    bool,
    bool,
    bool,
    Option<String>,
) {
    parse_args_from(env::args().skip(1))
}

fn find_dll(hint: &str) -> Option<PathBuf> {
    // English comments: try hint, then current exe dir, then dist paths in workspace
    let mut candidates: Vec<PathBuf> = vec![];
    if !hint.is_empty() {
        candidates.push(PathBuf::from(hint));
    }
    if let Ok(mut p) = std::env::current_exe() {
        p.set_file_name("probe_rs_lib.dll");
        candidates.push(p);
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let root = PathBuf::from(manifest).parent().unwrap().to_path_buf();
        candidates.push(root.join("dist/probe-rs-lib/bin/release/probe_rs_lib.dll"));
        candidates.push(root.join("dist/probe-rs-lib/bin/debug/probe_rs_lib.dll"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn proto_code(p: Protocol) -> i32 {
    match p {
        Protocol::Auto => 0,
        Protocol::Swd => 1,
        Protocol::Jtag => 2,
    }
}

fn main() {
    let (
        chip,
        probe,
        file,
        protocol,
        speed,
        op,
        base,
        dll_hint,
        verify,
        preverify,
        chip_erase,
        programmer_type,
    ) = parse_args();
    let dll = if dll_hint.is_empty() {
        let mut p = std::env::current_exe().expect("get current exe failed");
        p.set_file_name("probe_rs_lib.dll");
        if !p.is_file() {
            eprintln!("Required DLL not found in executable directory");
            std::process::exit(2);
        }
        p
    } else {
        match find_dll(&dll_hint) {
            Some(p) => p,
            None => {
                eprintln!("probe_rs_lib.dll not found; use --dll <path> to specify");
                std::process::exit(2);
            }
        }
    };
    let ffi = load_ffi(dll.to_string_lossy().as_ref());

    let op = op.unwrap_or_else(|| {
        if file.is_some() {
            "flash".to_string()
        } else {
            "check".to_string()
        }
    });
    if op != "list" {
        let pt_str = match programmer_type {
            Some(t) => t,
            None => {
                eprintln!("--programmer-type required");
                std::process::exit(1);
            }
        };
        let c_pt = CString::new(pt_str.clone()).unwrap();
        let mut code: i32 = -1;
        let rc_conv =
            unsafe { (ffi.pr_programmer_type_from_string)(c_pt.as_ptr(), &mut code as *mut i32) };
        if rc_conv != 0 || code < 0 {
            eprintln!("Unsupported programmer type: {}", pt_str);
            std::process::exit(1);
        }
        let supported = unsafe { (ffi.pr_programmer_type_is_supported_code)(code) };
        if supported == 0 {
            eprintln!("Unsupported programmer type code: {}", pt_str);
            std::process::exit(1);
        }
        let rc = unsafe { (ffi.pr_set_programmer_type_code)(code) };
        if rc != 0 {
            print_last_error(&ffi);
            std::process::exit(rc);
        }
    }

    match op.as_str() {
        "list" => unsafe {
            let n = (ffi.pr_probe_count)();
            println!("Found {} probes", n);
            for i in 0..n {
                let mut name = vec![0u8; 128];
                let mut sn = vec![0u8; 128];
                let mut vid: u16 = 0;
                let mut pid: u16 = 0;
                let rc = (ffi.pr_probe_info)(
                    i,
                    name.as_mut_ptr() as *mut c_char,
                    name.len(),
                    &mut vid,
                    &mut pid,
                    sn.as_mut_ptr() as *mut c_char,
                    sn.len(),
                );
                if rc != 0 {
                    print_last_error(&ffi);
                    continue;
                }
                let mut drv = 0u32;
                let mut feat = 0u32;
                let _ = (ffi.pr_probe_features)(i, &mut drv, &mut feat);
                let connected = (ffi.pr_probe_check_target)(i);
                println!(
                    "[{}] {} {:04x}:{:04x} SN={} drv=0x{:08x} feat=0x{:08x} connected={}",
                    i,
                    String::from_utf8_lossy(&name).trim_end_matches('\0'),
                    vid,
                    pid,
                    String::from_utf8_lossy(&sn).trim_end_matches('\0'),
                    drv,
                    feat,
                    if connected == 1 { "yes" } else { "no" }
                );
            }
        },
        "check" => unsafe {
            let chip = match chip {
                Some(c) => c,
                None => {
                    eprintln!("--chip required for check");
                    std::process::exit(1);
                }
            };
            let c_chip = CString::new(chip).unwrap();
            let handle = if let Some(sel) = probe.clone() {
                let c_sel = CString::new(sel).unwrap();
                (ffi.pr_session_open_with_probe)(
                    c_sel.as_ptr(),
                    c_chip.as_ptr(),
                    speed,
                    proto_code(protocol),
                )
            } else {
                (ffi.pr_session_open_auto)(c_chip.as_ptr(), speed, proto_code(protocol))
            };
            if handle == 0 {
                print_last_error(&ffi);
                std::process::exit(3);
            }
            println!("Session opened: {}", handle);
            let _ = (ffi.pr_session_close)(handle);
            println!("Session closed");
        },
        "flash" => unsafe {
            (ffi.pr_set_progress_callback)(cli_progress_cb);
            let chip = match chip {
                Some(c) => c,
                None => {
                    eprintln!("--chip required for flash");
                    std::process::exit(1);
                }
            };
            let path = match file {
                Some(p) => p,
                None => {
                    eprintln!("--file required for flash");
                    std::process::exit(1);
                }
            };
            let c_chip = CString::new(chip).unwrap();
            let c_path = CString::new(path.to_string_lossy().to_string()).unwrap();
            let base_val = base.unwrap_or(0);
            let rc = (ffi.pr_flash_auto)(
                c_chip.as_ptr(),
                c_path.as_ptr(),
                base_val,
                0,
                if verify { 1 } else { 0 },
                if preverify { 1 } else { 0 },
                if chip_erase { 1 } else { 0 },
                speed,
                proto_code(protocol),
            );
            (ffi.pr_clear_progress_callback)();
            if rc != 0 {
                print_last_error(&ffi);
                std::process::exit(rc);
            }
            println!("Flash complete");
        },
        _ => {
            eprintln!("unknown --op");
            std::process::exit(1);
        }
    }
}

unsafe extern "C" fn cli_progress_cb(_op: i32, percent: f32, status: *const c_char, eta_ms: i32) {
    let status_str = unsafe { CStr::from_ptr(status).to_str().unwrap_or("") };
    let eta_text = if eta_ms > 0 {
        format!(" ETA ~{}s", eta_ms / 1000)
    } else {
        String::new()
    };
    let _ = io::stdout()
        .write_all(format!("{} {:>6.2}%{}\n", status_str, percent, eta_text).as_bytes());
    let _ = io::stdout().flush();
}

// English comments: unit tests cover argument parsing behavior without touching the DLL
#[cfg(test)]
mod tests {
    use super::*;

    fn make_args(v: &[&str]) -> impl Iterator<Item = String> {
        v.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parse_defaults() {
        let (
            chip,
            probe,
            file,
            protocol,
            speed,
            op,
            base,
            dll_hint,
            verify,
            preverify,
            chip_erase,
            programmer_type,
        ) = parse_args_from(make_args(&[]));
        assert!(chip.is_none());
        assert!(probe.is_none());
        assert!(file.is_none());
        match protocol {
            Protocol::Auto => {}
            _ => panic!("protocol should default to Auto"),
        }
        assert_eq!(speed, 4000);
        assert!(op.is_none());
        assert!(base.is_none());
        assert_eq!(dll_hint, "");
        assert!(verify);
        assert!(!preverify);
        assert!(chip_erase);
        assert!(programmer_type.is_none());
    }

    #[test]
    fn parse_protocol_speed_and_flags() {
        let args = make_args(&[
            "--protocol",
            "swd",
            "--speed",
            "5000",
            "--verify",
            "--no-preverify",
            "--chip-erase",
        ]);
        let (_, _, _, protocol, speed, _, _, _, verify, preverify, chip_erase, _) =
            parse_args_from(args);
        match protocol {
            Protocol::Swd => {}
            _ => panic!("protocol should be swd"),
        }
        assert_eq!(speed, 5000);
        assert!(verify);
        assert!(!preverify);
        assert!(chip_erase);
    }

    #[test]
    fn parse_base_formats() {
        let args_hex = make_args(&["--base", "0x1000"]);
        let (_, _, _, _, _, _, base_hex, _, _, _, _, _) = parse_args_from(args_hex);
        assert_eq!(base_hex, Some(0x1000));

        let args_bin = make_args(&["--base", "0b1010"]);
        let (_, _, _, _, _, _, base_bin, _, _, _, _, _) = parse_args_from(args_bin);
        assert_eq!(base_bin, Some(10));

        let args_oct = make_args(&["--base", "0o77"]);
        let (_, _, _, _, _, _, base_oct, _, _, _, _, _) = parse_args_from(args_oct);
        assert_eq!(base_oct, Some(63));

        let args_dec = make_args(&["--base", "4096"]);
        let (_, _, _, _, _, _, base_dec, _, _, _, _, _) = parse_args_from(args_dec);
        assert_eq!(base_dec, Some(4096));
    }
}
