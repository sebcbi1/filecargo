//! Windows only: embeds the app icon in `filecargo.exe` as icon resource 1, which GPUI loads for
//! the window class (title bar, taskbar, Alt-Tab) and Explorer shows on the file.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    // absolute: the resource compiler resolves relative include paths against its own directory
    let icons = manifest_dir.join("../../assets/icons");
    println!("cargo:rerun-if-changed=windows/filecargo.rc");
    println!(
        "cargo:rerun-if-changed={}",
        icons.join("filecargo.ico").display()
    );
    if let Err(error) = embed_resource::compile_for(
        "windows/filecargo.rc",
        ["filecargo"],
        embed_resource::ParamsIncludeDirs([icons]),
    )
    .manifest_required()
    {
        panic!("cannot embed the app icon: {error}");
    }
}
