//! The desktop application's optional physical-port boundary.
//!
//! A normal owner survey asks Linkboy for every serial port. For a focused
//! maintenance run, `SIGNALMAN_SERIAL_PORTS` can name a comma-separated
//! allowlist, so unrelated connected devices are not opened during either the
//! initial survey or a Rescan.

use signalman::DeviceCandidate;

pub fn devices() -> Vec<DeviceCandidate> {
    match std::env::var("SIGNALMAN_SERIAL_PORTS") {
        Ok(value) => {
            let ports = value
                .split(',')
                .map(str::trim)
                .filter(|port| !port.is_empty())
                .map(str::to_owned);
            signalman::survey_ports(ports)
        }
        Err(_) => signalman::survey_devices(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn comma_separated_ports_ignore_whitespace_and_empty_entries() {
        let ports: Vec<_> = " COM6, ,COM7 ,,"
            .split(',')
            .map(str::trim)
            .filter(|port| !port.is_empty())
            .map(str::to_owned)
            .collect();
        assert_eq!(ports, ["COM6", "COM7"]);
    }
}
