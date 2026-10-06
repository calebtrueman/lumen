fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // Only when building for the BIOS target (not `cargo check` on a host).
    if std::env::var("TARGET").unwrap_or_default().contains("lumen-bios") {
        println!("cargo:rustc-link-arg-bin=stage2=-T{dir}/stage2.ld");
        println!("cargo:rustc-link-arg-bin=lumen=-T{dir}/lumen.ld");
    }
    println!("cargo:rerun-if-changed=stage2.ld");
    println!("cargo:rerun-if-changed=lumen.ld");
}
