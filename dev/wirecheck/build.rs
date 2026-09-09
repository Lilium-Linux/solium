//! Compiles the *real* `crates/solium/qml/host.cpp` from this checkout.
//!
//! Deliberately NOT linking `target/debug/build/solium-*/out/libsolium_qml_host.a`:
//! there is more than one such directory and a glob picks a stale one silently
//! -- see the trap in dev/README.md. This compiles the source next to it, so
//! what runs is what is checked out.

use std::path::{Path, PathBuf};

/// The repository this harness belongs to: two levels up from `dev/wirecheck`.
///
/// Derived rather than written down, so a worktree, a clone under another name
/// or somebody else's checkout all compile the host.cpp sitting next to them
/// rather than one that happens to be on this machine.
fn repo() -> PathBuf {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .expect("dev/wirecheck is two levels below the repository root")
        .to_path_buf()
}

fn main() {
    let qml = repo().join("crates/solium/qml");
    println!("cargo:rerun-if-changed={}", qml.join("host.cpp").display());
    println!("cargo:rerun-if-env-changed=WIRECHECK_HOST_CPP");
    // Our own C++ too. cc-rs does not always emit these, and a stale object
    // file here shows up as an undefined symbol at link time rather than as
    // anything to do with the file that was edited.
    println!("cargo:rerun-if-changed=src/host_tu.cpp");
    println!("cargo:rerun-if-changed=src/readback.cpp");
    println!("cargo:rerun-if-changed={}", qml.join("compat.cpp").display());

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .flag_if_supported("-fPIC")
        .flag_if_supported("-Wno-unused-parameter")
        .include(&qml)
        // src/host_tu.cpp is a five-line translation unit whose only job is to
        // #include the real host.cpp verbatim. host.cpp is compiled exactly
        // once, from this checkout, unmodified.
        .file("src/host_tu.cpp")
        .file(qml.join("compat.cpp"));
    // Overridable only so a deliberately broken copy can be compiled as a
    // negative control; the default is always the host.cpp beside this crate.
    let host = std::env::var_os("WIRECHECK_HOST_CPP")
        .map(PathBuf::from)
        .unwrap_or_else(|| qml.join("host.cpp"));
    build.define(
        "WIRECHECK_HOST_CPP",
        format!("\"{}\"", host.display()).as_str(),
    );

    let _ = pkg_config::Config::new()
        .atleast_version("6.5")
        .probe("Qt6Network");
    let qt = pkg_config::Config::new()
        .atleast_version("6.5")
        .probe("Qt6Quick")
        .expect("Qt6Quick dev files");

    for path in &qt.include_paths {
        build.include(path);
        for module in ["QtCore", "QtGui", "QtQml", "QtQuick"] {
            let module_path: PathBuf = path.join(module);
            if module_path.is_dir() {
                build.include(&module_path);
            }
        }
    }

    let egl = pkg_config::Config::new().probe("egl").expect("EGL dev files");
    for path in &egl.include_paths {
        build.include(path);
    }

    // moc, exactly as the crate's own build.rs does.
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let generated = out.join("moc_compat.cpp");
    let moc = find_moc(&qt);
    let status = std::process::Command::new(&moc)
        .arg(qml.join("compat.h"))
        .arg("-o")
        .arg(&generated)
        .status()
        .expect("running moc");
    assert!(status.success(), "moc failed");
    build.file(&generated);

    build.compile("solium_qml_host");

    // The readback: an independent EGL display on its own GBM device, no Qt and
    // no smithay. The control for "can *anything* see these pixels", which is
    // what separates "the compositor cannot see them" from "there are none".
    let mut rb = cc::Build::new();
    rb.cpp(true).std("c++17").flag_if_supported("-fPIC");
    for path in &egl.include_paths {
        rb.include(path);
    }
    rb.file("src/readback.cpp");
    rb.compile("wirereadback");


    let _ = pkg_config::Config::new().probe("gbm");
    let _ = pkg_config::Config::new().probe("glesv2");
    println!("cargo:rustc-link-lib=dylib=EGL");
    println!("cargo:rustc-link-lib=dylib=GLESv2");
    println!("cargo:rustc-link-lib=dylib=gbm");
}

fn find_moc(qt: &pkg_config::Library) -> PathBuf {
    if let Some(path) = std::env::var_os("QT_MOC") {
        return PathBuf::from(path);
    }
    for directory in &qt.link_paths {
        for candidate in [
            directory.join("qt6/libexec/moc"),
            directory.join("qt6/bin/moc"),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    for candidate in [
        "/usr/lib64/qt6/libexec/moc",
        "/usr/lib/qt6/libexec/moc",
        "/usr/lib/x86_64-linux-gnu/qt6/libexec/moc",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("moc")
}
