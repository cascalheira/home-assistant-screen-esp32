// SD card and camera on the ESP32-P4 board (Guition JC1060P470C). Written in
// C because ESP-IDF describes the SDMMC host with a large initializer macro
// (SDMMC_HOST_DEFAULT) and the camera is a V4L2 device (ioctl structures).
#include "board_p4.h"

#include "driver/sdmmc_host.h"
#include "esp_vfs_fat.h"
#include "sd_pwr_ctrl_by_on_chip_ldo.h"
#include "sdmmc_cmd.h"

esp_err_t board_p4_sd_mount(const char *base_path, int max_files)
{
    sdmmc_host_t host = SDMMC_HOST_DEFAULT();
    host.slot = SDMMC_HOST_SLOT_0;
    host.max_freq_khz = SDMMC_FREQ_HIGHSPEED;

    static sd_pwr_ctrl_handle_t pwr_ctrl = NULL;
    if (pwr_ctrl == NULL) {
        sd_pwr_ctrl_ldo_config_t ldo_config = {
            .ldo_chan_id = 4,
        };
        esp_err_t err = sd_pwr_ctrl_new_on_chip_ldo(&ldo_config, &pwr_ctrl);
        if (err != ESP_OK) {
            return err;
        }
    }
    host.pwr_ctrl_handle = pwr_ctrl;

    // D0-D3 = GPIO39-42, CMD = 44, CLK = 43: slot 0's dedicated IO-MUX pins.
    sdmmc_slot_config_t slot_config = SDMMC_SLOT_CONFIG_DEFAULT();
    slot_config.width = 4;
    slot_config.cd = SDMMC_SLOT_NO_CD;
    slot_config.wp = SDMMC_SLOT_NO_WP;

    esp_vfs_fat_sdmmc_mount_config_t mount_config = {
        .format_if_mount_failed = false,
        .max_files = max_files,
        .allocation_unit_size = 16 * 1024,
    };
    sdmmc_card_t *card = NULL;
    return esp_vfs_fat_sdmmc_mount(base_path, &host, &slot_config, &mount_config, &card);
}

// ---- Camera ------------------------------------------------------------------
// The OV02C10 through Espressif's esp_video (MIPI-CSI receiver, ISP with auto
// exposure), used as a V4L2 device. Frames are only reduced to a coarse
// brightness grid for motion detection: nothing is kept or sent.

#include <fcntl.h>
#include <inttypes.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include "esp_log.h"
#include "esp_video_device.h"
#include "esp_video_init.h"
#include "linux/videodev2.h"

#define CAMERA_BUFFERS 2

static const char *TAG = "camera";
static int s_fd = -1;
static uint8_t *s_buf[CAMERA_BUFFERS];
static uint32_t s_width, s_height;
static bool s_streaming;

esp_err_t board_p4_camera_init(i2c_master_bus_handle_t i2c_bus)
{
    esp_video_init_csi_config_t csi_config = {
        .sccb_config = {
            .init_sccb = false,
            .i2c_handle = i2c_bus,
            .freq = 100000,
        },
        .reset_pin = -1,
        .pwdn_pin = -1,
    };
    esp_video_init_config_t config = {
        .csi = &csi_config,
    };
    esp_err_t err = esp_video_init(&config);
    if (err != ESP_OK) {
        return err;
    }
    s_fd = open(ESP_VIDEO_MIPI_CSI_DEVICE_NAME, O_RDONLY);
    if (s_fd < 0) {
        ESP_LOGE(TAG, "no camera device");
        return ESP_ERR_NOT_FOUND;
    }
    struct v4l2_format format = {.type = V4L2_BUF_TYPE_VIDEO_CAPTURE};
    if (ioctl(s_fd, VIDIOC_G_FMT, &format) != 0) {
        return ESP_FAIL;
    }
    s_width = format.fmt.pix.width;
    s_height = format.fmt.pix.height;
    format.fmt.pix.pixelformat = V4L2_PIX_FMT_RGB565;
    if (ioctl(s_fd, VIDIOC_S_FMT, &format) != 0) {
        ESP_LOGE(TAG, "RGB565 refused");
        return ESP_FAIL;
    }
    struct v4l2_requestbuffers req = {
        .count = CAMERA_BUFFERS,
        .type = V4L2_BUF_TYPE_VIDEO_CAPTURE,
        .memory = V4L2_MEMORY_MMAP,
    };
    if (ioctl(s_fd, VIDIOC_REQBUFS, &req) != 0) {
        return ESP_ERR_NO_MEM;
    }
    for (int i = 0; i < CAMERA_BUFFERS; i++) {
        struct v4l2_buffer buf = {
            .type = V4L2_BUF_TYPE_VIDEO_CAPTURE,
            .memory = V4L2_MEMORY_MMAP,
            .index = i,
        };
        if (ioctl(s_fd, VIDIOC_QUERYBUF, &buf) != 0) {
            return ESP_FAIL;
        }
        s_buf[i] = mmap(NULL, buf.length, PROT_READ | PROT_WRITE, MAP_SHARED, s_fd, buf.m.offset);
        if (s_buf[i] == NULL) {
            return ESP_ERR_NO_MEM;
        }
    }
    ESP_LOGI(TAG, "camera ready, %" PRIu32 "x%" PRIu32, s_width, s_height);
    return ESP_OK;
}

esp_err_t board_p4_camera_stream(bool on)
{
    if (s_fd < 0) {
        return ESP_ERR_INVALID_STATE;
    }
    if (on == s_streaming) {
        return ESP_OK;
    }
    int type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (on) {
        for (int i = 0; i < CAMERA_BUFFERS; i++) {
            struct v4l2_buffer buf = {
                .type = V4L2_BUF_TYPE_VIDEO_CAPTURE,
                .memory = V4L2_MEMORY_MMAP,
                .index = i,
            };
            if (ioctl(s_fd, VIDIOC_QBUF, &buf) != 0) {
                return ESP_FAIL;
            }
        }
        if (ioctl(s_fd, VIDIOC_STREAMON, &type) != 0) {
            return ESP_FAIL;
        }
    } else if (ioctl(s_fd, VIDIOC_STREAMOFF, &type) != 0) {
        return ESP_FAIL;
    }
    s_streaming = on;
    return ESP_OK;
}

esp_err_t board_p4_camera_grid(uint8_t *grid, int cols, int rows)
{
    if (!s_streaming) {
        return ESP_ERR_INVALID_STATE;
    }
    struct v4l2_buffer buf = {
        .type = V4L2_BUF_TYPE_VIDEO_CAPTURE,
        .memory = V4L2_MEMORY_MMAP,
    };
    if (ioctl(s_fd, VIDIOC_DQBUF, &buf) != 0) {
        return ESP_FAIL;
    }
    const uint16_t *px = (const uint16_t *)s_buf[buf.index];
    const uint32_t cw = s_width / cols, ch = s_height / rows;
    // Every 4th pixel of every 4th line is plenty for a mean.
    for (int r = 0; r < rows; r++) {
        for (int c = 0; c < cols; c++) {
            uint32_t sum = 0, n = 0;
            for (uint32_t y = r * ch; y < (r + 1) * ch; y += 4) {
                const uint16_t *line = px + y * s_width;
                for (uint32_t x = c * cw; x < (c + 1) * cw; x += 4) {
                    uint16_t p = line[x];
                    // RGB565 to 0-255 luma: (2R + 5G + B) / 8 on 8-bit channels.
                    uint32_t r8 = (p >> 11) << 3, g8 = ((p >> 5) & 0x3F) << 2, b8 = (p & 0x1F) << 3;
                    sum += (2 * r8 + 5 * g8 + b8) >> 3;
                    n++;
                }
            }
            grid[r * cols + c] = n ? sum / n : 0;
        }
    }
    return ioctl(s_fd, VIDIOC_QBUF, &buf) == 0 ? ESP_OK : ESP_FAIL;
}
