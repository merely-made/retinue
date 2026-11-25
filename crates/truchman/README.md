# truchman

A SAMv3 client for I2P routers (Java I2P, i2pd, emissary), in the
[retinue](https://github.com/merely-made/retinue) family. It is a sans-io core with
a tokio shell.

- **Core**, always built: typed SAM commands that write themselves as
  validated lines, a line buffer that turns bridge bytes into replies and
  keeps whatever follows as stream data, destination and private-key parsing,
  `.b32.i2p` names, and I2P's base64/base32.
- **`tokio` feature**, on by default: a `Bridge` at any address (`DEST
  GENERATE`, `NAMING LOOKUP`), and `StreamSession` with `STREAM CONNECT` /
  `STREAM ACCEPT`, yielding byte streams that split into halves. A session
  answers the bridge's `PING`s and reports when the bridge closes it.

Every value written onto a SAM line is checked first, so a configured name,
key or option cannot inject a second command. Datagram sessions are not
implemented yet.

```rust,no_run
use truchman::{Bridge, Key, SessionOptions, StreamSession};

# async fn demo() -> truchman::Result<()> {
let bridge = Bridge::default(); // 127.0.0.1:7656
let session = StreamSession::create(bridge, Key::Transient, SessionOptions::default()).await?;
let peer = bridge.resolve("example.b32.i2p").await?;
let stream = session.connect(&peer).await?;
# drop(stream);
# Ok(())
# }
```

A truchman is an interpreter or go-between, the old English form of the word
that also gave *dragoman*: it speaks the router's protocol on the
application's behalf.

Licensed under the Mozilla Public License 2.0.
