#[path = "build_support/standard_identity.rs"]
mod standard_identity;

fn main() {
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies the manifest directory"),
    );
    let root = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the compiler belongs to the Tondo workspace");
    let inputs = standard_identity::read_inputs(root)
        .expect("the hosted standard implementation must have a complete source bundle");
    for path in standard_identity::INPUTS {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    let hash =
        standard_identity::fingerprint(&inputs).expect("standard source paths must be unique");
    println!("cargo:rustc-env=TONDO_BOOTSTRAP_STANDARD_HASH={hash}");
}
