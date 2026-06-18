//! Build script: copy the vendored Mesa software-OpenGL runtime next to the
//! built executable.
//!
//! The app forces Mesa's `llvmpipe` software rasterizer (see `main.rs`) so it
//! renders on machines without a usable GPU/OpenGL driver (e.g. VMs, cloud
//! hosts). That only works if Mesa's `opengl32.dll` and its `libgallium_wgl.dll`
//! gallium driver sit next to `Dinosaur.exe`. We vendor those DLLs under
//! `vendor/mesa/` and copy them into the target output directory on every build
//! so the binary always has them, even after `cargo clean`.

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    // Only relevant on Windows; the Mesa DLLs are Windows-only.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor_dir = manifest_dir.join("vendor").join("mesa");

    // OUT_DIR is `<target>/<profile>/build/<crate>-<hash>/out`; the executable
    // lives four levels up at `<target>/<profile>`.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let exe_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has the expected depth")
        .to_path_buf();

    for dll in ["opengl32.dll", "libgallium_wgl.dll"] {
        let src = vendor_dir.join(dll);
        let dst = exe_dir.join(dll);
        println!("cargo:rerun-if-changed={}", src.display());
        copy_if_newer(&src, &dst);
    }
}

/// Copy `src` to `dst` unless `dst` already exists with the same size (avoids
/// recopying the large gallium driver on every incremental build).
fn copy_if_newer(src: &Path, dst: &Path) {
    if !src.exists() {
        println!(
            "cargo:warning=vendored Mesa DLL missing: {} (the app may fail to \
             start on machines without an OpenGL 2.0+ driver)",
            src.display()
        );
        return;
    }
    let same = fs::metadata(src)
        .ok()
        .zip(fs::metadata(dst).ok())
        .map(|(a, b)| a.len() == b.len())
        .unwrap_or(false);
    if same {
        return;
    }
    if let Err(e) = fs::copy(src, dst) {
        println!(
            "cargo:warning=failed to copy {} to {}: {e}",
            src.display(),
            dst.display()
        );
    }
}
