# Retinue T114 v47 recovery

This package uses the T114's S140 serial DFU bootloader. A running Retinue T114 identifies
itself, so Linkboy can plan from that status, enter DFU, discover the new serial port, and
transfer the package.

If the application does not return after a transfer:

1. Close serial monitors and other tools using the board.
2. If the current application is foreign or silent, double-tap reset to mount its `HT-n5262`
   UF2 volume, then capture the board's own loader and SoftDevice record:

   ```text
   linkboy capture-t114-loader D:\\ t114-loader.json
   ```

3. Let the application return, identify its current serial port, then run the verified package
   with that explicit handoff and the owner-confirmed revision:

   ```text
   linkboy flash COM10 firmware/packages/t114-v47.toml t114@2.x --loader-snapshot t114-loader.json
   ```

   The snapshot is the physical `INFO_UF2.TXT` record. The current serial location and T114
   revision remain explicit owner input; Linkboy rediscovers the subsequent DFU port rather
   than treating a COM number as identity.

4. For a board that cannot return to any application serial port, retain the failed package
   receipt and use the explicit expert raw recovery route. Do not represent that route as an
   immutable-package or graphical installation receipt.

The package writes the application range beginning at `0x26000`. The S140 bootloader and the
declared preserved ranges are outside that write. Do not erase the whole device or choose a
different board family when recovering.
