#[derive(Clone, Debug)]
pub struct BuildConfig {
    pub toolchain: Toolchain,
    pub release: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            toolchain: Toolchain::default(),
            release: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Toolchain {
    pub rustup_toolchain: Option<String>,
    pub target: String,
}

impl Default for Toolchain {
    fn default() -> Self {
        Self {
            rustup_toolchain: None,
            target: "wasm32-unknown-unknown".into(),
        }
    }
}
