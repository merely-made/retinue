//! LE3 scan-physics receipt with one T114 listener and two V4 transmitters.
//!
//! Usage:
//! `cargo run -p tulle --features serial-async --example le3_scan_probe -- COM10 COM6 COM7`

use std::io;
use std::time::Duration;

use serial2_tokio::SerialPort;
use tokio::io::AsyncWriteExt;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

const BAUD: u32 = 115_200;
const FREQUENCY_HZ: u32 = 906_875_000;
const RECEIPT_FRAME_LEN: usize = 180;

struct ProbePort {
    port: SerialPort,
    pending: Vec<u8>,
}

impl Drop for ProbePort {
    fn drop(&mut self) {
        // Leave the firmware's outer session loop an explicit detach edge even
        // when a receipt assertion returns early.
        let _ = self.port.set_dtr(false);
        // The firmware samples DTR every 50 ms. Keep the handle and low edge
        // alive across several samples before Windows closes the port.
        std::thread::sleep(Duration::from_millis(250));
    }
}

impl ProbePort {
    async fn open(path: &str) -> io::Result<Self> {
        let port = SerialPort::open(path, BAUD)?;
        port.set_rts(false)?;
        port.set_dtr(true)?;
        tokio::time::sleep(Duration::from_millis(900)).await;
        let mut pending = Vec::new();
        let mut buffer = [0_u8; 256];
        while let Ok(Ok(read)) =
            tokio::time::timeout(Duration::from_millis(100), port.read(&mut buffer)).await
        {
            if read == 0 {
                break;
            }
            pending.extend_from_slice(&buffer[..read]);
        }
        // Opening the CDC port can reset the board and emit its application
        // banner. It proves identity to the operator, but it is not the answer
        // to the first probe command.
        pending.clear();
        Ok(Self { port, pending })
    }

    async fn command(&mut self, command: &str) -> io::Result<()> {
        // The firmware deliberately treats one USB packet as one probe. Keep
        // the command and newline in the same host write so the CDC driver
        // cannot expose a partial command at the channel boundary.
        let mut line = command.as_bytes().to_vec();
        line.push(b'\n');
        self.port.write_all(&line).await?;
        self.port.flush().await
    }

    async fn line(&mut self, patience: Duration) -> io::Result<String> {
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                return Ok(String::from_utf8_lossy(&line).trim().to_string());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "T114 did not finish its probe line",
                ));
            }
            let mut buffer = [0_u8; 256];
            let read = tokio::time::timeout(remaining, self.port.read(&mut buffer))
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "T114 probe timed out"))??;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "T114 probe port closed",
                ));
            }
            self.pending.extend_from_slice(&buffer[..read]);
        }
    }

    async fn answer(&mut self, command: &str, patience: Duration) -> io::Result<String> {
        self.command(command).await?;
        self.line(patience).await
    }

    async fn close(self) -> io::Result<()> {
        self.port.set_dtr(false)?;
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok(())
    }
}

fn profile(sync_word: u8, spreading_factor: u8) -> PhyProfile {
    let mut profile = PhyProfile::meshtastic_long_fast(FREQUENCY_HZ);
    profile.sync_word = sync_word;
    profile.spreading_factor = spreading_factor;
    profile.tx_power_dbm = 7;
    profile
}

async fn sender(
    path: &str,
    profile: PhyProfile,
) -> Result<DirectPhySerialLink, Box<dyn std::error::Error>> {
    let mut link = DirectPhySerialLink::open(
        path,
        profile,
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            // ESP32-S3 native USB does not gate on DTR. Leave it low so an
            // aborted receipt cannot strand a board on a control-line edge.
            dtr: false,
            online_timeout: Duration::from_secs(12),
            transmit_timeout: Duration::from_secs(12),
            ..DirectPhySerialConfig::default()
        },
    )?;
    tokio::time::timeout(Duration::from_secs(20), link.wait_online()).await??;
    Ok(link)
}

fn value(line: &str, name: &str) -> Option<u64> {
    line.split_ascii_whitespace().find_map(|field| {
        let (key, value) = field.split_once('=')?;
        (key == name).then(|| value.trim_end_matches("us").parse().ok())?
    })
}

async fn cad(
    listener: &mut ProbePort,
    transmitter: &DirectPhySerialLink,
    detection: u8,
) -> Result<String, Box<dyn std::error::Error>> {
    listener.command(&format!("le3 cad {detection}")).await?;
    let ready = listener.line(Duration::from_secs(3)).await?;
    if !ready.starts_with("le3 cad ready") {
        return Err(format!("unexpected CAD marker: {ready}").into());
    }
    transmitter.send(vec![0xCA; RECEIPT_FRAME_LEN]).await?;
    let result = listener.line(Duration::from_secs(8)).await?;
    println!("  {result}");
    Ok(result)
}

async fn capture(
    listener: &mut ProbePort,
    transmitter: &DirectPhySerialLink,
    receive: u8,
    fill: u8,
) -> Result<String, Box<dyn std::error::Error>> {
    listener.command(&format!("le3 rx {receive}")).await?;
    let ready = listener.line(Duration::from_secs(3)).await?;
    if !ready.starts_with("le3 rx ready") {
        return Err(format!("unexpected capture marker: {ready}").into());
    }
    transmitter.send(vec![fill; 64]).await?;
    let result = listener.line(Duration::from_secs(5)).await?;
    println!("  {result}");
    Ok(result)
}

fn require_positive(line: &str, field: &str) -> Result<(), Box<dyn std::error::Error>> {
    match value(line, field) {
        Some(value) if value > 0 => Ok(()),
        _ => Err(format!("expected positive {field} in: {line}").into()),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let listener_port = args.next().unwrap_or_else(|| "COM10".into());
    let meshtastic_port = args.next().unwrap_or_else(|| "COM6".into());
    let meshcore_port = args.next().unwrap_or_else(|| "COM7".into());

    println!(
        "LE3 bench: listener={listener_port}, 0x2b transmitter={meshtastic_port}, 0x12 transmitter={meshcore_port}"
    );
    println!("preflighting both V4 direct-PHY transmitters:");
    let long_fast = sender(&meshtastic_port, profile(0x2b, 11)).await?;
    let meshcore = sender(&meshcore_port, profile(0x12, 11)).await?;
    println!("  transmitters online");

    let mut listener = ProbePort::open(&listener_port).await?;
    let plan = listener.answer("le3 plan", Duration::from_secs(3)).await?;
    println!("{plan}");
    if !plan.contains("detections=2")
        || !plan.contains("receives=3")
        || !plan.contains("fits=1")
        || !plan.contains("overfull_rejected=1")
    {
        return Err("the board did not admit the receipt-shaped scan plan".into());
    }

    println!("exact 0x12 capture from the second transmitter:");
    let meshcore_exact = capture(&mut listener, &meshcore, 1, 0x12).await?;
    if !meshcore_exact.contains("result=capture") || !meshcore_exact.contains("len=64") {
        return Err(format!("0x12 window missed its matching frame: {meshcore_exact}").into());
    }
    meshcore.shutdown().await?;

    println!("CAD group 1 under matching SF11 and nonmatching SF9 traffic:");
    let d1_match = cad(&mut listener, &long_fast, 1).await?;
    let d2_miss = cad(&mut listener, &long_fast, 2).await?;
    require_positive(&d1_match, "hits")?;
    require_positive(&d2_miss, "misses")?;

    println!("fixed 0x12 window against a 0x2b frame, then exact 0x2b capture:");
    let mismatch = capture(&mut listener, &long_fast, 1, 0x2B).await?;
    if !mismatch.contains("result=miss") {
        return Err(format!("0x12 window did not miss the 0x2b frame: {mismatch}").into());
    }
    let exact = capture(&mut listener, &long_fast, 2, 0x2B).await?;
    if !exact.contains("result=capture") || !exact.contains("len=64") {
        return Err(format!("0x2b window did not capture the 0x2b frame: {exact}").into());
    }
    println!("CAD group 2 under matching SF9 and nonmatching SF11 traffic:");
    long_fast.reconfigure(profile(0x2b, 9)).await?;
    let d2_match = cad(&mut listener, &long_fast, 2).await?;
    let d1_miss = cad(&mut listener, &long_fast, 1).await?;
    require_positive(&d2_match, "hits")?;
    require_positive(&d1_miss, "misses")?;
    let fast_exact = capture(&mut listener, &long_fast, 3, 0xF9).await?;
    if !fast_exact.contains("result=capture") {
        return Err(format!("fast capture profile missed its matching frame: {fast_exact}").into());
    }
    long_fast.shutdown().await?;

    let air = listener.answer("air", Duration::from_secs(3)).await?;
    let scan = listener.line(Duration::from_secs(3)).await?;
    println!("{air}");
    println!("{scan}");
    listener.close().await?;
    println!("LE3 SCAN PHYSICS PASSED");
    Ok(())
}
