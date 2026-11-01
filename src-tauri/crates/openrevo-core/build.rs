use std::{env, fs, path::PathBuf};

#[path = "src/hal/vault_codegen.rs"]
mod vault_codegen;

fn main() {
    assert_eq!(
        env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("linux"),
        "openrevo-core supports Linux only"
    );
    let registry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/ec_registry.json");
    println!("cargo:rerun-if-changed={}", registry.display());
    println!("cargo:rerun-if-changed=src/hal/vault_codegen.rs");
    let source = fs::read_to_string(&registry).unwrap_or_else(|error| {
        panic!(
            "Cannot read hardware registry {}: {error}",
            registry.display()
        )
    });
    let generated = vault_codegen::generate(&source).unwrap_or_else(|error| {
        panic!("Invalid hardware registry {}: {error}", registry.display())
    });
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set by Cargo"));
    fs::write(output.join("generated_vault.rs"), generated)
        .expect("Cannot write generated hardware vault");
}
