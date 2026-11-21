use std::{env, fs, io, path::PathBuf};

#[allow(dead_code)]
#[path = "src/plugins/runtime/trust_inventory.rs"]
mod trust_inventory;

fn main() {
    // Cargo owns OUT_DIR, including any custom target root or target triple:
    // <profile>/build/mangoo-core-<hash>/out.
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let profile = out.ancestors().nth(3).expect("Cargo profile directory");
    let inventory = profile.join("bundled-plugins/.inventory.json");
    println!("cargo:rerun-if-changed={}", inventory.display());
    println!("cargo:rerun-if-changed=src/plugins/runtime/trust_inventory.rs");
    let bytes = match fs::read(&inventory) {
        Ok(bytes) => {
            let rows = trust_inventory::entries(&bytes)
                .unwrap_or_else(|error| panic!("invalid {}: {error}", inventory.display()));
            assert!(
                !rows.is_empty(),
                "empty staged inventory: {}",
                inventory.display()
            );
            bytes
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A plain cargo build on a fresh checkout can run without plugins.
            // Only a subsequent explicit staging/build can authorize artifacts.
            br#"{"schemaVersion":1,"plugins":[]}"#.to_vec()
        }
        Err(error) => panic!("cannot read {}: {error}", inventory.display()),
    };
    let payload = [
        trust_inventory::PREFIX,
        bytes.as_slice(),
        trust_inventory::SUFFIX,
    ]
    .concat();
    let output = out.join("bundled-trust.bin");
    if fs::read(&output).ok().as_deref() != Some(&payload) {
        fs::write(output, payload).expect("write compiled trust inventory");
    }
}
