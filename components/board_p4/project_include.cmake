# The OV02C10's image tuning for Espressif's IPA (see ov02c10/README.md).
if(CONFIG_CAMERA_OV02C10 AND CONFIG_CAMERA_OV02C10_DEFAULT_IPA_JSON_CONFIGURATION_FILE)
    if(CONFIG_ESP32P4_SELECTS_REV_LESS_V3)
        idf_build_set_property(ESP_IPA_JSON_CONFIG_FILE_PATH "${COMPONENT_PATH}/ov02c10/cfg/ov02c10_default_p4_eco4.json" APPEND)
    else()
        idf_build_set_property(ESP_IPA_JSON_CONFIG_FILE_PATH "${COMPONENT_PATH}/ov02c10/cfg/ov02c10_default_p4_eco5.json" APPEND)
    endif()
endif()
