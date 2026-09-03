//! NMEA parsing for a board-attached GNSS module: PD3 of the position disclosure plan.
//!
//! This is the host-testable half. A board task owns the UART and feeds bytes in; this
//! module turns them into a [`GnssState`] and never touches hardware, so every sentence
//! shape the L76K emits can be tested under `cargo test` rather than only on metal.
//!
//! Two sentences carry everything the status path needs. `RMC` says whether there is a
//! valid fix (`A`) or not (`V`) and carries latitude and longitude. `GGA` carries the
//! satellite count and HDOP. A fix is reported only from an `RMC` marked `A`; an `RMC`
//! marked `V` reports [`GnssState::NoFix`], which is how a lost fix falls back to absence
//! rather than leaving a stale value in place. `GGA` alone never produces a fix.
//!
//! Every sentence is checksum-verified before any field is read. A sentence that fails
//! its checksum, overflows the line buffer, or is not one of the two we read is dropped
//! and counted, and the current state does not change.

use radio_face::{GnssFix, GnssState};

/// Longest sentence accepted. NMEA caps a sentence at 82 characters including `$` and
/// `\r\n`; the margin absorbs vendor extensions without inviting an unbounded buffer.
pub const MAX_SENTENCE: usize = 96;

/// Byte-at-a-time NMEA sentence assembler and parser.
#[derive(Debug)]
pub struct NmeaParser {
    line: [u8; MAX_SENTENCE],
    len: usize,
    overflow: bool,
    state: GnssState,
    /// Satellites and HDOP from the most recent `GGA`, folded into the next fix.
    satellites: u8,
    hdop_tenths: u16,
    dropped: u32,
    accepted: u32,
}
impl Default for NmeaParser {
    fn default() -> Self {
        Self::new()
    }
}
impl NmeaParser {
    pub const fn new() -> Self {
        Self {
            line: [0; MAX_SENTENCE],
            len: 0,
            overflow: false,
            state: GnssState::Absent,
            satellites: 0,
            hdop_tenths: 0,
            dropped: 0,
            accepted: 0,
        }
    }

    /// Current state. `Absent` until the first sentence of either kind is accepted.
    pub const fn state(&self) -> GnssState {
        self.state
    }
    /// Sentences dropped for checksum, overflow, or unknown type.
    pub const fn dropped(&self) -> u32 {
        self.dropped
    }
    /// `RMC` and `GGA` sentences accepted.
    pub const fn accepted(&self) -> u32 {
        self.accepted
    }

    /// Feeds one byte. Returns `Some(state)` when a sentence completes and changes or
    /// confirms the state, `None` otherwise. `uptime_secs` stamps any fix produced.
    pub fn push(&mut self, byte: u8, uptime_secs: u32) -> Option<GnssState> {
        match byte {
            b'$' => {
                self.len = 0;
                self.overflow = false;
                self.line[0] = b'$';
                self.len = 1;
                None
            }
            b'\n' => {
                if self.overflow || self.len == 0 {
                    self.len = 0;
                    self.overflow = false;
                    self.dropped = self.dropped.saturating_add(1);
                    return None;
                }
                let len = self.len;
                self.len = 0;
                let (line, rest) = self.line.split_at_mut(len);
                let _ = rest;
                let result = parse_sentence(&*line, uptime_secs, self.satellites, self.hdop_tenths);
                match result {
                    Parsed::Rmc(state) => {
                        self.accepted = self.accepted.saturating_add(1);
                        self.state = state;
                        Some(state)
                    }
                    Parsed::Gga {
                        satellites,
                        hdop_tenths,
                    } => {
                        self.accepted = self.accepted.saturating_add(1);
                        self.satellites = satellites;
                        self.hdop_tenths = hdop_tenths;
                        // A GGA refines a held fix's sat count and HDOP without
                        // re-asserting validity, which only RMC may do.
                        if let GnssState::Fix(mut fix) = self.state {
                            fix.satellites = satellites;
                            fix.hdop_tenths = hdop_tenths;
                            self.state = GnssState::Fix(fix);
                        } else if self.state == GnssState::Absent {
                            self.state = GnssState::NoFix;
                        }
                        Some(self.state)
                    }
                    Parsed::Dropped => {
                        self.dropped = self.dropped.saturating_add(1);
                        None
                    }
                }
            }
            b'\r' => None,
            other => {
                if self.len == 0 {
                    // Bytes before a `$` are line noise; ignore until a sentence starts.
                    return None;
                }
                if self.len >= MAX_SENTENCE {
                    self.overflow = true;
                } else {
                    self.line[self.len] = other;
                    self.len += 1;
                }
                None
            }
        }
    }
}

enum Parsed {
    Rmc(GnssState),
    Gga { satellites: u8, hdop_tenths: u16 },
    Dropped,
}

/// Parses one complete sentence, `$` through the checksum, with no line ending.
fn parse_sentence(line: &[u8], uptime_secs: u32, satellites: u8, hdop_tenths: u16) -> Parsed {
    // `$TTTSS,fields*HH`
    let Some(star) = line.iter().rposition(|&b| b == b'*') else {
        return Parsed::Dropped;
    };
    if line.len() < 7 || star + 3 != line.len() || line[0] != b'$' {
        return Parsed::Dropped;
    }
    let body = &line[1..star];
    let mut sum = 0u8;
    for &b in body {
        sum ^= b;
    }
    let Some(expected) = hex_byte(line[star + 1], line[star + 2]) else {
        return Parsed::Dropped;
    };
    if sum != expected {
        return Parsed::Dropped;
    }
    // Talker is two chars (GP, GN, GL, GA, GB); the sentence type is the next three.
    if body.len() < 6 || body[5] != b',' {
        return Parsed::Dropped;
    }
    let kind = &body[2..5];
    let fields = &body[6..];
    match kind {
        b"RMC" => parse_rmc(fields, uptime_secs, satellites, hdop_tenths),
        b"GGA" => parse_gga(fields),
        _ => Parsed::Dropped,
    }
}

/// `RMC`: time,status,lat,N/S,lon,E/W,speed,course,date,...
fn parse_rmc(fields: &[u8], uptime_secs: u32, satellites: u8, hdop_tenths: u16) -> Parsed {
    let mut it = fields.split(|&b| b == b',');
    let _time = it.next();
    let status = it.next().unwrap_or(&[]);
    let lat = it.next().unwrap_or(&[]);
    let ns = it.next().unwrap_or(&[]);
    let lon = it.next().unwrap_or(&[]);
    let ew = it.next().unwrap_or(&[]);
    match status {
        b"A" => {
            let (Some(lat_e7), Some(lon_e7)) = (
                parse_coord(lat, ns, 2),
                parse_coord(lon, ew, 3),
            ) else {
                return Parsed::Dropped;
            };
            Parsed::Rmc(GnssState::Fix(GnssFix {
                lat_e7,
                lon_e7,
                satellites,
                hdop_tenths,
                at_uptime_secs: uptime_secs,
            }))
        }
        b"V" => Parsed::Rmc(GnssState::NoFix),
        _ => Parsed::Dropped,
    }
}

/// `GGA`: time,lat,N/S,lon,E/W,quality,numSV,HDOP,...
fn parse_gga(fields: &[u8]) -> Parsed {
    let mut it = fields.split(|&b| b == b',');
    for _ in 0..6 {
        it.next();
    }
    let num_sv = it.next().unwrap_or(&[]);
    let hdop = it.next().unwrap_or(&[]);
    let Some(satellites) = parse_u32(num_sv) else {
        return Parsed::Dropped;
    };
    let hdop_tenths = parse_tenths(hdop).unwrap_or(0);
    Parsed::Gga {
        satellites: satellites.min(u8::MAX as u32) as u8,
        hdop_tenths,
    }
}

/// `ddmm.mmmm` (or `dddmm.mmmm` for longitude) with a hemisphere letter, to degrees
/// times ten million. Four decimal minutes is the L76K's resolution; more are accepted
/// and truncated, fewer are scaled.
fn parse_coord(value: &[u8], hemi: &[u8], deg_digits: usize) -> Option<i32> {
    if value.len() < deg_digits + 2 {
        return None;
    }
    let (deg, min) = value.split_at(deg_digits);
    let degrees = parse_u32(deg)?;
    let dot = min.iter().position(|&b| b == b'.')?;
    let (min_int, min_frac) = (&min[..dot], &min[dot + 1..]);
    let minutes = parse_u32(min_int)?;
    // Minutes to 1e-7 degrees: min * 1e7 / 60. Work in u64 and take at most 7 fractional
    // digits of the minute, scaled so that `frac` is in units of 1e-7 minutes.
    let mut frac: u64 = 0;
    let mut scale: u64 = 10_000_000;
    for &b in min_frac.iter().take(7) {
        if !b.is_ascii_digit() {
            return None;
        }
        scale /= 10;
        frac += u64::from(b - b'0') * scale;
    }
    if minutes >= 60 {
        return None;
    }
    let minutes_e7 = u64::from(minutes) * 10_000_000 + frac;
    let e7 = u64::from(degrees) * 10_000_000 + minutes_e7 / 60;
    let signed = i32::try_from(e7).ok()?;
    match hemi {
        b"N" | b"E" => Some(signed),
        b"S" | b"W" => Some(-signed),
        _ => None,
    }
}

fn parse_u32(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() {
        return None;
    }
    let mut n: u32 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(u32::from(b - b'0'))?;
    }
    Some(n)
}

/// `d.d` to tenths. HDOP has one decimal in practice; a second is truncated.
fn parse_tenths(value: &[u8]) -> Option<u16> {
    if value.is_empty() {
        return None;
    }
    let dot = value.iter().position(|&b| b == b'.');
    let (int, frac) = match dot {
        Some(d) => (&value[..d], &value[d + 1..]),
        None => (value, &[][..]),
    };
    let whole = parse_u32(int)?;
    let tenth = match frac.first() {
        Some(b) if b.is_ascii_digit() => u32::from(b - b'0'),
        Some(_) => return None,
        None => 0,
    };
    u16::try_from(whole * 10 + tenth).ok()
}

fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    let nib = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    Some(nib(hi)? << 4 | nib(lo)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(parser: &mut NmeaParser, sentence: &[u8], uptime: u32) -> Option<GnssState> {
        let mut last = None;
        for &b in sentence {
            if let Some(s) = parser.push(b, uptime) {
                last = Some(s);
            }
        }
        last
    }

    // Real sentence shapes from a Quectel L76K at its 9600 default, with correct
    // checksums. Ashland KY, roughly.
    const RMC_FIX: &[u8] = b"$GNRMC,143012.000,A,3828.4521,N,08238.9123,W,0.05,0.00,020926,,,A*66\r\n";
    const RMC_VOID: &[u8] = b"$GNRMC,143012.000,V,,,,,,,020926,,,N*59\r\n";
    const GGA: &[u8] = b"$GNGGA,143012.000,3828.4521,N,08238.9123,W,1,09,1.2,182.4,M,-33.1,M,,*77\r\n";

    /// Wraps a sentence body in `$`, a correct checksum, and `\r\n`, without alloc.
    fn checksum(body: &str) -> heapless::Vec<u8, 128> {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut sum = 0u8;
        for b in body.bytes() {
            sum ^= b;
        }
        let mut out = heapless::Vec::new();
        out.push(b'$').unwrap();
        out.extend_from_slice(body.as_bytes()).unwrap();
        out.push(b'*').unwrap();
        out.push(HEX[usize::from(sum >> 4)]).unwrap();
        out.push(HEX[usize::from(sum & 0x0F)]).unwrap();
        out.extend_from_slice(b"\r\n").unwrap();
        out
    }

    #[test]
    fn rmc_active_produces_a_fix_with_e7_coordinates() {
        let mut p = NmeaParser::new();
        let s = checksum("GNRMC,143012.000,A,3828.4521,N,08238.9123,W,0.05,0.00,020926,,,A");
        let state = feed(&mut p, &s, 42).unwrap();
        let GnssState::Fix(fix) = state else {
            panic!("expected fix, got {state:?}");
        };
        // 38 + 28.4521/60 = 38.474201666.. -> 384742016 at e7
        assert_eq!(fix.lat_e7, 384_742_016);
        // 82 + 38.9123/60 = 82.648538333.. -> -826485383 (W)
        assert_eq!(fix.lon_e7, -826_485_383);
        assert_eq!(fix.at_uptime_secs, 42);
        assert_eq!(p.accepted(), 1);
        assert_eq!(p.dropped(), 0);
    }

    #[test]
    fn gga_refines_satellites_and_hdop_but_never_asserts_a_fix() {
        let mut p = NmeaParser::new();
        let gga = checksum("GNGGA,143012.000,3828.4521,N,08238.9123,W,1,09,1.2,182.4,M,-33.1,M,,");
        assert_eq!(feed(&mut p, &gga, 1), Some(GnssState::NoFix), "GGA alone is not a fix");
        let rmc = checksum("GNRMC,143012.000,A,3828.4521,N,08238.9123,W,0.05,0.00,020926,,,A");
        let GnssState::Fix(fix) = feed(&mut p, &rmc, 2).unwrap() else {
            panic!()
        };
        assert_eq!(fix.satellites, 9);
        assert_eq!(fix.hdop_tenths, 12);
        let gga2 = checksum("GNGGA,143013.000,3828.4521,N,08238.9123,W,1,11,0.9,182.4,M,-33.1,M,,");
        let GnssState::Fix(fix2) = feed(&mut p, &gga2, 3).unwrap() else {
            panic!()
        };
        assert_eq!(fix2.satellites, 11);
        assert_eq!(fix2.hdop_tenths, 9);
        assert_eq!(fix2.at_uptime_secs, 2, "GGA does not restamp the fix time");
    }

    #[test]
    fn rmc_void_falls_back_to_nofix_and_does_not_keep_the_old_position() {
        let mut p = NmeaParser::new();
        let rmc = checksum("GNRMC,143012.000,A,3828.4521,N,08238.9123,W,0.05,0.00,020926,,,A");
        assert!(matches!(feed(&mut p, &rmc, 1), Some(GnssState::Fix(_))));
        let void = checksum("GNRMC,143013.000,V,,,,,,,020926,,,N");
        assert_eq!(feed(&mut p, &void, 2), Some(GnssState::NoFix));
        assert_eq!(p.state(), GnssState::NoFix);
    }

    #[test]
    fn bad_checksum_overflow_and_unknown_sentences_are_dropped_without_changing_state() {
        let mut p = NmeaParser::new();
        assert_eq!(p.state(), GnssState::Absent);
        assert_eq!(feed(&mut p, b"$GNRMC,143012.000,A,3828.4521,N,08238.9123,W,0.05,0.00,020926,,,A*00\r\n", 1), None);
        assert_eq!(p.dropped(), 1);
        assert_eq!(feed(&mut p, &checksum("GNVTG,0.00,T,,M,0.05,N,0.09,K,A"), 1), None);
        assert_eq!(p.dropped(), 2);
        let mut long: heapless::Vec<u8, 256> = heapless::Vec::new();
        long.push(b'$').unwrap();
        for _ in 0..30 {
            long.extend_from_slice(b"GNRMC,").unwrap();
        }
        long.extend_from_slice(b"*00\r\n").unwrap();
        assert_eq!(feed(&mut p, &long, 1), None);
        assert_eq!(p.dropped(), 3);
        assert_eq!(p.state(), GnssState::Absent, "nothing accepted, state untouched");
        assert_eq!(p.accepted(), 0);
    }

    #[test]
    fn line_noise_before_dollar_and_a_torn_first_sentence_are_ignored() {
        let mut p = NmeaParser::new();
        // Typical power-on: garbage, then the tail of a sentence, then a clean one.
        let torn: &[u8] = b"\x00\xFF12.3,N,08238.9123,W,0.05,0.00,020926,,,A*7A\r\n";
        assert_eq!(feed(&mut p, torn, 1), None);
        let rmc = checksum("GPRMC,143012.000,A,3828.4521,N,08238.9123,W,0.05,0.00,020926,,,A");
        assert!(matches!(feed(&mut p, &rmc, 2), Some(GnssState::Fix(_))), "GP talker accepted too");
    }

    #[test]
    fn southern_and_eastern_hemispheres_sign_correctly() {
        let mut p = NmeaParser::new();
        let rmc = checksum("GNRMC,000000.000,A,3352.1234,S,15112.5678,E,0.00,0.00,010126,,,A");
        let GnssState::Fix(fix) = feed(&mut p, &rmc, 1).unwrap() else {
            panic!()
        };
        assert!(fix.lat_e7 < 0 && fix.lon_e7 > 0);
        assert_eq!(fix.lat_e7, -338_687_233);
        assert_eq!(fix.lon_e7, 1_512_094_633);
    }

    #[test]
    fn real_constants_carry_valid_checksums() {
        // Guards the test fixtures themselves: if a constant is edited, this fails
        // before any assertion about parsing does.
        let mut p = NmeaParser::new();
        assert!(feed(&mut p, RMC_FIX, 1).is_some());
        assert_eq!(feed(&mut p, RMC_VOID, 2), Some(GnssState::NoFix));
        assert!(feed(&mut p, GGA, 3).is_some());
        assert_eq!(p.dropped(), 0);
    }
}
