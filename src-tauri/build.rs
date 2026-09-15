fn main() {
    // tauri-build embeds the Windows app manifest (which requests Common Controls v6) into the app
    // executable only. Tauri imports TaskDialogIndirect, which exists only in Common Controls v6, so
    // test binaries without the manifest die at startup with STATUS_ENTRYPOINT_NOT_FOUND. Embedding
    // the same manifest through the linker covers every binary this package builds, tests included.
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
