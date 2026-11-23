# The ESP32-P4 board's extra components (see idf_component.yml), a small C
# helper for its SD card and camera, and the camera's sensor driver. On other
# chips the component is empty.
if(IDF_TARGET STREQUAL "esp32p4")
    idf_component_register(SRCS "board_p4.c" "ov02c10/ov02c10.c"
                           INCLUDE_DIRS "." "ov02c10/include"
                           PRIV_INCLUDE_DIRS "ov02c10/private_include"
                           REQUIRES fatfs sdmmc esp_driver_sdmmc esp_driver_i2c vfs
                           PRIV_REQUIRES espressif__esp_video espressif__esp_cam_sensor
                                         espressif__esp_sccb_intf esp_driver_gpio)
    # The driver registers itself in a linker section: nothing calls it, so
    # make the linker take it.
    target_link_libraries(${COMPONENT_LIB} INTERFACE "-u ov02c10_detect")
else()
    idf_component_register()
endif()
