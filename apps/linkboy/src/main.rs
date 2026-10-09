#![forbid(unsafe_code)]
// Same posture as the library root: large Err payloads are deliberate on this cold path.
#![allow(clippy::result_large_err)]

//! The linkboy, as a terminal.
//!
//! ```text
//! linkboy list                    what is on this machine's ports
//! linkboy inspect PACKAGE            verify and explain a package
//! linkboy catalog INDEX           verify a public package index
//! linkboy catalog-auth INDEX TRUST
//!                                      require local publisher trust and a valid signature
//! linkboy plan DEVICE PACKAGE [BOARD@REVISION]
//!                                      produce a refusal or immutable flash plan
//! linkboy flash DEVICE PACKAGE [BOARD@REVISION] [--loader-snapshot PATH] [--receipt PATH]
//!                                      execute an accepted package plan
//! linkboy flash-volume VOLUME PACKAGE BOARD@REVISION [--receipt PATH]
//!                                      execute an admitted T114 UF2 package
//! linkboy capture-t114-loader VOLUME PATH
//!                                      capture the HT-n5262 UF2 and SoftDevice record
//! linkboy make-uf2 BIN UF2 BASE FAMILY
//!                                      reproducibly package a raw application as UF2
//! linkboy verify-recovery PORT PACKAGE BOARD@REVISION RECOVERY --loader-snapshot PATH [--receipt PATH]
//!                                      verify a completed post-write recovery without writing
//! linkboy flash-raw PORT IMAGE [t114|v4]
//!                                      expert-only bench route for a raw image
//! linkboy bootloader PORT         send a T114 to its bootloader and name the new port
//! ```
//!
//! Public T114 packages use Linkboy's built-in UF2 writer. V4 packages use an admitted,
//! platform-specific `espflash` release for the ESP ROM loader; serial DFU remains an expert
//! T114 recovery route. Linkboy adds the part that is fiddly by hand and undocumented in one place: knowing
//! which board it is talking to, sending it to its bootloader, finding the port it comes back
//! on, and refusing to write anything until all of that is settled.

mod cli;

use cli::run_command;

fn main() {
    if let Err(error) = run_command() {
        eprintln!("linkboy: {error}");
        std::process::exit(1);
    }
}
