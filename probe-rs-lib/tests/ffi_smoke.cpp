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

    // Invalid index checks symbol linkage without requiring a physical probe.
    if (pr_probe_detect_target_info(std::numeric_limits<uint32_t>::max(), nullptr,
                                    nullptr, nullptr, 0) != 0) {
        return 3;
    }
    return 0;
}
