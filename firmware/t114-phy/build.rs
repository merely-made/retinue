use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let source = include_str!("memory.x");
    fs::write(out.join("memory.x"), source).expect("write memory.x");

    // The Rust side needs both persistent-pair addresses, and the linker needs
    // all three kept clear of code. Deriving all of them from the same lines
    // means the pairs cannot drift into one another or application code.
    let (flash_origin, flash_length) = region(source, "FLASH");
    let (control_origin, control_length) = region(source, "CONTROL");
    let (reservation_origin, reservation_length) = region(source, "RESERVATION");
    let (store_origin, store_length) = region(source, "STORE");
    let page = 4096;
    let announce_lease = env::var("RETINUE_ANNOUNCE_LEASE_ORDINALS")
        .map(|value| {
            value
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("RETINUE_ANNOUNCE_LEASE_ORDINALS must be a decimal u64"))
        })
        .unwrap_or(65_536);
    assert!(
        (1..=(1_u64 << 40) - 1).contains(&announce_lease),
        "RETINUE_ANNOUNCE_LEASE_ORDINALS must fit the nonzero 40-bit timebase range"
    );

    assert_eq!(
        flash_origin + flash_length,
        control_origin,
        "FLASH must end exactly where CONTROL begins, or the linker can place code into durable state; adjust memory.x together"
    );
    assert_eq!(
        control_origin + control_length,
        reservation_origin,
        "CONTROL must end exactly where RESERVATION begins"
    );
    assert_eq!(
        reservation_origin + reservation_length,
        store_origin,
        "RESERVATION must end exactly where STORE begins"
    );
    assert_eq!(
        control_origin % page,
        0,
        "CONTROL must begin on a flash page boundary; NVMC erases whole pages"
    );
    assert_eq!(
        control_length,
        2 * page,
        "CONTROL is an A/B pair, so it is exactly two pages"
    );
    assert_eq!(
        reservation_origin % page,
        0,
        "RESERVATION must begin on a flash page boundary; NVMC erases whole pages"
    );
    assert_eq!(
        reservation_length,
        2 * page,
        "RESERVATION is an A/B pair, so it is exactly two pages"
    );
    assert_eq!(
        store_origin % page,
        0,
        "STORE must begin on a flash page boundary; the NVMC erases whole pages"
    );
    assert_eq!(
        store_length,
        2 * page,
        "the store is an A/B pair, so it is exactly two pages"
    );
    assert!(
        control_origin + control_length <= 0x000E_C000,
        "CONTROL must stay below the T114 bootloader region at 0xEC000"
    );
    assert!(
        store_origin + store_length <= 0x000E_C000,
        "STORE must stay below the T114 bootloader region at 0xEC000"
    );

    fs::write(
        out.join("store_region.rs"),
        format!(
            "/// Absolute flash address of the store's first page, from `memory.x`.\n\
             pub const STORE_ORIGIN: u32 = {store_origin:#010X};\n\
             /// Bytes the store spans, from `memory.x`.\n\
             pub const STORE_LENGTH: u32 = {store_length:#010X};\n"
        ),
    )
    .expect("write store_region.rs");

    fs::write(
        out.join("control_region.rs"),
        format!(
            "/// Absolute flash address of the control journal's first page, from `memory.x`.\n\
             pub const CONTROL_ORIGIN: u32 = {control_origin:#010X};\n\
             /// Bytes the control journal spans, from `memory.x`.\n\
             pub const CONTROL_LENGTH: u32 = {control_length:#010X};\n"
        ),
    )
    .expect("write control_region.rs");

    fs::write(
        out.join("reservation_region.rs"),
        format!(
            "/// Absolute flash address of the announce-reservation pair, from `memory.x`.\n\
             pub const RESERVATION_ORIGIN: u32 = {reservation_origin:#010X};\n\
             /// Bytes the reservation pair spans, from `memory.x`.\n\
             pub const RESERVATION_LENGTH: u32 = {reservation_length:#010X};\n"
        ),
    )
    .expect("write reservation_region.rs");

    fs::write(
        out.join("announce_lease.rs"),
        format!(
            "/// Ordinals reserved per native-node boot. Set by \
             `RETINUE_ANNOUNCE_LEASE_ORDINALS`, defaulting to 65,536.\n\
             pub const ANNOUNCE_TIMEBASE_LEASE: u64 = {announce_lease};\n"
        ),
    )
    .expect("write announce_lease.rs");

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-env-changed=RETINUE_ANNOUNCE_LEASE_ORDINALS");
}

/// Read one `NAME : ORIGIN = 0x…, LENGTH = 0x…` line out of a linker memory map.
fn region(source: &str, name: &str) -> (u32, u32) {
    let line = source
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(name))
        .unwrap_or_else(|| panic!("memory.x declares no {name} region"));
    (
        hex_field(line, name, "ORIGIN"),
        hex_field(line, name, "LENGTH"),
    )
}

fn hex_field(line: &str, name: &str, key: &str) -> u32 {
    let rest = line
        .split(key)
        .nth(1)
        .unwrap_or_else(|| panic!("{name} region declares no {key}"));
    let digits: String = rest
        .trim_start()
        .trim_start_matches('=')
        .trim_start()
        .trim_start_matches("0x")
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    u32::from_str_radix(&digits, 16)
        .unwrap_or_else(|_| panic!("{name} {key} is not a hexadecimal literal"))
}
