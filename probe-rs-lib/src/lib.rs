use probe_rs::config::Registry;
use probe_rs::flashing::{
    self, BinOptions, DownloadOptions, FlashProgress, Format, FormatKind, ProgressEvent,
    ProgressOperation,
};
use probe_rs::probe::{DebugProbeSelector, WireProtocol, list::Lister};
use probe_rs::probe::{
    ch347usbjtag::Ch347UsbJtagFactory, cmsisdap::CmsisDapFactory, espusbjtag::EspUsbJtagFactory,
    ftdi::FtdiProbeFactory, glasgow::GlasgowFactory, jlink::JLinkFactory,
    sifliuart::SifliUartFactory, stlink::StLinkFactory, wlink::WchLinkFactory,
};
use probe_rs::{CoreStatus, MemoryInterface, Permissions, Session, SessionConfig};
use probe_rs_target::MemoryRegion;
use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

static LAST_ERROR: OnceLock<Mutex<String>> = OnceLock::new();
static SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<Mutex<Session>>>>> = OnceLock::new();
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
type ProgressCb = unsafe extern "C" fn(i32, f32, *const c_char, i32);
static PROGRESS_CB: OnceLock<Mutex<Option<ProgressCb>>> = OnceLock::new();
#[derive(Clone, Copy)]
enum ProgrammerType {
    CmsisDap,
    JLink,
    StLink,
    Ftdi,
    EspUsbJtag,
    WchLink,
    SifliUart,
    Glasgow,
    Ch347UsbJtag,
}
static PROGRAMMER_TYPE: OnceLock<Mutex<Option<ProgrammerType>>> = OnceLock::new();
static REGISTRY: OnceLock<Registry> = OnceLock::new();

#[derive(Clone)]
struct ManuEntry {
    name: String,
    chips: Vec<String>,
}

struct ChipDb {
    manufacturers: Vec<ManuEntry>,
    name_to_index: HashMap<String, (u32, u32)>,
}

static CHIP_DB: OnceLock<ChipDb> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Registry::from_builtin_families())
}

fn build_chip_db() -> ChipDb {
    let reg = registry();
    let mut manu_map: HashMap<(u8, u8), usize> = HashMap::new();
    let mut manufacturers: Vec<ManuEntry> = Vec::new();

    for family in reg.families() {
        let (cc, id, mname) = match family.manufacturer {
            Some(code) => {
                let name = code.get().unwrap_or("<unknown>").to_string();
                (code.cc, code.id, name)
            }
            None => (0, 0, "Generic".to_string()),
        };
        let idx = *manu_map.entry((cc, id)).or_insert_with(|| {
            let i = manufacturers.len();
            manufacturers.push(ManuEntry {
                name: mname.clone(),
                chips: Vec::new(),
            });
            i
        });
        let targets = reg
            .get_targets_by_family_name(&family.name)
            .unwrap_or_default();
        manufacturers[idx].chips.extend(targets);
    }

    for m in manufacturers.iter_mut() {
        m.chips.sort();
        m.chips.dedup();
    }

    let mut name_to_index: HashMap<String, (u32, u32)> = HashMap::new();
    for (mi, m) in manufacturers.iter().enumerate() {
        for (ci, c) in m.chips.iter().enumerate() {
            name_to_index.insert(c.clone(), (mi as u32, ci as u32));
        }
    }

    ChipDb {
        manufacturers,
        name_to_index,
    }
}

fn do_chip_erase(chip: &str, speed_khz: u32, proto: Option<WireProtocol>) -> i32 {
    let lister = Lister::new();
    let mut probes = lister.list_all();
    if let Some(ty) = *programmer_type_lock().lock().unwrap() {
        probes.retain(|p| info_matches_type(p, ty));
    }
    if probes.is_empty() {
        set_error("no matching probes found".to_string());
        return -1;
    }

    let target = match registry().get_target_by_name(chip) {
        Ok(t) => t,
        Err(e) => {
            set_error(format!("failed to get target: {}", e));
            return -1;
        }
    };

    let mut probe = match probes[0].open() {
        Ok(p) => p,
        Err(e) => {
            set_error(format!("failed to open probe: {}", e));
            return -1;
        }
    };

    if let Some(p) = proto {
        if let Err(e) = probe.select_protocol(p) {
            set_error(format!("failed to select protocol: {}", e));
            return -1;
        }
    }

    if speed_khz > 0 {
        if let Err(e) = probe.set_speed(speed_khz) {
            set_error(format!("failed to set speed: {}", e));
            return -1;
        }
    }

    let mut session = match probe.attach(target, Permissions::new()) {
        Ok(s) => s,
        Err(e) => {
            set_error(format!("failed to attach: {}", e));
            return -1;
        }
    };

    let mut progress = FlashProgress::new(|_| {});
    let res = flashing::erase_all(&mut session, &mut progress);
    match res {
        Ok(_) => 0,
        Err(e) => {
            set_error(e.to_string());
            -1
        }
    }
}

/// Erase the entire flash memory of a target chip.
///
/// This function attempts to connect to a target chip and erase its entire
/// non-volatile memory.
///
/// # Arguments
///
/// * `chip` - A C-style string specifying the target chip model (e.g., "stm32f407").
/// * `speed_khz` - The desired debug probe speed in kilohertz. If 0, a default speed is used.
/// * `protocol_code` - An integer code representing the wire protocol to use:
///   - 1 for SWD
///   - 2 for JTAG
///   - Any other value defaults to the probe's default protocol.
///
/// # Returns
///
/// * `0` on success.
/// * `-1` on failure. Call `pr_get_last_error` to retrieve a detailed error message.
///
/// # Safety
///
/// This function is unsafe because it dereferences a raw pointer (`chip`). The caller
/// must ensure that `chip` is a valid, null-terminated C string.
#[unsafe(no_mangle)]
pub extern "C" fn pr_chip_erase(chip: *const c_char, speed_khz: u32, protocol_code: i32) -> i32 {
    let Ok(chip_str) = cstr_to_string(chip) else {
        set_error("invalid chip string".to_string());
        return -1;
    };
    let proto = protocol_from_int(protocol_code);
    do_chip_erase(&chip_str, speed_khz, proto)
}

fn chip_db() -> &'static ChipDb {
    CHIP_DB.get_or_init(build_chip_db)
}

fn make_target_spec_string(manufacturer: &str, chip_name: &str) -> Result<String, String> {
    let target = match registry().get_target_by_name(chip_name) {
        Ok(t) => t,
        Err(e) => return Err(format!("get_target_by_name error: {}", e)),
    };

    let arch = format!("{:?}", target.architecture());
    let cores = target
        .cores
        .iter()
        .map(|c| format!("{}:{:?}", c.name, c.core_type))
        .collect::<Vec<_>>()
        .join(", ");

    let mut ram_total: u64 = 0;
    let mut nvm_total: u64 = 0;
    let mut regions: Vec<String> = Vec::new();
    for region in target.memory_map.iter() {
        match region {
            MemoryRegion::Ram(r) => {
                let size = r.range.end.saturating_sub(r.range.start);
                ram_total = ram_total.saturating_add(size);
                regions.push(format!(
                    "Ram({:#010x}-{:#010x})",
                    r.range.start, r.range.end
                ));
            }
            MemoryRegion::Nvm(n) => {
                let size = n.range.end.saturating_sub(n.range.start);
                nvm_total = nvm_total.saturating_add(size);
                regions.push(format!(
                    "Nvm({:#010x}-{:#010x})",
                    n.range.start, n.range.end
                ));
            }
            MemoryRegion::Generic(g) => {
                regions.push(format!(
                    "Generic({:#010x}-{:#010x})",
                    g.range.start, g.range.end
                ));
            }
        }
    }

    let flash_algos = target
        .flash_algorithms
        .iter()
        .map(|a| a.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let default_fmt = target.default_format.clone().unwrap_or_default();

    let s = format!(
        "{{\"manufacturer\":\"{}\",\"chip\":\"{}\",\"architecture\":\"{}\",\"cores\":\"{}\",\"ram_bytes\":{},\"nvm_bytes\":{},\"regions\":\"{}\",\"flash_algorithms\":\"{}\",\"default_format\":\"{}\"}}",
        manufacturer,
        chip_name,
        arch,
        cores,
        ram_total,
        nvm_total,
        regions.join(";"),
        flash_algos,
        default_fmt
    );
    Ok(s)
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_chip_manufacturer_count() -> u32 {
    chip_db().manufacturers.len() as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_chip_manufacturer_name(index: u32, buf: *mut c_char, buf_len: usize) -> usize {
    let db = chip_db();
    let Some(m) = db.manufacturers.get(index as usize) else {
        set_error("manufacturer index out of range".to_string());
        return 0;
    };
    let bytes = m.name.as_bytes();
    let need = bytes.len().saturating_add(1);
    if buf.is_null() || buf_len == 0 {
        return need;
    }
    let copy = need.min(buf_len);
    unsafe {
        let slice = std::slice::from_raw_parts_mut(buf as *mut u8, copy);
        let n = copy.saturating_sub(1);
        slice[..n].copy_from_slice(&bytes[..n]);
        slice[n] = 0;
    }
    need
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_chip_model_count(manufacturer_index: u32) -> u32 {
    let db = chip_db();
    let Some(m) = db.manufacturers.get(manufacturer_index as usize) else {
        set_error("manufacturer index out of range".to_string());
        return 0;
    };
    m.chips.len() as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_chip_model_name(
    manufacturer_index: u32,
    chip_index: u32,
    buf: *mut c_char,
    buf_len: usize,
) -> usize {
    let db = chip_db();
    let Some(m) = db.manufacturers.get(manufacturer_index as usize) else {
        set_error("manufacturer index out of range".to_string());
        return 0;
    };
    let Some(name) = m.chips.get(chip_index as usize) else {
        set_error("chip index out of range".to_string());
        return 0;
    };
    let bytes = name.as_bytes();
    let need = bytes.len().saturating_add(1);
    if buf.is_null() || buf_len == 0 {
        return need;
    }
    let copy = need.min(buf_len);
    unsafe {
        let slice = std::slice::from_raw_parts_mut(buf as *mut u8, copy);
        let n = copy.saturating_sub(1);
        slice[..n].copy_from_slice(&bytes[..n]);
        slice[n] = 0;
    }
    need
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_chip_model_specs(
    manufacturer_index: u32,
    chip_index: u32,
    buf: *mut c_char,
    buf_len: usize,
) -> usize {
    let db = chip_db();
    let Some(m) = db.manufacturers.get(manufacturer_index as usize) else {
        set_error("manufacturer index out of range".to_string());
        return 0;
    };
    let Some(name) = m.chips.get(chip_index as usize) else {
        set_error("chip index out of range".to_string());
        return 0;
    };
    let spec = match make_target_spec_string(&m.name, name) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let bytes = spec.as_bytes();
    let need = bytes.len().saturating_add(1);
    if buf.is_null() || buf_len == 0 {
        return need;
    }
    let copy = need.min(buf_len);
    unsafe {
        let slice = std::slice::from_raw_parts_mut(buf as *mut u8, copy);
        let n = copy.saturating_sub(1);
        slice[..n].copy_from_slice(&bytes[..n]);
        slice[n] = 0;
    }
    need
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_chip_specs_by_name(
    name: *const c_char,
    buf: *mut c_char,
    buf_len: usize,
) -> usize {
    let Ok(chip_name) = cstr_to_string(name) else {
        set_error("invalid chip name".to_string());
        return 0;
    };
    let (manu_idx, _) = match chip_db().name_to_index.get(&chip_name) {
        Some(ix) => *ix,
        None => (u32::MAX, u32::MAX),
    };
    let manufacturer = if manu_idx != u32::MAX {
        chip_db()
            .manufacturers
            .get(manu_idx as usize)
            .map(|m| m.name.clone())
    } else {
        None
    };
    let mname = manufacturer.unwrap_or_else(|| "<unknown>".to_string());
    let spec = match make_target_spec_string(&mname, &chip_name) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let bytes = spec.as_bytes();
    let need = bytes.len().saturating_add(1);
    if buf.is_null() || buf_len == 0 {
        return need;
    }
    let copy = need.min(buf_len);
    unsafe {
        let slice = std::slice::from_raw_parts_mut(buf as *mut u8, copy);
        let n = copy.saturating_sub(1);
        slice[..n].copy_from_slice(&bytes[..n]);
        slice[n] = 0;
    }
    need
}

fn set_error(msg: String) {
    let lock = LAST_ERROR.get_or_init(|| Mutex::new(String::new()));
    let mut s = lock.lock().unwrap();
    *s = msg;
}

fn progress_cb_lock() -> &'static Mutex<Option<ProgressCb>> {
    PROGRESS_CB.get_or_init(|| Mutex::new(None))
}

fn op_code(op: ProgressOperation) -> i32 {
    match op {
        ProgressOperation::Erase => 1,
        ProgressOperation::Program => 2,
        ProgressOperation::Verify => 3,
        ProgressOperation::Fill => 0,
    }
}

fn status_text(op: ProgressOperation) -> &'static str {
    match op {
        ProgressOperation::Erase => "erasing",
        ProgressOperation::Program => "programming",
        ProgressOperation::Verify => "verifying",
        ProgressOperation::Fill => "filling",
    }
}

fn cstr_to_string(ptr: *const c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Err("null string".to_string());
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(|s| s.to_string())
        .map_err(|e| e.to_string())
}

fn parse_programmer_type(name: &str) -> Option<ProgrammerType> {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "cmsis-dap" | "cmsisdap" => Some(ProgrammerType::CmsisDap),
        "jlink" => Some(ProgrammerType::JLink),
        "stlink" | "st-link" => Some(ProgrammerType::StLink),
        "ftdi" => Some(ProgrammerType::Ftdi),
        "esp-usb-jtag" | "espusbjtag" => Some(ProgrammerType::EspUsbJtag),
        "wch-link" | "wlink" => Some(ProgrammerType::WchLink),
        "sifli-uart" | "sifliuart" => Some(ProgrammerType::SifliUart),
        "glasgow" => Some(ProgrammerType::Glasgow),
        "ch347-usb-jtag" | "ch347usbjtag" => Some(ProgrammerType::Ch347UsbJtag),
        _ => None,
    }
}

fn programmer_type_lock() -> &'static Mutex<Option<ProgrammerType>> {
    PROGRAMMER_TYPE.get_or_init(|| Mutex::new(None))
}

fn type_to_code(ty: ProgrammerType) -> i32 {
    match ty {
        ProgrammerType::CmsisDap => 1,
        ProgrammerType::StLink => 2,
        ProgrammerType::JLink => 3,
        ProgrammerType::Ftdi => 4,
        ProgrammerType::EspUsbJtag => 5,
        ProgrammerType::WchLink => 6,
        ProgrammerType::SifliUart => 7,
        ProgrammerType::Glasgow => 8,
        ProgrammerType::Ch347UsbJtag => 9,
    }
}

fn code_to_type(code: i32) -> Option<ProgrammerType> {
    match code {
        1 => Some(ProgrammerType::CmsisDap),
        2 => Some(ProgrammerType::StLink),
        3 => Some(ProgrammerType::JLink),
        4 => Some(ProgrammerType::Ftdi),
        5 => Some(ProgrammerType::EspUsbJtag),
        6 => Some(ProgrammerType::WchLink),
        7 => Some(ProgrammerType::SifliUart),
        8 => Some(ProgrammerType::Glasgow),
        9 => Some(ProgrammerType::Ch347UsbJtag),
        _ => None,
    }
}

fn type_to_str(ty: ProgrammerType) -> &'static str {
    match ty {
        ProgrammerType::CmsisDap => "cmsis-dap",
        ProgrammerType::StLink => "stlink",
        ProgrammerType::JLink => "jlink",
        ProgrammerType::Ftdi => "ftdi",
        ProgrammerType::EspUsbJtag => "esp-usb-jtag",
        ProgrammerType::WchLink => "wch-link",
        ProgrammerType::SifliUart => "sifli-uart",
        ProgrammerType::Glasgow => "glasgow",
        ProgrammerType::Ch347UsbJtag => "ch347-usb-jtag",
    }
}

fn info_matches_type(info: &probe_rs::probe::DebugProbeInfo, ty: ProgrammerType) -> bool {
    match ty {
        ProgrammerType::CmsisDap => info.is_probe_type::<CmsisDapFactory>(),
        ProgrammerType::JLink => info.is_probe_type::<JLinkFactory>(),
        ProgrammerType::StLink => info.is_probe_type::<StLinkFactory>(),
        ProgrammerType::Ftdi => info.is_probe_type::<FtdiProbeFactory>(),
        ProgrammerType::EspUsbJtag => info.is_probe_type::<EspUsbJtagFactory>(),
        ProgrammerType::WchLink => info.is_probe_type::<WchLinkFactory>(),
        ProgrammerType::SifliUart => info.is_probe_type::<SifliUartFactory>(),
        ProgrammerType::Glasgow => info.is_probe_type::<GlasgowFactory>(),
        ProgrammerType::Ch347UsbJtag => info.is_probe_type::<Ch347UsbJtagFactory>(),
    }
}

fn protocol_from_int(code: i32) -> Option<WireProtocol> {
    match code {
        1 => Some(WireProtocol::Swd),
        2 => Some(WireProtocol::Jtag),
        _ => None,
    }
}

fn detect_format_kind(path: &str) -> Option<FormatKind> {
    use std::path::Path;
    let p = Path::new(path);
    let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "elf" | "axf" => Some(FormatKind::Elf),
        "hex" | "ihex" => Some(FormatKind::Hex),
        "bin" => None,
        _ => None,
    }
}

fn detect_format_from_path(path: &str, base: Option<u64>, skip: u32) -> Result<Format, String> {
    if let Some(kind) = detect_format_kind(path) {
        Ok(Format::from(kind))
    } else {
        // Treat as BIN if extension is .bin
        if path.to_ascii_lowercase().ends_with(".bin") {
            let base_addr =
                base.ok_or_else(|| "base_address required for bin format".to_string())?;
            Ok(Format::Bin(BinOptions {
                base_address: Some(base_addr),
                skip,
            }))
        } else {
            Err("unsupported file format extension".to_string())
        }
    }
}

fn do_flash(
    chip: &str,
    path: &str,
    format: Format,
    verify: i32,
    preverify: i32,
    chip_erase: i32,
    speed_khz: u32,
    proto: Option<WireProtocol>,
) -> i32 {
    let mut opts = DownloadOptions::default();
    opts.verify = verify != 0;
    opts.preverify = preverify != 0;
    opts.do_chip_erase = chip_erase != 0;

    if let Some(cb) = *progress_cb_lock().lock().unwrap() {
        use std::time::Duration;

        let mut t_erase: Option<u64> = None;
        let mut d_erase: u64 = 0;
        let mut tm_erase: Duration = Duration::ZERO;
        let mut t_prog: Option<u64> = None;
        let mut d_prog: u64 = 0;
        let mut tm_prog: Duration = Duration::ZERO;
        let mut t_verify: Option<u64> = None;
        let mut d_verify: u64 = 0;
        let mut tm_verify: Duration = Duration::ZERO;
        let mut t_fill: Option<u64> = None;
        let mut d_fill: u64 = 0;
        let mut tm_fill: Duration = Duration::ZERO;
        let mut last_erase_pct: f32 = -1.0;
        let mut last_prog_pct: f32 = -1.0;
        let mut last_verify_pct: f32 = -1.0;
        let mut last_fill_pct: f32 = -1.0;

        opts.progress = FlashProgress::new(move |event| match event {
            ProgressEvent::AddProgressBar { operation, total } => {
                match operation {
                    ProgressOperation::Erase => {
                        t_erase = total;
                        d_erase = 0;
                        tm_erase = Duration::ZERO;
                    }
                    ProgressOperation::Program => {
                        t_prog = total;
                        d_prog = 0;
                        tm_prog = Duration::ZERO;
                    }
                    ProgressOperation::Verify => {
                        t_verify = total;
                        d_verify = 0;
                        tm_verify = Duration::ZERO;
                    }
                    ProgressOperation::Fill => {
                        t_fill = total;
                        d_fill = 0;
                        tm_fill = Duration::ZERO;
                    }
                }
                match operation {
                    ProgressOperation::Erase => {
                        last_erase_pct = -1.0;
                    }
                    ProgressOperation::Program => {
                        last_prog_pct = -1.0;
                    }
                    ProgressOperation::Verify => {
                        last_verify_pct = -1.0;
                    }
                    ProgressOperation::Fill => {
                        last_fill_pct = -1.0;
                    }
                }
            }
            ProgressEvent::Started(op) => {
                let st = status_text(op);
                let cs = std::ffi::CString::new(st).unwrap();
                unsafe { cb(op_code(op), 0.0, cs.as_ptr(), -1) };
                match op {
                    ProgressOperation::Erase => {
                        last_erase_pct = 0.0;
                    }
                    ProgressOperation::Program => {
                        last_prog_pct = 0.0;
                    }
                    ProgressOperation::Verify => {
                        last_verify_pct = 0.0;
                    }
                    ProgressOperation::Fill => {
                        last_fill_pct = 0.0;
                    }
                }
            }
            ProgressEvent::Progress {
                operation,
                size,
                time,
            } => {
                let (total_opt, d_ref, tm_ref) = match operation {
                    ProgressOperation::Erase => (&t_erase, &mut d_erase, &mut tm_erase),
                    ProgressOperation::Program => (&t_prog, &mut d_prog, &mut tm_prog),
                    ProgressOperation::Verify => (&t_verify, &mut d_verify, &mut tm_verify),
                    ProgressOperation::Fill => (&t_fill, &mut d_fill, &mut tm_fill),
                };
                *d_ref = d_ref.saturating_add(size);
                *tm_ref += time;
                let total = total_opt.unwrap_or(0);
                let percent = if total > 0 {
                    ((*d_ref as f64 / total as f64) * 100.0) as f32
                } else {
                    0.0
                };
                let eta_ms = if total > 0 && *tm_ref > Duration::ZERO {
                    let remaining = total.saturating_sub(*d_ref) as f64;
                    let rate = (*d_ref as f64) / tm_ref.as_secs_f64();
                    if rate > 0.0 {
                        (remaining / rate * 1000.0) as i32
                    } else {
                        -1
                    }
                } else {
                    -1
                };
                let st = status_text(operation);
                let cs = std::ffi::CString::new(st).unwrap();
                let last = match operation {
                    ProgressOperation::Erase => &mut last_erase_pct,
                    ProgressOperation::Program => &mut last_prog_pct,
                    ProgressOperation::Verify => &mut last_verify_pct,
                    ProgressOperation::Fill => &mut last_fill_pct,
                };
                let pct = percent.min(100.0);
                let changed = (pct - *last).abs() >= 0.1 || pct >= 100.0;
                if changed {
                    unsafe { cb(op_code(operation), pct, cs.as_ptr(), eta_ms) };
                    *last = pct;
                }
            }
            ProgressEvent::Finished(op) => {
                let st = status_text(op);
                let cs = std::ffi::CString::new(st).unwrap();
                let last = match op {
                    ProgressOperation::Erase => &mut last_erase_pct,
                    ProgressOperation::Program => &mut last_prog_pct,
                    ProgressOperation::Verify => &mut last_verify_pct,
                    ProgressOperation::Fill => &mut last_fill_pct,
                };
                if *last < 100.0 {
                    unsafe { cb(op_code(op), 100.0, cs.as_ptr(), 0) };
                    *last = 100.0;
                }
            }
            ProgressEvent::Failed(op) => {
                let st = status_text(op);
                let cs = std::ffi::CString::new(st).unwrap();
                unsafe { cb(op_code(op), 0.0, cs.as_ptr(), -1) };
                match op {
                    ProgressOperation::Erase => {
                        last_erase_pct = 0.0;
                    }
                    ProgressOperation::Program => {
                        last_prog_pct = 0.0;
                    }
                    ProgressOperation::Verify => {
                        last_verify_pct = 0.0;
                    }
                    ProgressOperation::Fill => {
                        last_fill_pct = 0.0;
                    }
                }
            }
            ProgressEvent::FlashLayoutReady { .. } | ProgressEvent::DiagnosticMessage { .. } => {}
        });
    }

    let session_cfg = SessionConfig {
        permissions: Default::default(),
        speed: if speed_khz == 0 {
            None
        } else {
            Some(speed_khz)
        },
        protocol: proto,
    };
    let mut session = if let Some(ty) = *programmer_type_lock().lock().unwrap() {
        let lister = Lister::new();
        let list = lister.list_all();
        let Some(info) = list.into_iter().find(|i| info_matches_type(i, ty)) else {
            set_error("no probe matching programmer type".to_string());
            return 1;
        };
        let mut probe = match info.open() {
            Ok(p) => p,
            Err(e) => {
                set_error(format!("open probe error: {}", e));
                return 1;
            }
        };
        if let Some(p) = proto {
            if let Err(e) = probe.select_protocol(p) {
                set_error(format!("select protocol error: {}", e));
                return 1;
            }
        }
        if speed_khz > 0 {
            if let Err(e) = probe.set_speed(speed_khz) {
                set_error(format!("set speed error: {}", e));
                return 1;
            }
        }
        match probe.attach(chip, Default::default()) {
            Ok(sess) => sess,
            Err(e) => {
                set_error(format!("attach error: {}", e));
                return 1;
            }
        }
    } else {
        match Session::auto_attach(chip, session_cfg) {
            Ok(s) => s,
            Err(e) => {
                set_error(format!("attach error: {}", e));
                return 1;
            }
        }
    };
    match flashing::download_file_with_options(&mut session, path, format, opts) {
        Ok(_) => 0,
        Err(e) => {
            set_error(format!("flash error: {}", e));
            2
        }
    }
}

fn sessions() -> &'static Mutex<HashMap<u64, Arc<Mutex<Session>>>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn make_handle(session: Session) -> u64 {
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    sessions()
        .lock()
        .unwrap()
        .insert(handle, Arc::new(Mutex::new(session)));
    handle
}

fn get_session(handle: u64) -> Result<Arc<Mutex<Session>>, String> {
    sessions()
        .lock()
        .unwrap()
        .get(&handle)
        .cloned()
        .ok_or_else(|| "invalid session handle".to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_last_error(buf: *mut c_char, buf_len: usize) -> usize {
    let s = {
        let lock = LAST_ERROR.get_or_init(|| Mutex::new(String::new()));
        lock.lock().unwrap().clone()
    };
    let bytes = s.as_bytes();
    let need = bytes.len() + 1;
    if buf.is_null() || buf_len == 0 {
        return need;
    }
    let copy = need.min(buf_len);
    unsafe {
        let slice = std::slice::from_raw_parts_mut(buf as *mut u8, copy);
        let n = copy.saturating_sub(1);
        slice[..n].copy_from_slice(&bytes[..n]);
        slice[n] = 0;
    }
    need
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_version(buf: *mut c_char, buf_len: usize) -> usize {
    let s = format!("{}", env!("CARGO_PKG_VERSION"));
    let bytes = s.as_bytes();
    let need = bytes.len() + 1;
    if buf.is_null() || buf_len == 0 {
        return need;
    }
    let copy = need.min(buf_len);
    unsafe {
        let slice = std::slice::from_raw_parts_mut(buf as *mut u8, copy);
        let n = copy.saturating_sub(1);
        slice[..n].copy_from_slice(&bytes[..n]);
        slice[n] = 0;
    }
    need
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_set_progress_callback(cb: ProgressCb) {
    let lock = progress_cb_lock();
    let mut l = lock.lock().unwrap();
    *l = Some(cb);
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_clear_progress_callback() {
    let lock = progress_cb_lock();
    let mut l = lock.lock().unwrap();
    *l = None;
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_probe_count() -> u32 {
    let lister = Lister::new();
    lister.list_all().len() as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_probe_info(
    index: u32,
    identifier: *mut c_char,
    identifier_len: usize,
    vid: *mut u16,
    pid: *mut u16,
    serial: *mut c_char,
    serial_len: usize,
) -> i32 {
    let lister = Lister::new();
    let probes = lister.list_all();
    let Some(info) = probes.get(index as usize) else {
        set_error("probe index out of range".to_string());
        return -1;
    };

    unsafe {
        if !vid.is_null() {
            *vid = info.vendor_id;
        }
        if !pid.is_null() {
            *pid = info.product_id;
        }
    }

    let id = info.identifier.as_str();
    let id_bytes = id.as_bytes();
    let copy_id = id_bytes.len().saturating_add(1).min(identifier_len);
    if !identifier.is_null() && copy_id > 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(identifier as *mut u8, copy_id);
            let n = copy_id.saturating_sub(1);
            slice[..n].copy_from_slice(&id_bytes[..n]);
            slice[n] = 0;
        }
    }

    let ser = info.serial_number.as_deref().unwrap_or("");
    let ser_bytes = ser.as_bytes();
    let copy_ser = ser_bytes.len().saturating_add(1).min(serial_len);
    if !serial.is_null() && copy_ser > 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(serial as *mut u8, copy_ser);
            let n = copy_ser.saturating_sub(1);
            slice[..n].copy_from_slice(&ser_bytes[..n]);
            slice[n] = 0;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_probe_features(
    index: u32,
    out_driver_flags: *mut u32,
    out_feature_flags: *mut u32,
) -> i32 {
    let lister = Lister::new();
    let probes = lister.list_all();
    let Some(info) = probes.get(index as usize) else {
        set_error("probe index out of range".to_string());
        return -1;
    };

    let mut driver_flags: u32 = 0;
    if info.is_probe_type::<CmsisDapFactory>() {
        driver_flags |= 0x00000001;
    }
    if info.is_probe_type::<JLinkFactory>() {
        driver_flags |= 0x00000002;
    }
    if info.is_probe_type::<StLinkFactory>() {
        driver_flags |= 0x00000004;
    }
    if info.is_probe_type::<FtdiProbeFactory>() {
        driver_flags |= 0x00000008;
    }
    if info.is_probe_type::<EspUsbJtagFactory>() {
        driver_flags |= 0x00000010;
    }
    if info.is_probe_type::<WchLinkFactory>() {
        driver_flags |= 0x00000020;
    }
    if info.is_probe_type::<SifliUartFactory>() {
        driver_flags |= 0x00000040;
    }
    if info.is_probe_type::<GlasgowFactory>() {
        driver_flags |= 0x00000080;
    }
    if info.is_probe_type::<Ch347UsbJtagFactory>() {
        driver_flags |= 0x00000100;
    }

    let mut feature_flags: u32 = 0;
    let mut probe = match info.open() {
        Ok(p) => p,
        Err(e) => {
            set_error(format!("open probe error: {}", e));
            return -1;
        }
    };

    if probe.select_protocol(WireProtocol::Swd).is_ok() {
        feature_flags |= 0x00000001;
    }
    if probe.select_protocol(WireProtocol::Jtag).is_ok() {
        feature_flags |= 0x00000002;
    }
    if probe.has_arm_debug_interface() {
        feature_flags |= 0x00000004;
    }
    if probe.has_riscv_interface() {
        feature_flags |= 0x00000008;
    }
    if probe.has_xtensa_interface() {
        feature_flags |= 0x00000010;
    }
    if probe.get_swo_interface().is_some() {
        feature_flags |= 0x00000020;
    }
    if probe.set_speed(1000).is_ok() {
        feature_flags |= 0x00000040;
    }

    unsafe {
        if !out_driver_flags.is_null() {
            *out_driver_flags = driver_flags;
        }
        if !out_feature_flags.is_null() {
            *out_feature_flags = feature_flags;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_probe_check_target(index: u32) -> i32 {
    let lister = Lister::new();
    let probes = lister.list_all();
    let Some(info) = probes.get(index as usize) else {
        set_error("probe index out of range".to_string());
        return -1;
    };

    let mut probe = match info.open() {
        Ok(p) => p,
        Err(e) => {
            set_error(format!("open probe error: {}", e));
            return -1;
        }
    };

    let mut last_err: Option<String> = None;
    for proto in [WireProtocol::Swd, WireProtocol::Jtag] {
        if probe.select_protocol(proto).is_err() {
            continue;
        }
        match probe.attach_to_unspecified() {
            Ok(()) => {
                let _ = probe.detach();
                return 1;
            }
            Err(e) => {
                last_err = Some(format!("attach failed: {}", e));
            }
        }
    }

    if let Some(msg) = last_err {
        set_error(msg);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_open_auto(
    chip: *const c_char,
    speed_khz: u32,
    protocol_code: i32,
) -> u64 {
    let Ok(chip) = cstr_to_string(chip) else {
        set_error("invalid chip".to_string());
        return 0;
    };
    let proto = protocol_from_int(protocol_code);
    if let Some(ty) = *programmer_type_lock().lock().unwrap() {
        let lister = Lister::new();
        let list = lister.list_all();
        let Some(info) = list.into_iter().find(|i| info_matches_type(i, ty)) else {
            set_error("no probe matching programmer type".to_string());
            return 0;
        };
        match info.open() {
            Ok(mut probe) => {
                if let Some(p) = proto {
                    if let Err(e) = probe.select_protocol(p) {
                        set_error(format!("select protocol error: {}", e));
                        return 0;
                    }
                }
                if speed_khz > 0 {
                    if let Err(e) = probe.set_speed(speed_khz) {
                        set_error(format!("set speed error: {}", e));
                        return 0;
                    }
                }
                match probe.attach(chip, Default::default()) {
                    Ok(sess) => make_handle(sess),
                    Err(e) => {
                        set_error(format!("attach error: {}", e));
                        0
                    }
                }
            }
            Err(e) => {
                set_error(format!("open probe error: {}", e));
                0
            }
        }
    } else {
        let session_cfg = SessionConfig {
            permissions: Default::default(),
            speed: if speed_khz == 0 {
                None
            } else {
                Some(speed_khz)
            },
            protocol: proto,
        };
        match Session::auto_attach(chip, session_cfg) {
            Ok(sess) => make_handle(sess),
            Err(e) => {
                set_error(format!("attach error: {}", e));
                0
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_open_with_probe(
    selector: *const c_char,
    chip: *const c_char,
    speed_khz: u32,
    protocol_code: i32,
) -> u64 {
    let Ok(sel) = cstr_to_string(selector) else {
        set_error("invalid selector".to_string());
        return 0;
    };
    let Ok(chip) = cstr_to_string(chip) else {
        set_error("invalid chip".to_string());
        return 0;
    };
    let lister = Lister::new();
    let selector: DebugProbeSelector = match sel.parse() {
        Ok(s) => s,
        Err(e) => {
            set_error(format!("selector parse error: {}", e));
            return 0;
        }
    };
    let v = selector.vendor_id;
    let p = selector.product_id;
    let sn = selector.serial_number.clone();
    match lister.open(selector) {
        Ok(mut probe) => {
            if let Some(ty) = *programmer_type_lock().lock().unwrap() {
                let probes = Lister::new().list_all();
                let maybe_info = probes.into_iter().find(|i| {
                    i.vendor_id == v
                        && i.product_id == p
                        && match (&sn, &i.serial_number) {
                            (Some(a), Some(b)) => a == b,
                            (Some(_), None) => false,
                            (None, _) => true,
                        }
                });
                if let Some(info) = maybe_info {
                    if !info_matches_type(&info, ty) {
                        set_error("programmer type mismatch".to_string());
                        return 0;
                    }
                } else {
                    set_error("probe not found".to_string());
                    return 0;
                }
            }
            if let Some(p) = protocol_from_int(protocol_code) {
                if let Err(e) = probe.select_protocol(p) {
                    set_error(format!("select protocol error: {}", e));
                    return 0;
                }
            }
            if speed_khz > 0 {
                if let Err(e) = probe.set_speed(speed_khz) {
                    set_error(format!("set speed error: {}", e));
                    return 0;
                }
            }
            match probe.attach(chip, Default::default()) {
                Ok(sess) => make_handle(sess),
                Err(e) => {
                    set_error(format!("attach error: {}", e));
                    0
                }
            }
        }
        Err(e) => {
            set_error(format!("open probe error: {}", e));
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_close(session: u64) -> i32 {
    let mut map = sessions().lock().unwrap();
    match map.remove(&session) {
        Some(arc) => {
            drop(arc);
            0
        }
        None => {
            set_error("invalid session handle".to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_core_count(session: u64) -> u32 {
    let Ok(sess) = get_session(session) else {
        return 0;
    };
    let lock = sess.lock().unwrap();
    lock.list_cores().len() as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_core_halt(session: u64, core_index: u32, timeout_ms: u32) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.halt(std::time::Duration::from_millis(timeout_ms as u64)) {
            Ok(_) => 0,
            Err(e) => {
                set_error(format!("halt error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_core_run(session: u64, core_index: u32) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.run() {
            Ok(_) => 0,
            Err(e) => {
                set_error(format!("run error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_core_step(session: u64, core_index: u32) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.step() {
            Ok(_) => 0,
            Err(e) => {
                set_error(format!("step error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_core_reset(session: u64, core_index: u32) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.reset() {
            Ok(_) => 0,
            Err(e) => {
                set_error(format!("reset error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_core_reset_and_halt(session: u64, core_index: u32, timeout_ms: u32) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => {
            match core.reset_and_halt(std::time::Duration::from_millis(timeout_ms as u64)) {
                Ok(_) => 0,
                Err(e) => {
                    set_error(format!("reset_and_halt error: {}", e));
                    -2
                }
            }
        }
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_core_status(session: u64, core_index: u32) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.status() {
            Ok(st) => match st {
                CoreStatus::Halted(_) => 1,
                CoreStatus::Running => 2,
                _ => 0,
            },
            Err(e) => {
                set_error(format!("status error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_read_8(
    session: u64,
    core_index: u32,
    address: u64,
    buf: *mut u8,
    len: u32,
) -> i32 {
    if buf.is_null() {
        set_error("buf is null".to_string());
        return -1;
    }
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    let mut tmp = vec![0u8; len as usize];
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.read_8(address, &mut tmp) {
            Ok(_) => {
                unsafe {
                    std::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, len as usize);
                }
                0
            }
            Err(e) => {
                set_error(format!("read_8 error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_write_8(
    session: u64,
    core_index: u32,
    address: u64,
    buf: *const u8,
    len: u32,
) -> i32 {
    if buf.is_null() {
        set_error("buf is null".to_string());
        return -1;
    }
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    let slice = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.write_8(address, slice) {
            Ok(_) => 0,
            Err(e) => {
                set_error(format!("write_8 error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_read_32(
    session: u64,
    core_index: u32,
    address: u64,
    buf: *mut u32,
    len_words: u32,
) -> i32 {
    if buf.is_null() {
        set_error("buf is null".to_string());
        return -1;
    }
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    let mut tmp = vec![0u32; len_words as usize];
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.read_32(address, &mut tmp) {
            Ok(_) => {
                unsafe {
                    std::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, len_words as usize);
                }
                0
            }
            Err(e) => {
                set_error(format!("read_32 error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_write_32(
    session: u64,
    core_index: u32,
    address: u64,
    buf: *const u32,
    len_words: u32,
) -> i32 {
    if buf.is_null() {
        set_error("buf is null".to_string());
        return -1;
    }
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    let slice = unsafe { std::slice::from_raw_parts(buf, len_words as usize) };
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.write_32(address, slice) {
            Ok(_) => 0,
            Err(e) => {
                set_error(format!("write_32 error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_registers_count(session: u64, core_index: u32) -> u32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return 0;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(core) => core.registers().all_registers().count() as u32,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_register_info(
    session: u64,
    core_index: u32,
    reg_index: u32,
    reg_id: *mut u16,
    bit_size: *mut u32,
    name: *mut c_char,
    name_len: usize,
) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    let Ok(core) = lock.core(core_index as usize) else {
        set_error("core access error".to_string());
        return -1;
    };
    let regs = core.registers();
    let Some(desc) = regs.all_registers().nth(reg_index as usize) else {
        set_error("reg index out of range".to_string());
        return -1;
    };
    unsafe {
        if !reg_id.is_null() {
            *reg_id = desc.id.0;
        }
        if !bit_size.is_null() {
            *bit_size = match desc.data_type {
                probe_rs::RegisterDataType::UnsignedInteger(bits) => bits as u32,
                probe_rs::RegisterDataType::FloatingPoint(bits) => bits as u32,
            };
        }
    }
    // Primary display name from register descriptor
    let name_str = desc.name();
    let bytes = name_str.as_bytes();
    if !name.is_null() && name_len > 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(name as *mut u8, name_len);
            let n = name_len.saturating_sub(1);
            let m = n.min(bytes.len());
            slice[..m].copy_from_slice(&bytes[..m]);
            slice[m] = 0;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_read_reg_u64(
    session: u64,
    core_index: u32,
    reg_id: u16,
    out_value: *mut u64,
) -> i32 {
    if out_value.is_null() {
        set_error("out_value is null".to_string());
        return -1;
    }
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.read_core_reg::<u64>(probe_rs::RegisterId(reg_id)) {
            Ok(v) => {
                unsafe {
                    *out_value = v;
                }
                0
            }
            Err(e) => {
                set_error(format!("read reg error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_write_reg_u64(session: u64, core_index: u32, reg_id: u16, value: u64) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.write_core_reg(probe_rs::RegisterId(reg_id), value) {
            Ok(()) => 0,
            Err(e) => {
                set_error(format!("write reg error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_available_breakpoint_units(
    session: u64,
    core_index: u32,
    out_units: *mut u32,
) -> i32 {
    if out_units.is_null() {
        set_error("out_units is null".to_string());
        return -1;
    }
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.available_breakpoint_units() {
            Ok(v) => {
                unsafe {
                    *out_units = v;
                }
                0
            }
            Err(e) => {
                set_error(format!("bp units error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_set_hw_breakpoint(session: u64, core_index: u32, address: u64) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.set_hw_breakpoint(address) {
            Ok(()) => 0,
            Err(e) => {
                set_error(format!("set bp error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_clear_hw_breakpoint(session: u64, core_index: u32, address: u64) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.clear_hw_breakpoint(address) {
            Ok(()) => 0,
            Err(e) => {
                set_error(format!("clear bp error: {}", e));
                -2
            }
        },
        Err(e) => {
            set_error(format!("core access error: {}", e));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_clear_all_hw_breakpoints(session: u64) -> i32 {
    let Ok(sess) = get_session(session) else {
        set_error("invalid session handle".to_string());
        return -1;
    };
    let mut lock = sess.lock().unwrap();
    match lock.clear_all_hw_breakpoints() {
        Ok(()) => 0,
        Err(e) => {
            set_error(format!("clear all bp error: {}", e));
            -2
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_flash_elf(
    chip: *const c_char,
    path: *const c_char,
    verify: i32,
    preverify: i32,
    chip_erase: i32,
    speed_khz: u32,
    protocol_code: i32,
) -> i32 {
    let chip = match cstr_to_string(chip) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 1;
        }
    };
    let path = match cstr_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 1;
        }
    };
    let fmt = Format::from(FormatKind::Elf);
    do_flash(
        &chip,
        &path,
        fmt,
        verify,
        preverify,
        chip_erase,
        speed_khz,
        protocol_from_int(protocol_code),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_flash_hex(
    chip: *const c_char,
    path: *const c_char,
    verify: i32,
    preverify: i32,
    chip_erase: i32,
    speed_khz: u32,
    protocol_code: i32,
) -> i32 {
    let chip = match cstr_to_string(chip) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 1;
        }
    };
    let path = match cstr_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 1;
        }
    };
    let fmt = Format::from(FormatKind::Hex);
    do_flash(
        &chip,
        &path,
        fmt,
        verify,
        preverify,
        chip_erase,
        speed_khz,
        protocol_from_int(protocol_code),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_flash_bin(
    chip: *const c_char,
    path: *const c_char,
    base_address: u64,
    skip: u32,
    verify: i32,
    preverify: i32,
    chip_erase: i32,
    speed_khz: u32,
    protocol_code: i32,
) -> i32 {
    let chip = match cstr_to_string(chip) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 1;
        }
    };
    let path = match cstr_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 1;
        }
    };
    let fmt = Format::Bin(BinOptions {
        base_address: Some(base_address),
        skip,
    });
    do_flash(
        &chip,
        &path,
        fmt,
        verify,
        preverify,
        chip_erase,
        speed_khz,
        protocol_from_int(protocol_code),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_flash_auto(
    chip: *const c_char,
    path: *const c_char,
    base_address: u64,
    skip: u32,
    verify: i32,
    preverify: i32,
    chip_erase: i32,
    speed_khz: u32,
    protocol_code: i32,
) -> i32 {
    let chip = match cstr_to_string(chip) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 1;
        }
    };
    let path = match cstr_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 1;
        }
    };
    let fmt = match detect_format_from_path(&path, Some(base_address).filter(|v| *v != 0), skip) {
        Ok(f) => f,
        Err(msg) => {
            set_error(msg);
            return 1;
        }
    };
    do_flash(
        &chip,
        &path,
        fmt,
        verify,
        preverify,
        chip_erase,
        speed_khz,
        protocol_from_int(protocol_code),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn version_roundtrip() {
        let need = pr_version(std::ptr::null_mut(), 0);
        assert!(need > 0);
        let mut buf = vec![0u8; need];
        let wrote = pr_version(buf.as_mut_ptr() as *mut i8, buf.len());
        assert_eq!(wrote, need);
    }

    #[test]
    fn invalid_chip_sets_error() {
        let chip = CString::new("not_a_real_chip").unwrap();
        let handle = pr_session_open_auto(chip.as_ptr(), 0, 0);
        assert_eq!(handle, 0);
        let need = pr_last_error(std::ptr::null_mut(), 0);
        assert!(need > 0);
    }

    #[test]
    fn detect_format_kind_exts() {
        assert!(matches!(
            detect_format_kind("firmware.elf"),
            Some(FormatKind::Elf)
        ));
        assert!(matches!(
            detect_format_kind("app.axf"),
            Some(FormatKind::Elf)
        ));
        assert!(matches!(
            detect_format_kind("image.hex"),
            Some(FormatKind::Hex)
        ));
        assert!(matches!(
            detect_format_kind("image.ihex"),
            Some(FormatKind::Hex)
        ));
        assert!(matches!(detect_format_kind("blob.bin"), None));
        assert!(matches!(detect_format_kind("unknown.xyz"), None));
    }

    #[test]
    fn detect_format_from_path_bin_requires_base() {
        let ok = detect_format_from_path("blob.bin", Some(0x08000000), 0);
        assert!(ok.is_ok());
        let err = detect_format_from_path("blob.bin", None, 0);
        assert!(err.is_err());
    }

    #[test]
    fn detect_format_from_path_elf_hex() {
        let ok_elf = detect_format_from_path("firmware.elf", None, 0);
        assert!(ok_elf.is_ok());
        let ok_hex = detect_format_from_path("image.hex", None, 0);
        assert!(ok_hex.is_ok());
    }

    #[test]
    fn chip_manufacturer_count_is_nonzero() {
        let n = pr_chip_manufacturer_count();
        assert!(n > 0);
    }

    #[test]
    fn chip_specs_by_name_returns_string() {
        let name = CString::new("nrf51822_Xxaa").unwrap();
        let need = pr_chip_specs_by_name(name.as_ptr(), std::ptr::null_mut(), 0);
        assert!(need > 0);
        let mut buf = vec![0u8; need];
        let wrote = pr_chip_specs_by_name(name.as_ptr(), buf.as_mut_ptr() as *mut i8, buf.len());
        assert_eq!(wrote, need);
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("\"chip\":"));
    }

    #[test]
    fn chip_model_listing_has_entries() {
        let m = pr_chip_manufacturer_count();
        assert!(m > 0);
        for mi in 0..m.min(32) {
            // limit iterations
            let c = pr_chip_model_count(mi);
            if c > 0 {
                let need = pr_chip_model_name(mi, 0, std::ptr::null_mut(), 0);
                assert!(need > 0);
                let mut buf = vec![0u8; need];
                let wrote = pr_chip_model_name(mi, 0, buf.as_mut_ptr() as *mut i8, buf.len());
                assert_eq!(wrote, need);
                let cname = String::from_utf8_lossy(&buf);
                assert!(cname.trim_end_matches('\0').len() > 0);
                return;
            }
        }
        panic!("no manufacturer with models found");
    }
}
// removed string-based programmer type setters/getters; use enum-based APIs and conversion helpers

#[unsafe(no_mangle)]
pub extern "C" fn pr_set_programmer_type_code(type_code: i32) -> i32 {
    let Some(ty) = code_to_type(type_code) else {
        set_error("unsupported programmer type code".to_string());
        return -1;
    };
    let lock = programmer_type_lock();
    let mut l = lock.lock().unwrap();
    *l = Some(ty);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_get_programmer_type_code() -> i32 {
    let lock = programmer_type_lock();
    let l = lock.lock().unwrap();
    match *l {
        Some(t) => type_to_code(t),
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_programmer_type_is_supported_code(type_code: i32) -> i32 {
    code_to_type(type_code).map(|_| 1).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_programmer_type_to_string(
    type_code: i32,
    buf: *mut c_char,
    buf_len: usize,
) -> usize {
    let s = match code_to_type(type_code) {
        Some(t) => type_to_str(t),
        None => "",
    };
    let bytes = s.as_bytes();
    let need = bytes.len() + 1;
    if buf.is_null() || buf_len == 0 {
        return need;
    }
    let copy = need.min(buf_len);
    unsafe {
        let slice = std::slice::from_raw_parts_mut(buf as *mut u8, copy);
        let n = copy.saturating_sub(1);
        slice[..n].copy_from_slice(&bytes[..n]);
        slice[n] = 0;
    }
    need
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_programmer_type_from_string(
    type_name: *const c_char,
    out_code: *mut i32,
) -> i32 {
    if out_code.is_null() {
        return -1;
    }
    let Ok(name) = cstr_to_string(type_name) else {
        return -1;
    };
    match parse_programmer_type(&name) {
        Some(t) => {
            unsafe { *out_code = type_to_code(t) };
            0
        }
        None => -1,
    }
}
