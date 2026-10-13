fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let profile = std::env::var("PROFILE").unwrap();
        let source = root.join(format!("../../target/{profile}/storycut-daemon.exe"));
        println!("cargo:rerun-if-changed={}", source.display());
        let bytes = std::fs::read(&source).expect("Build the matching workspace storycut-daemon first (scripts/dev.ps1 or scripts/build.ps1)");
        let folder = root.join("binaries");
        std::fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("storycut-daemon.exe");
        if std::fs::read(&destination).ok().as_deref() != Some(bytes.as_slice()) {
            std::fs::write(destination, bytes).unwrap();
        }
    }
    tauri_build::build()
}
