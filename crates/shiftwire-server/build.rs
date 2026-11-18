//! The web UI is embedded from `ui/dist`. Until it has been built (`npm run build`
//! in `ui/`), embed a placeholder page so `cargo build` always works.

use std::{fs, path::Path};

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let index = dist.join("index.html");
    if !index.exists() {
        fs::create_dir_all(&dist).expect("create ui/dist");
        fs::write(
            &index,
            "<!doctype html><title>shiftWire</title><body style=\"font-family:system-ui;padding:3rem\">\
             <h1>shiftWire is running</h1><p>The UI has not been built yet: run <code>npm run build</code> in <code>ui/</code>.</p>",
        )
        .expect("write placeholder ui");
    }
}
