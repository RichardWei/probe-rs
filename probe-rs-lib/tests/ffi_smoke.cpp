#include "probe_rs_lib.h"

#include <cstdint>
#include <limits>
#include <vector>

int main() {
    const size_t needed = pr_version(nullptr, 0);
    if (needed < 2) {
        return 1;
    }

    std::vector<char> version(needed);
    if (pr_version(version.data(), version.size()) != needed || version[0] == '\0') {
        return 2;
    }

    // Invalid handles check the new symbols without requiring hardware.
    if (pr_session_target_info(std::numeric_limits<uint64_t>::max(), nullptr,
                               nullptr, nullptr, 0) != 0) {
        return 3;
    }
    if (pr_session_erase_all(std::numeric_limits<uint64_t>::max(), nullptr, nullptr) == 0) return 4;
    if (pr_session_flash(std::numeric_limits<uint64_t>::max(), "firmware.hex", nullptr,
                         nullptr, nullptr, nullptr) == 0) return 5;
    return 0;
}
