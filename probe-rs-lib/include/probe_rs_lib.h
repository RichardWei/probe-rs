#pragma once
/* C-compatible API for probe-rs. Returned session handles own the connection. */
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 Error API
 - Retrieve the calling thread's last error string. If buf==NULL or buf_len==0,
   returns the required size (including NUL). Read it immediately after failure.
*/
size_t pr_last_error(char* buf, size_t buf_len);

/*
 Version API
 - Returns the library version string length (including NUL). If buf provided, writes the version string.
*/
size_t pr_version(char* buf, size_t buf_len);

/*
 Probe listing
 - Count connected debug probes
 - Query probe info (identifier, VID, PID, optional serial)
*/
uint32_t pr_probe_count(void);
int32_t pr_probe_info(uint32_t index,
                      char* identifier, size_t identifier_len,
                      uint16_t* vid, uint16_t* pid,
                      char* serial, size_t serial_len);

/* Driver flags are static metadata; querying them does not open a probe. */

/* Driver flag bits */
#define PR_DRIVER_CMSISDAP      0x00000001u
#define PR_DRIVER_JLINK         0x00000002u
#define PR_DRIVER_STLINK        0x00000004u
#define PR_DRIVER_FTDI          0x00000008u
#define PR_DRIVER_ESP_USB_JTAG  0x00000010u
#define PR_DRIVER_WCHLINK       0x00000020u
#define PR_DRIVER_SIFLI_UART    0x00000040u
#define PR_DRIVER_GLASGOW       0x00000080u
#define PR_DRIVER_CH347_USBJTAG 0x00000100u

int32_t pr_probe_driver_flags(uint32_t index, uint32_t* out_flags);

/*
 Session management
 - chip==NULL selects TargetSelector::Auto; otherwise the supplied chip name is used.
 - protocol_code: 0=auto, 1=SWD, 2=JTAG; speed_khz=0 means not set.
 - allow_erase_all is passed to Permissions at attach time.
 - programmer_type_code: 0=no filter, otherwise a PR_PROG_* value.
*/
uint64_t pr_session_open_auto(const char* chip, uint32_t speed_khz, int32_t protocol_code,
                              int32_t allow_erase_all, int32_t programmer_type_code);
uint64_t pr_session_open_with_probe(const char* selector, const char* chip, uint32_t speed_khz,
                                    int32_t protocol_code, int32_t allow_erase_all, int32_t programmer_type_code);
int32_t pr_session_close(uint64_t session);
/* Reads the attached target without reopening the probe; size includes NUL. */
size_t pr_session_target_info(uint64_t session, uint32_t* out_manufacturer_index,
                              uint32_t* out_chip_index, char* name_buf, size_t name_buf_len);
int32_t pr_core_count(uint64_t session, uint32_t* out_count);

/*
 Core control
 - Halt/Run/Step/Reset/Reset-and-halt; returns 0 on success.
*/
int32_t pr_core_halt(uint64_t session, uint32_t core_index, uint32_t timeout_ms);
int32_t pr_core_run(uint64_t session, uint32_t core_index);
int32_t pr_core_step(uint64_t session, uint32_t core_index);
int32_t pr_core_reset(uint64_t session, uint32_t core_index);
int32_t pr_core_reset_and_halt(uint64_t session, uint32_t core_index, uint32_t timeout_ms);

typedef struct {
    int32_t state;             /* 0 Unknown, 1 Running, 2 Halted, 3 LockedUp, 4 Sleeping */
    int32_t halt_reason;       /* 0 None/Unknown, 1 Multiple, 2 Breakpoint, 3 Exception, 4 Watchpoint, 5 Step, 6 Request, 7 External */
    int32_t breakpoint_cause;  /* 0 None/Unknown, 1 Hardware, 2 Software, 3 Semihosting */
} pr_core_status_t;
/* Semihosting details, if present, are written as Rust Debug text. */
int32_t pr_core_status(uint64_t session, uint32_t core_index, pr_core_status_t* out_status,
                       char* semihosting_buf, size_t semihosting_buf_len, size_t* out_semihosting_len);

/*
 Memory operations
 - Read/Write 8-bit, 16-bit and 32-bit buffers.
*/
int32_t pr_read_8(uint64_t session, uint32_t core_index, uint64_t address, uint8_t* buf, uint32_t len);
int32_t pr_write_8(uint64_t session, uint32_t core_index, uint64_t address, const uint8_t* buf, uint32_t len);

/* Read 16-bit words from target memory. len_words is the number of u16 items. */
int32_t pr_read_16(uint64_t session, uint32_t core_index, uint64_t address, uint16_t* buf, uint32_t len_words);
/* Write 16-bit words to target memory. len_words is the number of u16 items. */
int32_t pr_write_16(uint64_t session, uint32_t core_index, uint64_t address, const uint16_t* buf, uint32_t len_words);

int32_t pr_read_32(uint64_t session, uint32_t core_index, uint64_t address, uint32_t* buf, uint32_t len_words);
int32_t pr_write_32(uint64_t session, uint32_t core_index, uint64_t address, const uint32_t* buf, uint32_t len_words);

/*
 Register operations
 - Enumerate register file and read/write by RegisterId (u16).
 - data_type: 1=UnsignedInteger, 2=FloatingPoint; bit_size is the Rust register width.
*/
int32_t pr_registers_count(uint64_t session, uint32_t core_index, uint32_t* out_count);
int32_t pr_register_info(uint64_t session, uint32_t core_index, uint32_t reg_index,
                         uint16_t* reg_id, uint32_t* bit_size, int32_t* data_type,
                         char* name, size_t name_len, size_t* out_name_len);
int32_t pr_read_reg_u64(uint64_t session, uint32_t core_index, uint16_t reg_id, uint64_t* out_value);
int32_t pr_write_reg_u64(uint64_t session, uint32_t core_index, uint16_t reg_id, uint64_t value);

/*
 Breakpoint operations
*/
int32_t pr_available_breakpoint_units(uint64_t session, uint32_t core_index, uint32_t* out_units);
int32_t pr_set_hw_breakpoint(uint64_t session, uint32_t core_index, uint64_t address);
int32_t pr_clear_hw_breakpoint(uint64_t session, uint32_t core_index, uint64_t address);
int32_t pr_clear_all_hw_breakpoints(uint64_t session);

/* These fields correspond to probe_rs::flashing::DownloadOptions. NULL uses Rust Default. */
typedef struct {
    int32_t keep_unwritten_bytes, dry_run, do_chip_erase, skip_erase;
    int32_t preverify, verify, disable_double_buffering;
    const char* const* preferred_algos;
    size_t preferred_algos_len;
    int32_t has_ram_chunk_size;
    uint64_t ram_chunk_size;
} pr_download_options_t;

/* format: 1=ELF, 2=Intel HEX, 3=BIN. BIN address 0 is valid when has_base_address=1. */
typedef struct {
    int32_t format, has_base_address;
    uint64_t base_address;
    uint32_t skip;
    const char* const* skip_sections;
    size_t skip_sections_len;
} pr_image_options_t;

typedef struct { uint64_t address, size; } pr_flash_span_t;
typedef struct { uint64_t address; uint32_t size; const uint8_t* data; } pr_flash_page_t;
typedef struct { uint64_t address, size; size_t page_index; } pr_flash_fill_t;
typedef struct {
    const pr_flash_span_t* sectors; size_t sector_count;
    const pr_flash_page_t* pages; size_t page_count;
    const pr_flash_fill_t* fills; size_t fill_count;
    const pr_flash_span_t* data_blocks; size_t data_block_count;
} pr_flash_layout_t;
/* kind: 1 LayoutReady, 2 AddProgressBar, 3 Started, 4 Progress,
 * 5 Finished, 6 Failed, 7 DiagnosticMessage.
 * operation: 0 Fill, 1 Erase, 2 Program, 3 Verify, 4 Ram, -1 if absent.
 * For Progress, size is the byte increment and duration_ns is its elapsed time.
 * Pointers and their nested arrays remain valid only during the callback. */
typedef struct {
    int32_t kind, operation, has_total;
    uint64_t total, size, duration_ns;
    const uint8_t* message; size_t message_len; /* UTF-8 bytes; may contain NUL */
    const pr_flash_layout_t* layouts;
    size_t layout_count;
} pr_progress_event_t;
typedef void (*pr_progress_event_cb)(const pr_progress_event_t* event, void* context);

/* Both operations use the supplied Session. They never scan, attach, retry, or close it. */
int32_t pr_session_erase_all(uint64_t session, pr_progress_event_cb callback, void* context);
int32_t pr_session_flash(uint64_t session, const char* path, const pr_image_options_t* image,
                         const pr_download_options_t* options, pr_progress_event_cb callback, void* context);

/* Programmer type API */
/* Programmer type enumeration */
typedef enum {
    PR_PROG_UNKNOWN = 0,
    PR_PROG_CMSIS_DAP = 1,
    PR_PROG_STLINK = 2,
    PR_PROG_JLINK = 3,
    PR_PROG_FTDI = 4,
    PR_PROG_ESP_USB_JTAG = 5,
    PR_PROG_WCH_LINK = 6,
    PR_PROG_SIFLI_UART = 7,
    PR_PROG_GLASGOW = 8,
    PR_PROG_CH347_USB_JTAG = 9,
} pr_programmer_type_t;

/* Programmer type conversion is stateless; the filter is supplied on session open. */
int32_t pr_programmer_type_is_supported_code(int32_t type_code);
size_t  pr_programmer_type_to_string(int32_t type_code, char* buf, size_t buf_len);
int32_t pr_programmer_type_from_string(const char* type_name, int32_t* out_code);

/* Chip database and detection */
/*
   Manufacturer & Chip Listing
   - All names are exposed as UTF-8 C strings.
   - Use integer indexes; do not rely on string matching in C code.
   Functions:
     - pr_chip_manufacturer_count(): Return the number of manufacturers.
     - pr_chip_manufacturer_name(index, buf, buf_len): Get manufacturer name by index.
       If buf==NULL or buf_len==0, returns required size (including NUL).
     - pr_chip_model_count(manu_index, out_count): Return status and number of chip models.
     - pr_chip_model_name(manu_index, chip_index, buf, buf_len): Get chip model name.
       Same size semantics as above.
     - pr_chip_model_specs(manu_index, chip_index, buf, buf_len): Return a JSON string
       of spec details (architecture, cores, memory regions, algorithms).
     - pr_chip_specs_by_name(name, buf, buf_len): Return a JSON spec string for a given name.
   Error handling: On invalid index or name, functions return 0 and set pr_last_error().
*/
uint32_t pr_chip_manufacturer_count(void);
size_t   pr_chip_manufacturer_name(uint32_t index, char* buf, size_t buf_len);
int32_t pr_chip_model_count(uint32_t manu_index, uint32_t* out_count);
size_t   pr_chip_model_name(uint32_t manu_index, uint32_t chip_index, char* buf, size_t buf_len);
size_t pr_chip_model_specs(uint32_t manu_index, uint32_t chip_index, char *buf, size_t buf_len);
size_t pr_chip_specs_by_name(const char *name, char *buf, size_t buf_len);


#ifdef __cplusplus
}
#endif
