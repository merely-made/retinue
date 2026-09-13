# Stock NomadNet form request fixtures

Captured 2026-09-13 from unmodified NomadNet 1.4.2 / RNS 1.5.3 on WSL Ubuntu,
using its terminal UI against a controlled Retinue loopback endpoint. These are
the complete decrypted request envelopes returned by `receive_raw_request`,
before application decoding, not reconstructed MessagePack. Only generated dummy
fields and task-local identities were used. No GPL/AGPL implementation source was read.

The `.json` files give independently observed expected maps: the same three UI
actions were first performed against a public Python RNS request handler. Retinue
then sent those values back to that handler; all three matched. Python
`msgpack==1.1.2` independently decoded these final raw stock-client envelopes and
checked the path hash and fields against the first handler's observations.
The timestamp is the stock client's original float64; map ordering is not semantic.

| Fixture | UI action | Bytes | SHA-256 |
| --- | --- | --- | --- |
| defaults | Submit all with declared defaults, including empty and masked text | 187 | `ad0e058a43d83d2e708b5bca06c7290e0c6ff2c29417fa55cb56a08fa6b273b4` |
| edited | Edit text to `edited café 雪` and multiline to `first line\nsecond 雪`; submit all | 215 | `1ddafdc761e60cb6cd4a1bec7d5d979cc0be555a29e1b680d0b649299246a456` |
| selected | Uncheck red, select blue radio; submit named fields and fixed variable | 133 | `4417515cd98cc8d52a2d620df483954a54caee52902c2d9d3ba00b6724d3ee30` |

The destination was `b13513fcc2f27bd12b8bcf1e8502d7d7`, with path
`/page/capture.mu`. The raw payload's third element is a string-to-string map.
Keys already contain `field_` and `var_`; a static page navigation instead sent
`nil`. The selected submission excludes empty, masked and multiline fields.

Reproduction artifacts: `C:\t\micron-forms-20260913`, including `index.mu`,
`server.py` (public Python API only), `probe` (owned Retinue API), `verify.py`,
both handlers' JSONL output, raw packet hex, and edited/selected terminal screens.
The server/client profiles disable instance sharing and transport and use only
TCP loopback ports 42541/42542. All three task-owned tmux sessions were stopped.

The actual API receipt is the linked section in
[NomadNet interoperability receipts](../../../../../design_docs/2026-09-13_nomadnet_go_resource_compression_receipt.md#typed-form-request-receipt).
Knot/Turnstone editable widgets and dynamic-site handlers are not qualified here.
