# OV02C10 camera sensor driver

The ESP32-P4 board's camera. Espressif's `esp_cam_sensor` component in the
registry has no driver for it, so this one comes from the copy of
`esp_cam_sensor` (2.1.0) in Guition's JC1060P470C demo package, unchanged.

Copyright Espressif Systems, Apache License 2.0 (see `LICENSE`).
`cfg/` holds the image tuning (auto exposure, white balance...) for
Espressif's IPA, for ESP32-P4 chips before revision 3 (`eco4`) and from it
(`eco5`).
