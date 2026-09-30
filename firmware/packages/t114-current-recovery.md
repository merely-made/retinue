# T114 current firmware recovery

The source-pinned `711816a` package writes only `0x26000..0x72100`.
The S140 SoftDevice, control pair `0xE6000..0xE8000`, announce/settings
pairs `0xE8000..0xEC000`, and stock bootloader remain outside that range.
Its immutable payload emits the native-node persistent-state guard.

Double-tap reset, select the mounted `HT-n5262` volume explicitly, and use:

```text
linkboy capture-t114-loader VOLUME LOADER.json
linkboy flash-volume VOLUME firmware/packages/t114-current.toml t114@2.x --receipt RECEIPT.json
```

Linkboy validates the complete UF2 and loader facts before writing. After the
volume ejects and the application returns, status must identify
`build=711816a8124bd58cd8cc314ec4de5009be7df166 image=t114-native`.
Check the persisted identity and selected region/channel separately.

The [fresh Windows receipt](../../testing/receipts/firmware-refresh/follow-up/README.md)
records installation, original settings sequence, guarded admission, and native
RF checks. A retry of these same bytes after a failed application has not been
exercised; the current catalog entry therefore remains `partial`. Historical
v51 recovery receipts qualify that older artifact only. The loader's CURRENT.UF2
backup omitted the settings pair, so this run does not claim byte-identical
settings readback or a complete device backup.
