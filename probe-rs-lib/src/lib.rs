use probe_rs::config::{Registry, TargetSelector};
use probe_rs::flashing::{
    self, BinLoader, BinOptions, DownloadOptions, ElfLoader, ElfOptions, FlashProgress, HexLoader,
    ImageLoader,
};
use probe_rs::probe::{DebugProbeSelector, Probe, WireProtocol, list::Lister};
use probe_rs::probe::{
    ch347::Ch347Factory, cmsisdap::CmsisDapFactory, ftdi::FtdiProbeFactory,
    glasgow::GlasgowFactory, jlink::JLinkFactory,
    sifliuart::SifliUartFactory, stlink::StLinkFactory, wlink::WchLinkFactory,
};
use probe_rs_espressif::espusbjtag::EspUsbJtagFactory;
use probe_rs::{BreakpointCause, CoreStatus, HaltReason, MemoryInterface, Permissions, Session, SessionConfig};
use probe_rs_target::MemoryRegion;
use std::collections::HashMap;
use std::cell::RefCell;
use std::error::Error;
use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};

mod ffi_flash;

thread_local! { static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) }; }
static SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<Mutex<Session>>>>> = OnceLock::new();
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
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
static PLUGINS: Once = Once::new();

fn initialize_plugins() {
    PLUGINS.call_once(probe_rs_espressif::register_plugin);
}

fn lister() -> Lister {
    initialize_plugins();
    Lister::new()
}

fn registry() -> &'static Registry {
    initialize_plugins();
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
        manufacturers[idx].chips.extend(
            family
                .variants
                .iter()
                .flat_map(|chip| chip.package_variants().cloned()),
        );
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
        .map(|c| serde_json::json!({"name": c.name, "type": format!("{:?}", c.core_type)}))
        .collect::<Vec<_>>();

    let mut ram_total: u64 = 0;
    let mut nvm_total: u64 = 0;
    let mut regions = Vec::new();
    for region in target.memory_map.iter() {
        match region {
            MemoryRegion::Ram(r) => {
                let size = r.range.end.saturating_sub(r.range.start);
                ram_total = ram_total.saturating_add(size);
                regions.push(serde_json::json!({"kind": "Ram", "start": r.range.start, "end": r.range.end}));
            }
            MemoryRegion::Nvm(n) => {
                let size = n.range.end.saturating_sub(n.range.start);
                nvm_total = nvm_total.saturating_add(size);
                regions.push(serde_json::json!({"kind": "Nvm", "start": n.range.start, "end": n.range.end, "is_alias": n.is_alias}));
            }
            MemoryRegion::Generic(g) => {
                regions.push(serde_json::json!({"kind": "Generic", "start": g.range.start, "end": g.range.end}));
            }
        }
    }

    let flash_algos = target
        .flash_algorithms
        .iter()
        .map(|a| a.name.clone())
        .collect::<Vec<_>>();
    let default_fmt = target.default_format.clone();

    Ok(serde_json::json!({
        "manufacturer": manufacturer,
        "chip": chip_name,
        "architecture": arch,
        "cores": cores,
        "ram_bytes": ram_total,
        "nvm_bytes": nvm_total,
        "regions": regions,
        "flash_algorithms": flash_algos,
        "default_format": default_fmt,
    })
    .to_string())
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
pub extern "C" fn pr_chip_model_count(manufacturer_index: u32, out_count: *mut u32) -> i32 {
    if out_count.is_null() { set_error("out_count is null".into()); return -1; }
    let db = chip_db();
    let Some(m) = db.manufacturers.get(manufacturer_index as usize) else {
        set_error("manufacturer index out of range".to_string());
        return -1;
    };
    unsafe { *out_count = m.chips.len() as u32; }
    0
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
    LAST_ERROR.with(|last| *last.borrow_mut() = msg);
}

fn error_chain(error: &dyn Error) -> String {
    let mut result = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        result.push_str(": ");
        result.push_str(&next.to_string());
        source = next.source();
    }
    result
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
        ProgrammerType::Ch347UsbJtag => info.is_probe_type::<Ch347Factory>(),
    }
}

fn write_c_string(value: &str, buf: *mut c_char, buf_len: usize) -> usize {
    let bytes = value.as_bytes();
    let needed = bytes.len() + 1;
    if !buf.is_null() && buf_len > 0 {
        let count = bytes.len().min(buf_len - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), count);
            *buf.add(count) = 0;
        }
    }
    needed
}

fn configure_probe(
    mut probe: Probe,
    speed_khz: u32,
    protocol: Option<WireProtocol>,
) -> Result<Probe, String> {
    if speed_khz > 0 {
        probe
            .set_speed(speed_khz)
            .map_err(|error| format!("set speed error: {}", error_chain(&error)))?;
    }
    if let Some(protocol) = protocol {
        probe
            .select_protocol(protocol)
            .map_err(|error| format!("select protocol error: {}", error_chain(&error)))?;
    }
    Ok(probe)
}

fn attach_selected(
    chip: TargetSelector,
    speed_khz: u32,
    protocol: Option<WireProtocol>,
    permissions: Permissions,
    programmer_type: Option<ProgrammerType>,
) -> Result<Session, String> {
    if let Some(kind) = programmer_type {
        let info = lister()
            .list_all()
            .into_iter()
            .find(|info| info_matches_type(info, kind))
            .ok_or_else(|| "no probe matching programmer type".to_string())?;
        let probe = info
            .open()
            .map_err(|error| format!("open probe error: {}", error_chain(&error)))?;
        configure_probe(probe, speed_khz, protocol)?
            .attach(chip, permissions)
            .map_err(|error| format!("attach error: {}", error_chain(&error)))
    } else {
        let config = SessionConfig {
            permissions,
            speed: (speed_khz > 0).then_some(speed_khz),
            protocol,
        };
        Session::auto_attach(chip, config).map_err(|error| format!("attach error: {}", error_chain(&error)))
    }
}

fn protocol_from_int(code: i32) -> Option<WireProtocol> {
    match code {
        1 => Some(WireProtocol::Swd),
        2 => Some(WireProtocol::Jtag),
        _ => None,
    }
}

fn target_selector(chip: *const c_char) -> Result<TargetSelector, String> {
    if chip.is_null() { Ok(TargetSelector::Auto) } else { cstr_to_string(chip).map(TargetSelector::from) }
}

fn requested_programmer_type(code: i32) -> Result<Option<ProgrammerType>, String> {
    if code == 0 { Ok(None) } else { code_to_type(code).map(Some).ok_or_else(|| "invalid programmer type code".into()) }
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
    let s = LAST_ERROR.with(|last| last.borrow().clone());
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
pub extern "C" fn pr_probe_count() -> u32 {
    let lister = lister();
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
    let lister = lister();
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
pub extern "C" fn pr_probe_driver_flags(index: u32, out_flags: *mut u32) -> i32 {
    if out_flags.is_null() { set_error("out_flags is null".into()); return -1; }
    let probes = lister().list_all();
    let Some(info) = probes.get(index as usize) else { set_error("probe index out of range".into()); return -1; };
    let mut flags = 0u32;
    if info.is_probe_type::<CmsisDapFactory>() { flags |= 1 << 0; }
    if info.is_probe_type::<JLinkFactory>() { flags |= 1 << 1; }
    if info.is_probe_type::<StLinkFactory>() { flags |= 1 << 2; }
    if info.is_probe_type::<FtdiProbeFactory>() { flags |= 1 << 3; }
    if info.is_probe_type::<EspUsbJtagFactory>() { flags |= 1 << 4; }
    if info.is_probe_type::<WchLinkFactory>() { flags |= 1 << 5; }
    if info.is_probe_type::<SifliUartFactory>() { flags |= 1 << 6; }
    if info.is_probe_type::<GlasgowFactory>() { flags |= 1 << 7; }
    if info.is_probe_type::<Ch347Factory>() { flags |= 1 << 8; }
    unsafe { *out_flags = flags; }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_open_auto(
    chip: *const c_char,
    speed_khz: u32,
    protocol_code: i32,
    allow_erase_all: i32,
    programmer_type_code: i32,
) -> u64 {
    let chip = match target_selector(chip) { Ok(v) => v, Err(e) => { set_error(e); return 0; } };
    let programmer_type = match requested_programmer_type(programmer_type_code) { Ok(v) => v, Err(e) => { set_error(e); return 0; } };
    let permissions = if allow_erase_all != 0 { Permissions::new().allow_erase_all() } else { Permissions::new() };
    match attach_selected(
        chip,
        speed_khz,
        protocol_from_int(protocol_code),
        permissions,
        programmer_type,
    ) {
        Ok(session) => make_handle(session),
        Err(error) => {
            set_error(error);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_open_with_probe(
    selector: *const c_char,
    chip: *const c_char,
    speed_khz: u32,
    protocol_code: i32,
    allow_erase_all: i32,
    programmer_type_code: i32,
) -> u64 {
    let Ok(sel) = cstr_to_string(selector) else {
        set_error("invalid selector".to_string());
        return 0;
    };
    let chip = match target_selector(chip) { Ok(v) => v, Err(e) => { set_error(e); return 0; } };
    let programmer_type = match requested_programmer_type(programmer_type_code) { Ok(v) => v, Err(e) => { set_error(e); return 0; } };
    let permissions = if allow_erase_all != 0 { Permissions::new().allow_erase_all() } else { Permissions::new() };
    let lister = lister();
    let selector: DebugProbeSelector = match sel.parse() {
        Ok(s) => s,
        Err(e) => {
            set_error(format!("selector parse error: {}", error_chain(&e)));
            return 0;
        }
    };
    let opened = if let Some(kind) = programmer_type {
        lister
            .list(Some(&selector))
            .into_iter()
            .find(|info| info_matches_type(info, kind))
            .ok_or_else(|| "probe not found or programmer type mismatch".to_string())
            .and_then(|info| info.open().map_err(|error| format!("open probe error: {}", error_chain(&error))))
    } else {
        lister
            .open(selector)
            .map_err(|error| format!("open probe error: {}", error_chain(&error)))
    };
    match opened
        .and_then(|probe| configure_probe(probe, speed_khz, protocol_from_int(protocol_code)))
        .and_then(|probe| {
            probe
                .attach(chip, permissions)
                .map_err(|error| format!("attach error: {}", error_chain(&error)))
        })
    {
        Ok(session) => make_handle(session),
        Err(error) => {
            set_error(error);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_target_info(session: u64, out_manufacturer_index: *mut u32, out_chip_index: *mut u32, name_buf: *mut c_char, name_buf_len: usize) -> usize {
    let session = match get_session(session) { Ok(v) => v, Err(e) => { set_error(e); return 0; } };
    let session = session.lock().unwrap();
    let name = &session.target().name;
    let (manufacturer_index, chip_index) = chip_db().name_to_index.get(name).copied().unwrap_or((u32::MAX, u32::MAX));
    unsafe {
        if !out_manufacturer_index.is_null() { *out_manufacturer_index = manufacturer_index; }
        if !out_chip_index.is_null() { *out_chip_index = chip_index; }
    }
    write_c_string(name, name_buf, name_buf_len)
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_close(session: u64) -> i32 {
    let mut map = sessions().lock().unwrap();
    match map.remove(&session) {
        Some(arc) => {
            drop(map);
            // Wait for an operation already using this handle before releasing the probe.
            drop(arc.lock().unwrap());
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
pub extern "C" fn pr_core_count(session: u64, out_count: *mut u32) -> i32 {
    if out_count.is_null() { set_error("out_count is null".into()); return -1; }
    let sess = match get_session(session) { Ok(v) => v, Err(e) => { set_error(e); return -1; } };
    let lock = sess.lock().unwrap();
    unsafe { *out_count = lock.list_cores().len() as u32; }
    0
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

#[repr(C)]
pub struct PrCoreStatus {
    pub state: i32, // 0=Unknown, 1=Running, 2=Halted, 3=LockedUp, 4=Sleeping
    pub halt_reason: i32, // 0=none/unknown, 1=Multiple, 2=Breakpoint, 3=Exception, 4=Watchpoint, 5=Step, 6=Request, 7=External
    pub breakpoint_cause: i32, // 0=none/unknown, 1=Hardware, 2=Software, 3=Semihosting
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_core_status(session: u64, core_index: u32, out_status: *mut PrCoreStatus, semihosting_buf: *mut c_char, semihosting_buf_len: usize, out_semihosting_len: *mut usize) -> i32 {
    if out_status.is_null() { set_error("out_status is null".into()); return -1; }
    let sess = match get_session(session) { Ok(v) => v, Err(e) => { set_error(e); return -1; } };
    let mut lock = sess.lock().unwrap();
    let mut core = match lock.core(core_index as usize) { Ok(v) => v, Err(e) => { set_error(format!("core access error: {e}")); return -1; } };
    let status = match core.status() { Ok(v) => v, Err(e) => { set_error(format!("status error: {e}")); return -1; } };
    let mut output = PrCoreStatus { state: 0, halt_reason: 0, breakpoint_cause: 0 };
    let mut semihosting = String::new();
    match status {
        CoreStatus::Unknown => {}
        CoreStatus::Running => output.state = 1,
        CoreStatus::LockedUp => output.state = 3,
        CoreStatus::Sleeping => output.state = 4,
        CoreStatus::Halted(reason) => {
            output.state = 2;
            output.halt_reason = match reason {
                HaltReason::Unknown => 0,
                HaltReason::Multiple => 1,
                HaltReason::Breakpoint(cause) => {
                    output.breakpoint_cause = match cause {
                        BreakpointCause::Unknown => 0,
                        BreakpointCause::Hardware => 1,
                        BreakpointCause::Software => 2,
                        BreakpointCause::Semihosting(command) => { semihosting = format!("{command:?}"); 3 }
                    };
                    2
                }
                HaltReason::Exception => 3,
                HaltReason::Watchpoint => 4,
                HaltReason::Step => 5,
                HaltReason::Request => 6,
                HaltReason::External => 7,
            };
        }
    }
    unsafe { *out_status = output; }
    let needed = write_c_string(&semihosting, semihosting_buf, semihosting_buf_len);
    unsafe { if !out_semihosting_len.is_null() { *out_semihosting_len = needed; } }
    0
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
pub extern "C" fn pr_read_16(
    session: u64,
    core_index: u32,
    address: u64,
    buf: *mut u16,
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
    let mut tmp = vec![0u16; len_words as usize];
    match lock.core(core_index as usize) {
        Ok(mut core) => match core.read_16(address, &mut tmp) {
            Ok(_) => {
                unsafe {
                    std::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, len_words as usize);
                }
                0
            }
            Err(e) => {
                set_error(format!("read_16 error: {}", e));
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
pub extern "C" fn pr_write_16(
    session: u64,
    core_index: u32,
    address: u64,
    buf: *const u16,
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
        Ok(mut core) => match core.write_16(address, slice) {
            Ok(_) => 0,
            Err(e) => {
                set_error(format!("write_16 error: {}", e));
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
pub extern "C" fn pr_registers_count(session: u64, core_index: u32, out_count: *mut u32) -> i32 {
    if out_count.is_null() { set_error("out_count is null".into()); return -1; }
    let sess = match get_session(session) { Ok(v) => v, Err(e) => { set_error(e); return -1; } };
    let mut lock = sess.lock().unwrap();
    match lock.core(core_index as usize) {
        Ok(core) => { unsafe { *out_count = core.registers().all_registers().count() as u32; } 0 },
        Err(e) => { set_error(format!("core access error: {e}")); -1 },
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_register_info(
    session: u64,
    core_index: u32,
    reg_index: u32,
    reg_id: *mut u16,
    bit_size: *mut u32,
    data_type: *mut i32,
    name: *mut c_char,
    name_len: usize,
    out_name_len: *mut usize,
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
        if !data_type.is_null() {
            *data_type = match desc.data_type {
                probe_rs::RegisterDataType::UnsignedInteger(_) => 1,
                probe_rs::RegisterDataType::FloatingPoint(_) => 2,
            };
        }
    }
    // Primary display name from register descriptor
    let name_str = desc.name();
    let needed = write_c_string(name_str, name, name_len);
    unsafe { if !out_name_len.is_null() { *out_name_len = needed; } }
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
        let handle = pr_session_open_auto(chip.as_ptr(), 0, 0, 0, 0);
        assert_eq!(handle, 0);
        let need = pr_last_error(std::ptr::null_mut(), 0);
        assert!(need > 0);
    }

    #[test]
    fn invalid_session_count_is_not_a_valid_zero() {
        let mut count = u32::MAX;
        assert_ne!(pr_core_count(u64::MAX, &mut count), 0);
        assert_eq!(count, u32::MAX);
    }

    #[test]
    fn invalid_manufacturer_count_is_not_a_valid_zero() {
        let mut count = u32::MAX;
        assert_ne!(pr_chip_model_count(u32::MAX, &mut count), 0);
        assert_eq!(count, u32::MAX);
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
        let s = String::from_utf8_lossy(&buf[..need - 1]);
        let spec: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(spec["chip"], "nrf51822_Xxaa");
        assert!(spec["cores"].is_array());
        assert!(spec["regions"].is_array());
        assert!(spec["flash_algorithms"].is_array());
    }

    #[test]
    fn chip_model_listing_has_entries() {
        let m = pr_chip_manufacturer_count();
        assert!(m > 0);
        for mi in 0..m.min(32) {
            // limit iterations
            let mut c = 0;
            assert_eq!(pr_chip_model_count(mi, &mut c), 0);
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
