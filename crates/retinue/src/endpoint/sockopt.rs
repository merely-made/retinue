//! Socket options for stream carriers: keepalive and user timeout (`TCPInterface.py` 83-95,
//! 181-207).

use std::io;
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;

/// RNS's probe profiles: idle seconds before the first probe, seconds between probes, probes
/// before the peer is declared dead, and the Linux user timeout in seconds.
const PLAIN: (u64, u64, u32, u64) = (5, 2, 12, 24);
const I2P: (u64, u64, u32, u64) = (10, 9, 5, 45);

/// Set nodelay and RNS's keepalive profile, so a half-open peer (power loss, NAT expiry) is
/// found and its interface ends. RNS on macOS sets only the idle time (`TCPInterface.py`
/// 196-207), about 10 minutes to detect; the interval and count apply here on every platform
/// that has them.
pub(super) fn tune(stream: &TcpStream, i2p_tunneled: bool) -> io::Result<()> {
    let (idle, interval, probes, _user_timeout) = if i2p_tunneled { I2P } else { PLAIN };
    stream.set_nodelay(true)?;
    let keepalive = TcpKeepalive::new().with_time(Duration::from_secs(idle));
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        target_os = "freebsd",
        windows
    ))]
    let keepalive = keepalive
        .with_interval(Duration::from_secs(interval))
        .with_retries(probes);
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        target_os = "freebsd",
        windows
    )))]
    let _ = (interval, probes);
    let socket = SockRef::from(stream);
    socket.set_tcp_keepalive(&keepalive)?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    socket.set_tcp_user_timeout(Some(Duration::from_secs(_user_timeout)))?;
    Ok(())
}
