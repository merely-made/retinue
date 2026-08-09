# Retinue Heltec V4 recovery

This package uses the ESP32-S3 ROM loader. Keep the USB cable attached and close serial
monitors before starting.

If the application does not return after a transfer:

1. Close every other serial tool.
2. Put the Heltec WiFi LoRa 32 V4 into its ESP32-S3 ROM loader using the board's reset and
   boot controls.
3. Confirm that the loader identifies an ESP32-S3 with 16 MiB flash.
4. Retry the same verified `heltec-v4-current.toml` package with Linkboy.

Do not identify the board from a COM number. Do not erase the whole device. The package writes
the declared application range and preserves the final `0x10000` bytes.
