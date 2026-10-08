//! Print a lab trace: `cargo run -p retinue-sim --example lab -- cold|warm`.
//!
//! With `--faces` (and `--features face`), print the trace's face track instead
//! (`retinue-sim.face-track/v1`), a separate file derived from the same run:
//! `cargo run -p retinue-sim --example lab --features face -- cold --faces`.

mod scenarios;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const USAGE: &str = "usage: lab cold|warm [--faces]";
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (scenario, faces) = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [name] => (name, false),
        [name, "--faces"] => (name, true),
        _ => return Err(USAGE.into()),
    };
    let scenario = match scenario {
        "cold" => scenarios::cold(),
        "warm" => scenarios::warm(),
        _ => return Err(USAGE.into()),
    };
    let trace = retinue_sim::run(&scenario)?;
    if faces {
        println!("{}", face_track(&trace)?);
    } else {
        println!("{}", trace.to_json());
    }
    Ok(())
}

#[cfg(feature = "face")]
fn face_track(trace: &retinue_sim::Trace) -> Result<String, Box<dyn std::error::Error>> {
    Ok(retinue_sim::face::FaceTrack::from_trace(trace).to_json())
}

#[cfg(not(feature = "face"))]
fn face_track(_: &retinue_sim::Trace) -> Result<String, Box<dyn std::error::Error>> {
    Err("--faces needs `--features face`".into())
}
