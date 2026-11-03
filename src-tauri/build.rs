use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    build_apple_speech_helper();
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    embed_windows_manifest();
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}

/// Embed tauri-build's application manifest (Common Controls v6) into every
/// Windows binary, test executables included. tauri-build only embeds it into
/// the app, and without it a test binary that links the dialog code fails to
/// start with STATUS_ENTRYPOINT_NOT_FOUND (`TaskDialogIndirect` lives only in
/// Common Controls v6).
fn embed_windows_manifest() {
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    let windows_msvc = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !windows_msvc {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("windows-app-manifest.xml");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}

/// Compile the Apple Speech helper (`apple-speech/main.swift`) on macOS.
///
/// The binary is embedded into the app (`apple_speech.rs`) and written out at
/// runtime, like the CrisperWhisper runner script. SpeechAnalyzer needs the
/// macOS 26 SDK; without it (or without Swift at all) the app still builds and
/// reports the engine as unavailable.
fn build_apple_speech_helper() {
    println!("cargo:rustc-check-cfg=cfg(apple_speech_helper)");
    println!("cargo:rerun-if-changed=apple-speech/main.swift");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => return,
    };

    let output = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("apple-speech-helper");
    let result = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "swiftc",
            "-parse-as-library",
            "-O",
            "-target",
        ])
        .arg(format!("{arch}-apple-macos26.0"))
        .arg("apple-speech/main.swift")
        .arg("-o")
        .arg(&output)
        .output();

    match result {
        Ok(result) if result.status.success() => {
            println!("cargo:rustc-cfg=apple_speech_helper");
            println!("cargo:rustc-env=APPLE_SPEECH_HELPER={}", output.display());
        }
        Ok(result) => println!(
            "cargo:warning=Apple Speech helper not built (needs the macOS 26 SDK); the engine \
             will be unavailable. swiftc: {}",
            String::from_utf8_lossy(&result.stderr)
                .lines()
                .next()
                .unwrap_or_default()
        ),
        Err(error) => println!(
            "cargo:warning=Apple Speech helper not built ({error}); the engine will be unavailable."
        ),
    }
}
