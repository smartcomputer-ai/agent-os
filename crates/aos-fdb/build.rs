use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=FDB_CLIENT_LIB_PATH");

    if let Ok(lib_path) = env::var("FDB_CLIENT_LIB_PATH") {
        println!("cargo:rustc-link-search=native={lib_path}");
        return;
    }

    if cfg!(target_os = "macos") {
        let default_path = "/usr/local/lib";
        if Path::new(default_path).join("libfdb_c.dylib").exists() {
            println!("cargo:rustc-link-search=native={default_path}");
        }
    }
}
