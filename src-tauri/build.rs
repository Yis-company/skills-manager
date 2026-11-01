fn main() {
    // Use a custom Windows application manifest that opts the process into long
    // path support (#298/#299). app_manifest() replaces Tauri's default, so the
    // manifest file re-declares the Common-Controls v6 dependency Tauri ships.
    // On non-Windows targets the manifest is simply ignored.
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let windows = if msvc {
        // Embed once below for all targets, including library unit tests.
        // Keeping it in Tauri's binary-only resource as well duplicates it.
        tauri_build::WindowsAttributes::new_without_app_manifest()
    } else {
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("windows-app-manifest.xml"))
    };
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");

    // Tauri's resource is linked into app binaries, but native adapter unit tests
    // also need Common Controls v6 to load (tauri-apps/tauri#14580).
    if msvc {
        let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
