# T114 v51 recovery

The package writes only `0x26000..0x69400`. The S140 SoftDevice, stock
bootloader, and persisted settings ranges remain intact. If the application
does not return, double-tap reset, select the mounted `HT-n5262` volume, and
apply the same verified v51 UF2. Linkboy's built-in writer needs no external
flashing helper. The native-node reservation at `0xE8000..0xEC000` is retained
across this package for later guard-aware recovery. This v51 payload predates the
guard and does not itself resume the native-node personality.
