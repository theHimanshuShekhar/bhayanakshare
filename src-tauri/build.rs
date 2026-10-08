fn main() {
    // The application manifest is embedded by the linker instead (below), so that the programs
    // `cargo test` builds get it too; tauri-build's own copy would be a second one in the app.
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
    embed_manifest();
}

/// The dialogs and the tray need Common Controls v6, which a Windows program asks for in its
/// application manifest. tauri-build embeds one only in the app (a link argument for `bins`),
/// so the test programs, which link the same code, stop at start-up with
/// STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139). Cargo has no link argument for a library's unit
/// tests alone, so the linker embeds this same manifest in everything that is linked.
fn embed_manifest() {
    const MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
</assembly>
"#;
    let is = |var: &str, value: &str| std::env::var(var).is_ok_and(|v| v == value);
    if !is("CARGO_CFG_TARGET_OS", "windows") || !is("CARGO_CFG_TARGET_ENV", "msvc") {
        return;
    }
    let path = std::path::Path::new(&std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("app.manifest");
    std::fs::write(&path, MANIFEST).expect("write the application manifest");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", path.display());
    // Without this the linker adds a requestedExecutionLevel, which tauri's manifest has none of.
    println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
}
