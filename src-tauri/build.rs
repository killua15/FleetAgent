fn main() {
    embed_manifest_in_test_binaries();
    tauri_build::build()
}

/// Gives `cargo test`'s executables the Windows manifest that `tauri-build`
/// only gives the app.
///
/// Without it every test on Windows dies before `main` with
/// STATUS_ENTRYPOINT_NOT_FOUND: the manifest is what pins `comctl32.dll` to
/// version 6, and the v5 copy the loader picks otherwise has none of the
/// subclassing entry points Tauri's window code imports. The failure has no
/// output and names no symbol, so it reads as "all tests broken" rather than
/// "one missing dependency declaration".
///
/// `-tests` rather than `-bins` is the whole point — the app binary is already
/// handled, and this must not touch what gets bundled.
///
/// Upstream bug, still open: https://github.com/tauri-apps/tauri/issues/13419
fn embed_manifest_in_test_binaries() {
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");

    // The *target*, not the host: `cfg!(windows)` here would describe the
    // machine running this build script, which is the wrong question.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // `/MANIFEST:EMBED` is a `link.exe` flag. The GNU toolchain embeds
    // manifests by linking a resource object instead, and passing MSVC flags
    // to it would just fail the link.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }

    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}", manifest.display());
    // Without this, embedding a manifest makes the linker add its own default
    // requestedExecutionLevel, and a test binary asking for elevation would
    // hang CI on a UAC prompt nobody can answer.
    println!("cargo:rustc-link-arg-tests=/MANIFESTUAC:NO");
}
