# T114 v51 recovery

The package writes only `0x26000..0x693c2`. The S140 bootloader and persisted
settings ranges remain intact. If the application does not return, double-tap
the T114 reset, wait for the `HT-n5262` DFU port, then apply the same v51 ZIP.
