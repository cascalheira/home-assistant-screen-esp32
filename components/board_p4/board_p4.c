// SD card on the ESP32-P4 board (Guition JC1060P470C), as in Guition's board
// support package. Written in C because ESP-IDF describes the SDMMC host with
// a large initializer macro (SDMMC_HOST_DEFAULT).
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
