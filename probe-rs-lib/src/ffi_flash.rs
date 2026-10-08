use super::*;
use probe_rs::flashing::{FlashLayout, ProgressEvent, ProgressOperation};

pub type PrProgressCallback = unsafe extern "C" fn(*const PrProgressEvent, *mut c_void);

#[repr(C)]
pub struct PrDownloadOptions {
    pub keep_unwritten_bytes: i32,
    pub dry_run: i32,
    pub do_chip_erase: i32,
    pub skip_erase: i32,
    pub preverify: i32,
    pub verify: i32,
    pub disable_double_buffering: i32,
    pub preferred_algos: *const *const c_char,
    pub preferred_algos_len: usize,
    pub has_ram_chunk_size: i32,
    pub ram_chunk_size: u64,
}

#[repr(C)]
pub struct PrImageOptions {
    pub format: i32, // 1=ELF, 2=HEX, 3=BIN
    pub has_base_address: i32,
    pub base_address: u64,
    pub skip: u32,
    pub skip_sections: *const *const c_char,
    pub skip_sections_len: usize,
}

#[repr(C)]
pub struct PrFlashSpan {
    pub address: u64,
    pub size: u64,
}

#[repr(C)]
pub struct PrFlashPage {
    pub address: u64,
    pub size: u32,
    pub data: *const u8,
}

#[repr(C)]
pub struct PrFlashFill {
    pub address: u64,
    pub size: u64,
    pub page_index: usize,
}

#[repr(C)]
pub struct PrFlashLayout {
    pub sectors: *const PrFlashSpan,
    pub sector_count: usize,
    pub pages: *const PrFlashPage,
    pub page_count: usize,
    pub fills: *const PrFlashFill,
    pub fill_count: usize,
    pub data_blocks: *const PrFlashSpan,
    pub data_block_count: usize,
}

#[repr(C)]
pub struct PrProgressEvent {
    pub kind: i32, // 1=layout, 2=add-bar, 3=start, 4=progress, 5=finish, 6=fail, 7=message
    pub operation: i32, // 0=Fill, 1=Erase, 2=Program, 3=Verify, 4=Ram; -1 when absent
    pub has_total: i32,
    pub total: u64,
    pub size: u64,
    pub duration_ns: u64,
    pub message: *const u8,
    pub message_len: usize,
    pub layouts: *const PrFlashLayout,
    pub layout_count: usize,
}

struct OwnedLayout {
    sectors: Vec<PrFlashSpan>,
    pages: Vec<PrFlashPage>,
    fills: Vec<PrFlashFill>,
    data_blocks: Vec<PrFlashSpan>,
    // Keep the source pages (including their byte buffers) alive during the callback.
    source: FlashLayout,
}

fn operation_code(operation: ProgressOperation) -> i32 {
    match operation {
        ProgressOperation::Fill => 0,
        ProgressOperation::Erase => 1,
        ProgressOperation::Program => 2,
        ProgressOperation::Verify => 3,
        ProgressOperation::Ram => 4,
    }
}

fn forward_event(event: ProgressEvent, callback: PrProgressCallback, context: *mut c_void) {
    let mut output = PrProgressEvent {
        kind: 0,
        operation: -1,
        has_total: 0,
        total: 0,
        size: 0,
        duration_ns: 0,
        message: std::ptr::null(),
        message_len: 0,
        layouts: std::ptr::null(),
        layout_count: 0,
    };
    let mut message = None;
    let mut owned_layouts = Vec::new();
    let mut layout_views = Vec::new();
    match event {
        ProgressEvent::FlashLayoutReady { flash_layout } => {
            output.kind = 1;
            for source in flash_layout {
                let sectors = source.sectors().iter().map(|v| PrFlashSpan { address: v.address(), size: v.size() }).collect();
                let pages = source.pages().iter().map(|v| PrFlashPage { address: v.address(), size: v.size(), data: v.data().as_ptr() }).collect();
                let fills = source.fills().iter().map(|v| PrFlashFill { address: v.address(), size: v.size(), page_index: v.page_index() }).collect();
                let data_blocks = source.data_blocks().iter().map(|v| PrFlashSpan { address: v.address(), size: v.size() }).collect();
                owned_layouts.push(OwnedLayout { sectors, pages, fills, data_blocks, source });
            }
            for layout in &owned_layouts {
                layout_views.push(PrFlashLayout {
                    sectors: layout.sectors.as_ptr(), sector_count: layout.sectors.len(),
                    pages: layout.pages.as_ptr(), page_count: layout.pages.len(),
                    fills: layout.fills.as_ptr(), fill_count: layout.fills.len(),
                    data_blocks: layout.data_blocks.as_ptr(), data_block_count: layout.data_blocks.len(),
                });
            }
            output.layouts = layout_views.as_ptr();
            output.layout_count = layout_views.len();
        }
        ProgressEvent::AddProgressBar { operation, total } => {
            output.kind = 2;
            output.operation = operation_code(operation);
            if let Some(total) = total { output.has_total = 1; output.total = total; }
        }
        ProgressEvent::Started(operation) => { output.kind = 3; output.operation = operation_code(operation); }
        ProgressEvent::Progress { operation, size, time } => {
            output.kind = 4;
            output.operation = operation_code(operation);
            output.size = size;
            output.duration_ns = time.as_nanos().min(u64::MAX as u128) as u64;
        }
        ProgressEvent::Finished(operation) => { output.kind = 5; output.operation = operation_code(operation); }
        ProgressEvent::Failed(operation) => { output.kind = 6; output.operation = operation_code(operation); }
        ProgressEvent::DiagnosticMessage { message: text } => {
            output.kind = 7;
            message = Some(text);
            output.message = message.as_ref().unwrap().as_ptr();
            output.message_len = message.as_ref().unwrap().len();
        }
    }
    unsafe { callback(&output, context) };
    // These owners intentionally remain live until the callback returns.
    drop((message, layout_views, owned_layouts));
}

fn make_progress(callback: Option<PrProgressCallback>, context: *mut c_void) -> FlashProgress<'static> {
    match callback {
        Some(callback) => FlashProgress::new(move |event| forward_event(event, callback, context)),
        None => FlashProgress::empty(),
    }
}

fn read_string_array(ptr: *const *const c_char, len: usize) -> Result<Vec<String>, String> {
    if len == 0 { return Ok(Vec::new()); }
    if ptr.is_null() { return Err("string array is null".into()); }
    let values = unsafe { std::slice::from_raw_parts(ptr, len) };
    values.iter().map(|&value| cstr_to_string(value)).collect()
}

fn download_options(raw: *const PrDownloadOptions) -> Result<DownloadOptions<'static>, String> {
    let mut options = DownloadOptions::default();
    if raw.is_null() { return Ok(options); }
    let raw = unsafe { &*raw };
    options.keep_unwritten_bytes = raw.keep_unwritten_bytes != 0;
    options.dry_run = raw.dry_run != 0;
    options.do_chip_erase = raw.do_chip_erase != 0;
    options.skip_erase = raw.skip_erase != 0;
    options.preverify = raw.preverify != 0;
    options.verify = raw.verify != 0;
    options.disable_double_buffering = raw.disable_double_buffering != 0;
    options.preferred_algos = read_string_array(raw.preferred_algos, raw.preferred_algos_len)?;
    options.ram_chunk_size = (raw.has_ram_chunk_size != 0).then_some(raw.ram_chunk_size);
    Ok(options)
}

fn image_loader(raw: *const PrImageOptions) -> Result<Box<dyn ImageLoader>, String> {
    if raw.is_null() { return Err("image options are null".into()); }
    let raw = unsafe { &*raw };
    if raw.format != 1 && raw.skip_sections_len != 0 { return Err("skip_sections applies only to ELF".into()); }
    if raw.format != 3 && (raw.has_base_address != 0 || raw.skip != 0) { return Err("base_address and skip apply only to BIN".into()); }
    match raw.format {
        1 => Ok(Box::new(ElfLoader(ElfOptions { skip_sections: read_string_array(raw.skip_sections, raw.skip_sections_len)? }))),
        2 => Ok(Box::new(HexLoader)),
        3 => Ok(Box::new(BinLoader(BinOptions { base_address: (raw.has_base_address != 0).then_some(raw.base_address), skip: raw.skip }))),
        _ => Err("unsupported image format".into()),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_erase_all(session: u64, callback: Option<PrProgressCallback>, context: *mut c_void) -> i32 {
    let session = match get_session(session) { Ok(v) => v, Err(e) => { set_error(e); return -1; } };
    let mut session = session.lock().unwrap();
    let mut progress = make_progress(callback, context);
    match flashing::erase_all(&mut session, &mut progress, false) {
        Ok(()) => 0,
        Err(e) => { set_error(error_chain(&e)); -1 }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pr_session_flash(session: u64, path: *const c_char, image: *const PrImageOptions, options: *const PrDownloadOptions, callback: Option<PrProgressCallback>, context: *mut c_void) -> i32 {
    let path = match cstr_to_string(path) { Ok(v) => v, Err(e) => { set_error(e); return -1; } };
    let image = match image_loader(image) { Ok(v) => v, Err(e) => { set_error(e); return -1; } };
    let mut options = match download_options(options) { Ok(v) => v, Err(e) => { set_error(e); return -1; } };
    let session = match get_session(session) { Ok(v) => v, Err(e) => { set_error(e); return -1; } };
    options.progress = make_progress(callback, context);
    let mut session = session.lock().unwrap();
    match flashing::download_file_with_options(&mut session, path, image, options) {
        Ok(()) => 0,
        Err(e) => { set_error(error_chain(&e)); -1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_session_does_not_attach() {
        assert_eq!(pr_session_erase_all(u64::MAX, None, std::ptr::null_mut()), -1);
    }

    #[test]
    fn bin_zero_address_is_present() {
        let raw = PrImageOptions { format: 3, has_base_address: 1, base_address: 0, skip: 0, skip_sections: std::ptr::null(), skip_sections_len: 0 };
        assert!(image_loader(&raw).is_ok());
    }

    #[test]
    fn raw_progress_preserves_unknown_total_and_message_bytes() {
        unsafe extern "C" fn capture(event: *const PrProgressEvent, context: *mut c_void) {
            let output = unsafe { &mut *(context as *mut Vec<(i32, i32, Vec<u8>)>) };
            let event = unsafe { &*event };
            let message = if event.message.is_null() { Vec::new() } else {
                unsafe { std::slice::from_raw_parts(event.message, event.message_len) }.to_vec()
            };
            output.push((event.kind, event.has_total, message));
        }
        let mut events = Vec::new();
        let context = (&mut events as *mut Vec<(i32, i32, Vec<u8>)>).cast();
        forward_event(ProgressEvent::AddProgressBar { operation: ProgressOperation::Erase, total: None }, capture, context);
        forward_event(ProgressEvent::DiagnosticMessage { message: "a\0b".into() }, capture, context);
        assert_eq!(events, vec![(2, 0, vec![]), (7, 0, b"a\0b".to_vec())]);
    }
}
