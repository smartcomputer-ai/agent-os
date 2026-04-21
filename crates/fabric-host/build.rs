use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=LIBKRUN_BUNDLE");
    println!("cargo:rerun-if-env-changed=LIBKRUN_DIR");

    if env::var_os("CARGO_FEATURE_SMOLVM_RUNTIME").is_none() {
        return;
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    let default_lib_dir = default_libkrun_bundle();
    println!(
        "cargo:rerun-if-changed={}",
        default_lib_dir.join("libkrun.dylib").display()
    );

    let lib_dir = env::var_os("LIBKRUN_BUNDLE")
        .or_else(|| env::var_os("LIBKRUN_DIR"))
        .map(PathBuf::from)
        .or_else(|| {
            let lib_path = default_lib_dir.join("libkrun.dylib");
            lib_path.exists().then_some(default_lib_dir)
        })
        .unwrap_or_else(|| {
            panic!(
                "libkrun.dylib was not found. Run dev/fabric/bootstrap-smolvm-release.sh or set LIBKRUN_BUNDLE."
            )
        });

    let lib_path = lib_dir.join("libkrun.dylib");
    if !lib_path.exists() {
        panic!(
            "libkrun.dylib was not found at {}. Run dev/fabric/bootstrap-smolvm-release.sh or set LIBKRUN_BUNDLE.",
            lib_path.display()
        );
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());

    // smolvm weak-links libkrun from its own build script, but that link arg
    // does not propagate when fabric embeds smolvm as a library. Add the
    // concrete native link here in the package that owns the smolvm runtime.
    println!("cargo:rustc-link-lib=dylib=krun");
}

fn default_libkrun_bundle() -> PathBuf {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
    );
    manifest_dir.join("../../third_party/smolvm-release/lib")
}
