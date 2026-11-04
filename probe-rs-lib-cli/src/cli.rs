use std::env;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::Foundation::FreeLibrary;
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

#[cfg(unix)]
#[cfg_attr(target_os = "linux", link(name = "dl"))]
unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
}

struct DynamicLibrary { handle: *mut c_void }
impl DynamicLibrary {
    fn open(path: &Path) -> Result<Self, String> {
        #[cfg(windows)]
        let handle = {
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe { LoadLibraryW(wide.as_ptr()) }
        };
        #[cfg(unix)]
        let handle = {
            let path = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            unsafe { dlopen(path.as_ptr(), 2) }
        };
        if handle.is_null() { return Err(format!("cannot load {}", path.display())); }
        Ok(Self { handle })
    }
    fn symbol(&self, name: &str) -> Result<*const (), String> {
        let name = CString::new(name).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let symbol = unsafe { GetProcAddress(self.handle, name.as_ptr().cast::<u8>()) }.map(|v| v as *const ());
        #[cfg(unix)]
        let symbol = Some(unsafe { dlsym(self.handle, name.as_ptr()) } as *const ()).filter(|v| !v.is_null());
        symbol.ok_or_else(|| format!("missing library symbol: {}", name.to_string_lossy()))
    }
}
impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe { FreeLibrary(self.handle); }
        #[cfg(unix)]
        unsafe { dlclose(self.handle); }
    }
}

#[repr(C)]
struct DownloadOptions {
    keep_unwritten_bytes: i32, dry_run: i32, do_chip_erase: i32, skip_erase: i32,
    preverify: i32, verify: i32, disable_double_buffering: i32,
    preferred_algos: *const *const c_char, preferred_algos_len: usize,
    has_ram_chunk_size: i32, ram_chunk_size: u64,
}
#[repr(C)]
struct ImageOptions {
    format: i32, has_base_address: i32, base_address: u64, skip: u32,
    skip_sections: *const *const c_char, skip_sections_len: usize,
}
#[repr(C)]
struct FlashLayout {
    sectors: *const c_void, sector_count: usize, pages: *const c_void, page_count: usize,
    fills: *const c_void, fill_count: usize, data_blocks: *const c_void, data_block_count: usize,
}
#[repr(C)]
struct ProgressEvent {
    kind: i32, operation: i32, has_total: i32, total: u64, size: u64, duration_ns: u64,
    message: *const u8, message_len: usize, layouts: *const FlashLayout, layout_count: usize,
}
type ProgressCallback = unsafe extern "C" fn(*const ProgressEvent, *mut c_void);

struct Ffi {
    _library: DynamicLibrary,
    last_error: unsafe extern "C" fn(*mut c_char, usize) -> usize,
    probe_count: unsafe extern "C" fn() -> u32,
    probe_info: unsafe extern "C" fn(u32, *mut c_char, usize, *mut u16, *mut u16, *mut c_char, usize) -> i32,
    probe_driver_flags: unsafe extern "C" fn(u32, *mut u32) -> i32,
    open_auto: unsafe extern "C" fn(*const c_char, u32, i32, i32, i32) -> u64,
    open_with_probe: unsafe extern "C" fn(*const c_char, *const c_char, u32, i32, i32, i32) -> u64,
    close: unsafe extern "C" fn(u64) -> i32,
    target_info: unsafe extern "C" fn(u64, *mut u32, *mut u32, *mut c_char, usize) -> usize,
    flash: unsafe extern "C" fn(u64, *const c_char, *const ImageOptions, *const DownloadOptions, Option<ProgressCallback>, *mut c_void) -> i32,
    erase_all: unsafe extern "C" fn(u64, Option<ProgressCallback>, *mut c_void) -> i32,
    read_16: unsafe extern "C" fn(u64, u32, u64, *mut u16, u32) -> i32,
    write_16: unsafe extern "C" fn(u64, u32, u64, *const u16, u32) -> i32,
    programmer_type_from_string: unsafe extern "C" fn(*const c_char, *mut i32) -> i32,
    chip_manufacturer_count: unsafe extern "C" fn() -> u32,
    chip_manufacturer_name: unsafe extern "C" fn(u32, *mut c_char, usize) -> usize,
    chip_model_count: unsafe extern "C" fn(u32, *mut u32) -> i32,
    chip_model_name: unsafe extern "C" fn(u32, u32, *mut c_char, usize) -> usize,
    chip_model_specs: unsafe extern "C" fn(u32, u32, *mut c_char, usize) -> usize,
    chip_specs_by_name: unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> usize,
}

fn load_ffi(path: &Path) -> Result<Ffi, String> {
    let library = DynamicLibrary::open(path)?;
    unsafe {
        let load = |name| library.symbol(name);
        Ok(Ffi {
            last_error: std::mem::transmute(load("pr_last_error")?),
            probe_count: std::mem::transmute(load("pr_probe_count")?),
            probe_info: std::mem::transmute(load("pr_probe_info")?),
            probe_driver_flags: std::mem::transmute(load("pr_probe_driver_flags")?),
            open_auto: std::mem::transmute(load("pr_session_open_auto")?),
            open_with_probe: std::mem::transmute(load("pr_session_open_with_probe")?),
            close: std::mem::transmute(load("pr_session_close")?),
            target_info: std::mem::transmute(load("pr_session_target_info")?),
            flash: std::mem::transmute(load("pr_session_flash")?),
            erase_all: std::mem::transmute(load("pr_session_erase_all")?),
            read_16: std::mem::transmute(load("pr_read_16")?),
            write_16: std::mem::transmute(load("pr_write_16")?),
            programmer_type_from_string: std::mem::transmute(load("pr_programmer_type_from_string")?),
            chip_manufacturer_count: std::mem::transmute(load("pr_chip_manufacturer_count")?),
            chip_manufacturer_name: std::mem::transmute(load("pr_chip_manufacturer_name")?),
            chip_model_count: std::mem::transmute(load("pr_chip_model_count")?),
            chip_model_name: std::mem::transmute(load("pr_chip_model_name")?),
            chip_model_specs: std::mem::transmute(load("pr_chip_model_specs")?),
            chip_specs_by_name: std::mem::transmute(load("pr_chip_specs_by_name")?),
            _library: library,
        })
    }
}

fn last_error(ffi: &Ffi) -> String {
    unsafe {
        let needed = (ffi.last_error)(std::ptr::null_mut(), 0);
        let mut buffer = vec![0u8; needed.max(1)];
        (ffi.last_error)(buffer.as_mut_ptr().cast(), buffer.len());
        String::from_utf8_lossy(&buffer[..needed.saturating_sub(1)]).into_owned()
    }
}

struct SessionGuard<'a> { ffi: &'a Ffi, handle: u64 }
impl Drop for SessionGuard<'_> {
    fn drop(&mut self) { unsafe { (self.ffi.close)(self.handle); } }
}

#[derive(Default, Debug)]
struct Args {
    op: Option<String>, chip: Option<String>, probe: Option<String>, file: Option<PathBuf>,
    format: Option<String>, speed: u32, protocol: i32, library: Option<PathBuf>,
    programmer_type: Option<String>, base: Option<u64>, skip: u32, len: u32, core: u32,
    data: Vec<u16>, verify: bool, preverify: bool, chip_erase: bool,
    keep_unwritten_bytes: bool, dry_run: bool, skip_erase: bool, disable_double_buffering: bool,
    preferred_algos: Vec<String>, ram_chunk_size: Option<u64>, skip_sections: Vec<String>,
}
fn number(text: &str) -> Result<u64, String> {
    let (digits, radix) = if let Some(v) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) { (v,16) }
        else if let Some(v) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) { (v,2) }
        else if let Some(v) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) { (v,8) }
        else { (text,10) };
    u64::from_str_radix(digits, radix).map_err(|_| format!("invalid number: {text}"))
}
impl Args {
    fn parse(values: impl IntoIterator<Item=String>) -> Result<Self, String> {
        let mut args = Self { speed: 4000, len: 1, ..Self::default() };
        let mut it = values.into_iter();
        while let Some(flag) = it.next() {
            let mut value = || -> Result<String, String> { it.next().ok_or_else(|| format!("missing value for {flag}")) };
            match flag.as_str() {
                "--op" => args.op = Some(value()?),
                "--chip" => args.chip = Some(value()?),
                "--probe" => args.probe = Some(value()?),
                "--file" => args.file = Some(PathBuf::from(value()?)),
                "--format" => args.format = Some(value()?.to_ascii_lowercase()),
                "--dll" => args.library = Some(PathBuf::from(value()?)),
                "--programmer-type" => args.programmer_type = Some(value()?),
                "--protocol" => args.protocol = match value()?.to_ascii_lowercase().as_str() { "auto"=>0,"swd"=>1,"jtag"=>2,other=>return Err(format!("invalid protocol: {other}")) },
                "--speed" => args.speed = u32::try_from(number(&value()?)?).map_err(|e| e.to_string())?,
                "--base" => args.base = Some(number(&value()?)?),
                "--skip" => args.skip = u32::try_from(number(&value()?)?).map_err(|e| e.to_string())?,
                "--len" => args.len = u32::try_from(number(&value()?)?).map_err(|e| e.to_string())?,
                "--core" => args.core = u32::try_from(number(&value()?)?).map_err(|e| e.to_string())?,
                "--data" => args.data = value()?.split(',').map(number).map(|v| v.and_then(|n| u16::try_from(n).map_err(|e| e.to_string()))).collect::<Result<_,_>>()?,
                "--verify" => args.verify = true,
                "--no-verify" => args.verify = false,
                "--preverify" => args.preverify = true,
                "--no-preverify" => args.preverify = false,
                "--chip-erase" => args.chip_erase = true,
                "--no-chip-erase" => args.chip_erase = false,
                "--keep-unwritten-bytes" => args.keep_unwritten_bytes = true,
                "--dry-run" => args.dry_run = true,
                "--skip-erase" => args.skip_erase = true,
                "--disable-double-buffering" => args.disable_double_buffering = true,
                "--preferred-algo" => args.preferred_algos.push(value()?),
                "--ram-chunk-size" => args.ram_chunk_size = Some(number(&value()?)?),
                "--skip-section" => args.skip_sections.push(value()?),
                "--help" => {
                    println!("Operations: --op list|detect|check|flash|erase-all|read16|write16|chips|spec\n\
                        Connection: --chip NAME --probe VID:PID[:SERIAL] --protocol auto|swd|jtag --speed KHZ --programmer-type TYPE\n\
                        Image: --file PATH --format elf|hex|bin --base ADDRESS --skip BYTES --skip-section NAME\n\
                        Flash options: --verify --preverify --chip-erase --keep-unwritten-bytes --dry-run --skip-erase\n\
                        --disable-double-buffering --preferred-algo NAME --ram-chunk-size BYTES\n\
                        Memory: --core INDEX --len WORDS --data WORD[,WORD...]\n\
                        Library: --dll PATH");
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument: {flag}")),
            }
        }
        Ok(args)
    }
    fn operation(&self) -> &str { self.op.as_deref().unwrap_or(if self.file.is_some() { "flash" } else { "check" }) }
}

fn library_path(args: &Args) -> Result<PathBuf, String> {
    if let Some(path) = &args.library { return Ok(path.clone()); }
    let mut path = env::current_exe().map_err(|e| e.to_string())?;
    path.set_file_name(format!("{}probe_rs_lib{}", env::consts::DLL_PREFIX, env::consts::DLL_SUFFIX));
    if path.is_file() { Ok(path) } else { Err(format!("library not found: {}", path.display())) }
}

fn c_string(value: &str) -> Result<CString, String> { CString::new(value).map_err(|e| e.to_string()) }
fn programmer_type_code(ffi: &Ffi, args: &Args) -> Result<i32, String> {
    let Some(name) = &args.programmer_type else { return Ok(0); };
    let name = c_string(name)?;
    let mut code = 0;
    if unsafe { (ffi.programmer_type_from_string)(name.as_ptr(), &mut code) } != 0 { return Err("invalid programmer type".into()); }
    Ok(code)
}
fn open_session<'a>(ffi: &'a Ffi, args: &Args, allow_erase_all: bool) -> Result<SessionGuard<'a>, String> {
    let chip = args.chip.as_deref().map(c_string).transpose()?;
    let probe = args.probe.as_deref().map(c_string).transpose()?;
    let chip_ptr = chip.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
    let code = programmer_type_code(ffi, args)?;
    let handle = unsafe {
        if let Some(selector) = probe { (ffi.open_with_probe)(selector.as_ptr(), chip_ptr, args.speed, args.protocol, allow_erase_all as i32, code) }
        else { (ffi.open_auto)(chip_ptr, args.speed, args.protocol, allow_erase_all as i32, code) }
    };
    if handle == 0 { Err(last_error(ffi)) } else { Ok(SessionGuard { ffi, handle }) }
}

unsafe extern "C" fn progress(event: *const ProgressEvent, _: *mut c_void) {
    if event.is_null() { return; }
    let event = unsafe { &*event };
    let operation = match event.operation { 0=>"fill",1=>"erase",2=>"program",3=>"verify",4=>"ram",_=>"" };
    match event.kind {
        1 => println!("layout ready: {} regions", event.layout_count),
        2 => if event.has_total != 0 { println!("{operation}: total {} bytes", event.total); } else { println!("{operation}: total unknown"); },
        3 => println!("{operation}: started"),
        4 => println!("{operation}: +{} bytes in {} ns", event.size, event.duration_ns),
        5 => println!("{operation}: finished"),
        6 => println!("{operation}: failed"),
        7 => if !event.message.is_null() { let bytes = unsafe { std::slice::from_raw_parts(event.message, event.message_len) }; println!("diagnostic: {}", String::from_utf8_lossy(bytes)); },
        _ => {}
    }
}

fn read_text(needed: usize, read: impl FnOnce(*mut c_char, usize)) -> String {
    let mut buf = vec![0u8; needed.max(1)];
    read(buf.as_mut_ptr().cast(), buf.len());
    String::from_utf8_lossy(&buf[..needed.saturating_sub(1)]).into_owned()
}

fn run() -> Result<(), String> {
    let args = Args::parse(env::args().skip(1))?;
    let ffi = load_ffi(&library_path(&args)?)?;
    match args.operation() {
        "list" => unsafe {
            let count = (ffi.probe_count)();
            println!("Found {count} probes");
            for index in 0..count {
                let (mut name, mut serial, mut vid, mut pid, mut driver) = (vec![0 as c_char; 256], vec![0 as c_char; 256], 0, 0, 0);
                if (ffi.probe_info)(index, name.as_mut_ptr(), name.len(), &mut vid, &mut pid, serial.as_mut_ptr(), serial.len()) != 0 { return Err(last_error(&ffi)); }
                if (ffi.probe_driver_flags)(index, &mut driver) != 0 { return Err(last_error(&ffi)); }
                println!("[{index}] {} {vid:04x}:{pid:04x} SN={} driver=0x{driver:08x}", CStr::from_ptr(name.as_ptr()).to_string_lossy(), CStr::from_ptr(serial.as_ptr()).to_string_lossy());
            }
        },
        "detect" | "check" => {
            let session = open_session(&ffi, &args, false)?;
            let mut manufacturer = u32::MAX; let mut chip = u32::MAX;
            let needed = unsafe { (ffi.target_info)(session.handle, &mut manufacturer, &mut chip, std::ptr::null_mut(), 0) };
            if needed == 0 { return Err(last_error(&ffi)); }
            let name = read_text(needed, |ptr, len| { unsafe { (ffi.target_info)(session.handle, &mut manufacturer, &mut chip, ptr, len); } });
            println!("Target: {name} (manufacturer={manufacturer}, chip={chip})");
        }
        "flash" => {
            args.chip.as_deref().ok_or("--chip required for flash")?;
            let path = args.file.as_ref().ok_or("--file required")?;
            let format = args.format.as_deref().map(str::to_owned).unwrap_or_else(|| path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase());
            let format = match format.as_str() { "elf"|"axf"=>1,"hex"|"ihex"=>2,"bin"=>3,_=>return Err("unknown image format; use --format elf|hex|bin".into()) };
            if format != 1 && !args.skip_sections.is_empty() { return Err("--skip-section requires ELF".into()); }
            if format != 3 && (args.base.is_some() || args.skip != 0) { return Err("--base and --skip require BIN".into()); }
            let path = c_string(path.to_str().ok_or("non-UTF-8 path")?)?;
            let section_names = args.skip_sections.iter().map(|s| c_string(s)).collect::<Result<Vec<_>,_>>()?;
            let section_ptrs = section_names.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
            let algo_names = args.preferred_algos.iter().map(|s| c_string(s)).collect::<Result<Vec<_>,_>>()?;
            let algo_ptrs = algo_names.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
            let image = ImageOptions { format, has_base_address: args.base.is_some() as i32, base_address: args.base.unwrap_or(0), skip: args.skip, skip_sections: section_ptrs.as_ptr(), skip_sections_len: section_ptrs.len() };
            let options = DownloadOptions { keep_unwritten_bytes: args.keep_unwritten_bytes as i32, dry_run: args.dry_run as i32, do_chip_erase: args.chip_erase as i32, skip_erase: args.skip_erase as i32, preverify: args.preverify as i32, verify: args.verify as i32, disable_double_buffering: args.disable_double_buffering as i32, preferred_algos: algo_ptrs.as_ptr(), preferred_algos_len: algo_ptrs.len(), has_ram_chunk_size: args.ram_chunk_size.is_some() as i32, ram_chunk_size: args.ram_chunk_size.unwrap_or(0) };
            let session = open_session(&ffi, &args, args.chip_erase)?;
            if unsafe { (ffi.flash)(session.handle, path.as_ptr(), &image, &options, Some(progress), std::ptr::null_mut()) } != 0 { return Err(last_error(&ffi)); }
            println!("Flash complete");
        }
        "erase-all" => {
            args.chip.as_deref().ok_or("--chip required for erase-all")?;
            let session = open_session(&ffi, &args, true)?;
            if unsafe { (ffi.erase_all)(session.handle, Some(progress), std::ptr::null_mut()) } != 0 { return Err(last_error(&ffi)); }
            println!("Erase complete");
        }
        "read16" => {
            args.chip.as_deref().ok_or("--chip required for read16")?;
            let session = open_session(&ffi, &args, false)?;
            let mut data = vec![0u16; args.len as usize];
            if unsafe { (ffi.read_16)(session.handle, args.core, args.base.unwrap_or(0), data.as_mut_ptr(), args.len) } != 0 { return Err(last_error(&ffi)); }
            println!("{data:x?}");
        }
        "write16" => {
            args.chip.as_deref().ok_or("--chip required for write16")?;
            if args.data.is_empty() { return Err("--data required".into()); }
            let session = open_session(&ffi, &args, false)?;
            if unsafe { (ffi.write_16)(session.handle, args.core, args.base.unwrap_or(0), args.data.as_ptr(), args.data.len() as u32) } != 0 { return Err(last_error(&ffi)); }
            println!("Write complete");
        }
        "chips" => unsafe {
            let count = (ffi.chip_manufacturer_count)();
            for mi in 0..count {
                let n = (ffi.chip_manufacturer_name)(mi, std::ptr::null_mut(), 0);
                let name = read_text(n, |ptr, len| { (ffi.chip_manufacturer_name)(mi, ptr, len); });
                println!("[{mi}] {name}");
                let mut models = 0;
                if (ffi.chip_model_count)(mi, &mut models) != 0 { return Err(last_error(&ffi)); }
                for ci in 0..models {
                    let n = (ffi.chip_model_name)(mi, ci, std::ptr::null_mut(), 0);
                    println!("  {}", read_text(n, |ptr,len| { (ffi.chip_model_name)(mi,ci,ptr,len); }));
                    let spec_len = (ffi.chip_model_specs)(mi, ci, std::ptr::null_mut(), 0);
                    if spec_len == 0 { return Err(last_error(&ffi)); }
                    println!("    {}", read_text(spec_len, |ptr,len| { (ffi.chip_model_specs)(mi,ci,ptr,len); }));
                }
            }
        },
        "spec" => unsafe {
            let chip = c_string(args.chip.as_deref().ok_or("--chip required")?)?;
            let n = (ffi.chip_specs_by_name)(chip.as_ptr(), std::ptr::null_mut(), 0);
            if n == 0 { return Err(last_error(&ffi)); }
            println!("{}", read_text(n, |ptr,len| { (ffi.chip_specs_by_name)(chip.as_ptr(),ptr,len); }));
        },
        other => return Err(format!("unknown operation: {other}")),
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() { eprintln!("{error}"); std::process::exit(1); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_probe_rs() {
        let args = Args::parse(Vec::<String>::new()).unwrap();
        assert!(!args.verify && !args.preverify && !args.chip_erase);
    }
    #[test]
    fn explicit_zero_base_and_format_survive_parsing() {
        let args = Args::parse(["--format","bin","--base","0","--chip-erase"].map(String::from)).unwrap();
        assert_eq!(args.base, Some(0));
        assert_eq!(args.format.as_deref(), Some("bin"));
        assert!(args.chip_erase);
    }
}
