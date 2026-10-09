//! Opt-in physical packet-adapter proof; run through testing/mc3_personality_bench.py.
//! Host-driven retained Retinue Node link and Sennet state; no autonomous board scheduling.

mod adapters;
mod excursion;
mod radio;
mod run;
mod session;

use serde_json::json;
use std::path::PathBuf;
use tokio::time::Instant;
use tulle::personality::{CoverageEvidence, PersonalityId};

use run::run;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const HOME: PersonalityId = PersonalityId(1);
const OTHER: PersonalityId = PersonalityId(2);
const GAP: CoverageEvidence = CoverageEvidence { valid_until: None };
const EXCURSION_MS: u64 = 25_000;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!(
            "usage: murmuration_probe DUT_PORT PEER_PORT OUTPUT_JSON (guard with Python runner)"
        );
        std::process::exit(2);
    }
    let output = PathBuf::from(&args[2]);
    let mut events = Vec::new();
    let start = Instant::now();
    let outcome = run(&args[0], &args[1], &output, &mut events).await;
    let passed = outcome.is_ok();
    let report = json!({"passed":passed,"error":outcome.err().map(|e|e.to_string()),
        "elapsed_ms":start.elapsed().as_secs_f64()*1000.0,"events":events,
        "scope":"host-driven retained Retinue Node link and Sennet state through board radio owner; no autonomous firmware or third-party peer survival claim"});
    if let Err(e) = std::fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()) {
        eprintln!("receipt write: {e}");
        std::process::exit(2);
    }
    println!("{}", report);
    if !passed {
        std::process::exit(1);
    }
}
