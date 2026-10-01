//! Print a lab trace: `cargo run -p retinue-sim --example lab -- cold|warm`.

mod scenarios;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scenario = match std::env::args().nth(1).as_deref() {
        Some("cold") => scenarios::cold(),
        Some("warm") => scenarios::warm(),
        _ => return Err("usage: lab cold|warm".into()),
    };
    println!("{}", retinue_sim::run(&scenario)?.to_json());
    Ok(())
}
