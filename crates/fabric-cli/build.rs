use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=LIBKRUN_BUNDLE");
    println!("cargo:rerun-if-env-changed=LIBKRUN_DIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    if let Some(lib_dir) = env::var_os("LIBKRUN_BUNDLE").or_else(|| env::var_os("LIBKRUN_DIR")) {
        let lib_dir = PathBuf::from(lib_dir);
        if lib_dir.join("libkrun.dylib").exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
        }
    }
}
