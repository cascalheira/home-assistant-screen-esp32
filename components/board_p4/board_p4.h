#pragma once
#include "esp_err.h"

/// Mounts the SD card (SDMMC slot 0, 4-bit, powered from on-chip LDO 4) as
/// FAT at `base_path`. The SDMMC controller is shared with the ESP32-C6's
/// SDIO link on slot 1: a failed mount releases slot 0 only.
esp_err_t board_p4_sd_mount(const char *base_path, int max_files);
