#pragma once
#include <stdbool.h>
#include <stdint.h>
#include "esp_err.h"
#include "driver/i2c_master.h"

/// Mounts the SD card (SDMMC slot 0, 4-bit, powered from on-chip LDO 4) as
/// FAT at `base_path`. The SDMMC controller is shared with the ESP32-C6's
/// SDIO link on slot 1: a failed mount releases slot 0 only.
esp_err_t board_p4_sd_mount(const char *base_path, int max_files);

/// Starts the camera (OV02C10 on MIPI-CSI) with its SCCB on `i2c_bus` and
/// opens it for RGB565 frames. Does not start streaming.
esp_err_t board_p4_camera_init(i2c_master_bus_handle_t i2c_bus);

/// Starts (true) or stops (false) the frame stream.
esp_err_t board_p4_camera_stream(bool on);

/// Waits for the next frame and reduces it to a `cols` x `rows` grid of
/// mean brightness (0-255, row-major) in `grid`.
esp_err_t board_p4_camera_grid(uint8_t *grid, int cols, int rows);
