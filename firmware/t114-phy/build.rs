use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let source = include_str!("memory.x");
    fs::write(out.join("memory.x"), source).expect("write memory.x");

    // The Rust side needs the store's address, and the linker needs it kept
    // clear of code. Deriving both from the same lines means they cannot drift.
    let (flash_origin, flash_length) = region(source, "FLASH");
    let (store_origin, store_length) = region(source, "STORE");
    let page = 4096;

    assert_eq!(
        flash_origin + flash_length,
        store_origin,
        "FLASH must end exactly where STORE begins, or the linker can place code \
         into the store; adjust both in memory.x together"
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

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
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
