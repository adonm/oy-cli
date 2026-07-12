fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if !matches!(target.as_str(), "linux" | "macos") {
        panic!("oy-cli supports Linux and macOS only; Windows users should run oy-cli in WSL2");
    }
}
