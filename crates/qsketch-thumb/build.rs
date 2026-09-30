fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.ends_with("windows-gnu") {
        // Explorer loads the provider into its surrogate process, whose DLL
        // search path does not include our install directory, so no MinGW
        // runtime may be a separate DLL next to us: link libgcc statically.
        println!("cargo:rustc-link-arg=-static-libgcc");
    }
}
