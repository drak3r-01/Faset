//! Exposes the compilation target triple to the binary.
fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=PROBE_TARGET={target}");
}
