// Rust bindings for the ESP32-P4 board's panel driver (empty on other chips).
#include "sdkconfig.h"
#if CONFIG_IDF_TARGET_ESP32P4
#include "esp_lcd_jd9165.h"
#include "esp_lcd_mipi_dsi.h"
#include "esp_ldo_regulator.h"
#include "board_p4.h"
#endif
