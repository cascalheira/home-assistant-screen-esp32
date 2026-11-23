# The ESP32-P4 board's extra components (see idf_component.yml) and a small
# C helper for its SD card. On other chips the component is empty.
if(IDF_TARGET STREQUAL "esp32p4")
    idf_component_register(SRCS "board_p4.c"
                           INCLUDE_DIRS "."
                           REQUIRES fatfs sdmmc esp_driver_sdmmc)
else()
    idf_component_register()
endif()
