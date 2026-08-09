# Retinue T114 v47 recovery

This package uses the T114's S140 serial DFU bootloader. Keep the USB cable attached while
Linkboy enters DFU, waits for the new serial port, and transfers the package.

If the application does not return after a transfer:

1. Close serial monitors and other tools using the board.
2. Reset the T114, or open the original application port at 1200 baud and close it to request
   the bootloader.
3. Wait for the newly enumerated DFU port. The port number is not the board identity.
4. Retry the same verified `t114-v47.toml` package with Linkboy.

The package writes the application range beginning at `0x26000`. The S140 bootloader and the
declared preserved ranges are outside that write. Do not erase the whole device or choose a
different board family when recovering.
