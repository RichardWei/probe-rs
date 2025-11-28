#pragma once
/* C-compatible API for probe-rs dynamic library */
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 Error API
 - Retrieve the last error string. If buf==NULL or buf_len==0, returns the required size (including NUL).
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

/*
 Probe capabilities & target detection
 - Enumerate driver type and features for a given probe index
 - Check whether a given probe can attach to an unspecified target
*/

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

/* Feature flag bits */
#define PR_FEATURE_SWD          0x00000001u
#define PR_FEATURE_JTAG         0x00000002u
#define PR_FEATURE_ARM          0x00000004u
#define PR_FEATURE_RISCV        0x00000008u
#define PR_FEATURE_XTENSA       0x00000010u
#define PR_FEATURE_SWO          0x00000020u
#define PR_FEATURE_SPEED_CFG    0x00000040u

int32_t pr_probe_features(uint32_t index, uint32_t* out_driver_flags, uint32_t* out_feature_flags);
int32_t pr_probe_check_target(uint32_t index);

/*
 Session management
 - Open/close sessions. Returns a non-zero session handle on success.
 - protocol_code: 0=auto, 1=SWD, 2=JTAG; speed_khz=0 means not set.
*/
uint64_t pr_session_open_auto(const char* chip, uint32_t speed_khz, int32_t protocol_code);
uint64_t pr_session_open_with_probe(const char* selector, const char* chip, uint32_t speed_khz, int32_t protocol_code);
int32_t pr_session_close(uint64_t session);
uint32_t pr_core_count(uint64_t session);

/*
 Core control
 - Halt/Run/Step/Reset/Reset-and-halt; returns 0 on success.
*/
int32_t pr_core_halt(uint64_t session, uint32_t core_index, uint32_t timeout_ms);
int32_t pr_core_run(uint64_t session, uint32_t core_index);
int32_t pr_core_step(uint64_t session, uint32_t core_index);
int32_t pr_core_reset(uint64_t session, uint32_t core_index);
int32_t pr_core_reset_and_halt(uint64_t session, uint32_t core_index, uint32_t timeout_ms);

/*
 Core status
 - Returns: 0=Unknown, 1=Halted, 2=Running, <0 on error
*/
int32_t pr_core_status(uint64_t session, uint32_t core_index);

/*
 Memory operations
 - Read/Write 8-bit and 32-bit buffers.
*/
int32_t pr_read_8(uint64_t session, uint32_t core_index, uint64_t address, uint8_t* buf, uint32_t len);
int32_t pr_write_8(uint64_t session, uint32_t core_index, uint64_t address, const uint8_t* buf, uint32_t len);
int32_t pr_read_32(uint64_t session, uint32_t core_index, uint64_t address, uint32_t* buf, uint32_t len_words);
int32_t pr_write_32(uint64_t session, uint32_t core_index, uint64_t address, const uint32_t* buf, uint32_t len_words);

/*
 Register operations
 - Enumerate register file and read/write by RegisterId (u16).
*/
uint32_t pr_registers_count(uint64_t session, uint32_t core_index);
int32_t pr_register_info(uint64_t session, uint32_t core_index, uint32_t reg_index,
                         uint16_t* reg_id, uint32_t* bit_size,
                         char* name, size_t name_len);
int32_t pr_read_reg_u64(uint64_t session, uint32_t core_index, uint16_t reg_id, uint64_t* out_value);
int32_t pr_write_reg_u64(uint64_t session, uint32_t core_index, uint16_t reg_id, uint64_t value);

/*
 Breakpoint operations
*/
int32_t pr_available_breakpoint_units(uint64_t session, uint32_t core_index, uint32_t* out_units);
int32_t pr_set_hw_breakpoint(uint64_t session, uint32_t core_index, uint64_t address);
int32_t pr_clear_hw_breakpoint(uint64_t session, uint32_t core_index, uint64_t address);
int32_t pr_clear_all_hw_breakpoints(uint64_t session);

/* Flashing operations (firmware programming)
*/
/* Progress callback API */
/*
   Progress callback signature:
   - operation: 1=Erase, 2=Program, 3=Verify, 0=Fill/Unknown
   - percent: 0.0..100.0
   - status: short status string (e.g., "erasing"/"programming")
   - eta_ms: estimated remaining time in milliseconds, or -1 if unknown
*/
typedef void (*pr_progress_cb)(int32_t operation, float percent, const char* status, int32_t eta_ms);
void pr_set_progress_callback(pr_progress_cb cb);
void pr_clear_progress_callback(void);

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

/* Enum-based programmer type API */
int32_t pr_set_programmer_type_code(int32_t type_code);
int32_t pr_get_programmer_type_code(void);
int32_t pr_programmer_type_is_supported_code(int32_t type_code);
size_t  pr_programmer_type_to_string(int32_t type_code, char* buf, size_t buf_len);
int32_t pr_programmer_type_from_string(const char* type_name, int32_t* out_code);

/* String-based API removed: use enum-based APIs above, and conversion helpers */
/*
 * Parameters for pr_flash_elf:
 *  - chip: Target chip name string (must match targets database, e.g. "stm32f407zet6").
 *  - path: Absolute or relative file path to ELF/AXF firmware image.
 *  - verify: Set to 1 to verify after programming; 0 to skip verification.
 *  - preverify: Set to 1 to verify before programming (may skip unchanged ranges); 0 to disable.
 *  - chip_erase: Set to 1 to perform a mass/chip erase prior to programming; 0 to program only touched ranges.
 *  - speed_khz: Debug wire speed in kHz; set to 0 to keep default driver speed.
 *  - protocol_code: Debug protocol (0 = Auto, 1 = SWD, 2 = JTAG).
 *
 * Returns 0 on success; non‑zero error code on failure. Use pr_last_error() to retrieve details.
 */
int32_t pr_flash_elf(const char* chip, const char* path, int32_t verify, int32_t preverify, int32_t chip_erase, uint32_t speed_khz, int32_t protocol_code);
int32_t pr_flash_hex(const char* chip, const char* path, int32_t verify, int32_t preverify, int32_t chip_erase, uint32_t speed_khz, int32_t protocol_code);
int32_t pr_flash_bin(const char* chip, const char* path, uint64_t base_address, uint32_t skip, int32_t verify, int32_t preverify, int32_t chip_erase, uint32_t speed_khz, int32_t protocol_code);
/* Auto-detect format (by file extension): .elf/.axf => ELF, .hex/.ihex => HEX, .bin => BIN (requires base_address) */
int32_t pr_flash_auto(const char* chip, const char* path, uint64_t base_address, uint32_t skip, int32_t verify, int32_t preverify, int32_t chip_erase, uint32_t speed_khz, int32_t protocol_code);

/*
 * Perform a chip-wide erase.
 *
 * Parameters:
 *  - chip: Target chip name string (must match targets database, e.g. "stm32f407zet6").
 *  - speed_khz: Debug wire speed in kHz; set to 0 to keep default driver speed.
 *  - protocol_code: Debug protocol (0 = Auto, 1 = SWD, 2 = JTAG).
 *
 * Returns 0 on success; non-zero error code on failure. Use pr_last_error() to retrieve details.
 */
int32_t pr_chip_erase(const char* chip, uint32_t speed_khz, int32_t protocol_code);

/* Chip database and detection */
/*
   Manufacturer & Chip Listing
   - All names are exposed as UTF-8 C strings.
   - Use integer indexes; do not rely on string matching in C code.
   Functions:
     - pr_chip_manufacturer_count(): Return the number of manufacturers.
     - pr_chip_manufacturer_name(index, buf, buf_len): Get manufacturer name by index.
       If buf==NULL or buf_len==0, returns required size (including NUL).
     - pr_chip_model_count(manu_index): Return number of chip models for the manufacturer.
     - pr_chip_model_name(manu_index, chip_index, buf, buf_len): Get chip model name.
       Same size semantics as above.
     - pr_chip_model_specs(manu_index, chip_index, buf, buf_len): Return a JSON string
       of spec details (architecture, cores, memory regions, algorithms).
     - pr_chip_specs_by_name(name, buf, buf_len): Return a JSON spec string for a given name.
   Error handling: On invalid index or name, functions return 0 and set pr_last_error().
*/
uint32_t pr_chip_manufacturer_count(void);
size_t   pr_chip_manufacturer_name(uint32_t index, char* buf, size_t buf_len);
uint32_t pr_chip_model_count(uint32_t manu_index);
size_t   pr_chip_model_name(uint32_t manu_index, uint32_t chip_index, char* buf, size_t buf_len);
size_t pr_chip_model_specs(uint32_t manu_index, uint32_t chip_index, char *buf, size_t buf_len);
size_t pr_chip_specs_by_name(const char *name, char *buf, size_t buf_len);


#ifdef __cplusplus
}
#endif
