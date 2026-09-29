//! Public companion device-info response, pinned to MeshCore 1.17.1's
//! `docs/companion_protocol.md` at d92964352441e53b93e8667b802e04f6e072b39e.
//! This names the firmware release separately from the serial API version.
//! The BLE PIN field is intentionally neither retained nor displayed.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceInfo<'a> {
    pub protocol_version: u8,
    pub build: &'a str,
    pub model: &'a str,
    pub version: &'a str,
}

impl<'a> DeviceInfo<'a> {
    /// Decode one complete DEVICE_INFO payload, excluding serial framing.
    pub fn decode(frame: &'a [u8]) -> Option<Self> {
        if frame.first() != Some(&13) || *frame.get(1)? < 3 {
            return None;
        }
        fn text(bytes: &[u8]) -> Option<&str> {
            let end = bytes
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(bytes.len());
            core::str::from_utf8(&bytes[..end]).ok().map(str::trim)
        }
        let info = Self {
            protocol_version: frame[1],
            build: text(frame.get(8..20)?)?,
            model: text(frame.get(20..60)?)?,
            version: text(frame.get(60..80)?)?,
        };
        (!info.version.is_empty()).then_some(info)
    }

    pub fn matches_release(&self, expected: &str) -> bool {
        self.version.strip_prefix('v').unwrap_or(self.version)
            == expected.strip_prefix('v').unwrap_or(expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> [u8; 82] {
        let mut frame = [0; 82];
        frame[0] = 13;
        frame[1] = 10; // API version, not firmware release 10.
        frame[4..8].copy_from_slice(&[1, 2, 3, 4]); // Not exposed by DeviceInfo.
        frame[8..16].copy_from_slice(b"d9296435");
        frame[20..29].copy_from_slice(b"Heltec V4");
        frame[60..67].copy_from_slice(b"v1.17.1");
        frame
    }

    #[test]
    fn release_is_distinct_from_api_version_and_pin_is_not_retained() {
        let wire = frame();
        let info = DeviceInfo::decode(&wire).unwrap();
        assert_eq!(info.protocol_version, 10);
        assert_eq!(info.version, "v1.17.1");
        assert_eq!(info.model, "Heltec V4");
        assert!(info.matches_release("1.17.1"));
        assert!(!info.matches_release("1.15.0"));
        let mut different_pin = wire;
        different_pin[4..8].fill(0xff);
        assert_eq!(DeviceInfo::decode(&different_pin), Some(info));
    }

    #[test]
    fn incomplete_wrong_and_empty_responses_do_not_qualify_a_release() {
        let mut wire = frame();
        for length in 0..80 {
            assert!(DeviceInfo::decode(&wire[..length]).is_none());
        }
        wire[0] = 0;
        assert!(DeviceInfo::decode(&wire).is_none());
        wire[0] = 13;
        wire[60..80].fill(0);
        assert!(DeviceInfo::decode(&wire).is_none());
        wire[60] = 0xff;
        assert!(DeviceInfo::decode(&wire).is_none());
    }
}
