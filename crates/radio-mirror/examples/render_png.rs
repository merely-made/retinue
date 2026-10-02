//! Renders one screen to a PNG from JSON status documents.
//!
//! cargo run -p radio-mirror --features png --example render_png -- \
//!     <surface> <screen> <local.json> <host.json|-> <out.png> [mono|receipt]

use std::{error::Error, fs, path::PathBuf};

use radio_mirror::{input, mono_theme, names, receipt_theme, render_png};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [surface, screen, local, host, out, rest @ ..] = args.as_slice() else {
        return Err(
            "usage: <surface> <screen> <local.json> <host.json|-> <out.png> [mono|receipt]".into(),
        );
    };
    let surface = names::surface(surface)?;
    let theme = match rest.first().map(String::as_str) {
        None | Some("mono") => mono_theme(),
        Some("receipt") => receipt_theme(surface),
        Some(other) => return Err(format!("unknown palette {other:?}").into()),
    };
    let local = input::local_from_json(&fs::read_to_string(local)?)?;
    let host = match host.as_str() {
        "-" => None,
        path => Some(input::host_from_json(&fs::read_to_string(path)?)?),
    };
    let png = render_png(
        surface,
        theme,
        names::screen(screen)?,
        &local,
        host.as_ref(),
    )?;
    let out = PathBuf::from(out);
    fs::write(&out, png)?;
    println!("{}", out.display());
    Ok(())
}
