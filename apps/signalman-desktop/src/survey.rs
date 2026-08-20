//! The desktop application's optional physical-port boundary.
//!
//! A normal owner survey asks Linkboy for every unowned serial port. An
//! explicitly attached station owns `SIGNALMAN_STATION_PORT`, so the installer
//! never probes that port during startup or a Rescan. For a focused maintenance
//! run, `SIGNALMAN_SERIAL_PORTS` can further name a comma-separated allowlist.

use signalman::DeviceCandidate;

pub fn devices() -> Vec<DeviceCandidate> {
    let station_port = std::env::var("SIGNALMAN_STATION_PORT")
        .ok()
        .filter(|port| !port.trim().is_empty());
    let ports = match std::env::var("SIGNALMAN_SERIAL_PORTS") {
        Ok(value) => value
            .split(',')
            .map(str::trim)
            .filter(|port| !port.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(_) => linkboy::ports().unwrap_or_default(),
    };
    signalman::survey_ports(without_owned_station(ports, station_port.as_deref()))
}

fn without_owned_station(
    ports: impl IntoIterator<Item = String>,
    station_port: Option<&str>,
) -> Vec<String> {
    ports
        .into_iter()
        .filter(|port| {
            !station_port.is_some_and(|station| port.eq_ignore_ascii_case(station.trim()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::without_owned_station;

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

    #[test]
    fn attached_station_port_is_not_opened_by_the_installer_survey() {
        assert_eq!(
            without_owned_station(["COM7".to_string(), "COM10".to_string()], Some("com7")),
            ["COM10"]
        );
    }
}
