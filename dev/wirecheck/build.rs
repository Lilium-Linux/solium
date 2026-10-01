//! Compiles the *real* `crates/solium/qml/host.cpp` from this checkout.
//!
//! Deliberately NOT linking `target/debug/build/solium-*/out/libsolium_qml_host.a`:
//! there is more than one such directory and a glob picks a stale one silently
//! -- see the trap in dev/README.md. This compiles the source next to it, so
//! what runs is what is checked out.

use std::path::{Path, PathBuf};

/// The headers moc runs on, in crates/solium/qml: the same list as
/// `MOC_HEADERS` in crates/solium/build.rs.
const MOC_HEADERS: &[&str] = &["attached.h"];

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
    println!("cargo:rerun-if-changed={}", qml.join("attached.cpp").display());
    println!("cargo:rerun-if-changed={}", qml.join("attached.h").display());
    println!("cargo:rerun-if-env-changed=WIRECHECK_HOST_CPP");
    println!("cargo:rerun-if-env-changed=QT_MOC");
    // Our own C++ too. cc-rs does not always emit these, and a stale object
    // file here shows up as an undefined symbol at link time rather than as
    // anything to do with the file that was edited.
    println!("cargo:rerun-if-changed=src/host_tu.cpp");
    println!("cargo:rerun-if-changed=src/readback.cpp");

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
        // host.cpp includes attached.h and calls into attached.cpp, so the
        // real one is compiled beside it, with its moc output below: the
        // gate's wirecheck step links only with both.
        .file(qml.join("attached.cpp"));
    // Overridable only so a deliberately broken copy can be compiled as a
    // negative control; the default is always the host.cpp beside this crate.
    let host = std::env::var_os("WIRECHECK_HOST_CPP")
        .map(PathBuf::from)
        .unwrap_or_else(|| qml.join("host.cpp"));
    // Whichever file that turned out to be, and not only the default one at the
    // top of this function.
    //
    // Without this, editing a *control copy* while host.cpp is unchanged
    // rebuilds nothing: the env var has not changed either, so cargo never
    // re-runs this script and the old object file is linked. The binary is then
    // built from a host.cpp that is no longer on disk -- and the check this
    // README used to prescribe, `diff`ing the two source files, inspects the
    // files and never the binary, so it reports exactly what you hoped to see.
    //
    // Demonstrated rather than theorised: replacing host-control-render.cpp
    // with a byte-identical copy of host.cpp and rebuilding finished in 0.05s
    // without compiling anything, `diff` then called the files identical, and
    // the binary went on failing on a control that was not in its source. It
    // fires whenever a control's awk expression is edited and host.cpp is not,
    // which is what adding a control for a new path looks like.
    println!("cargo:rerun-if-changed={}", host.display());
    build.define(
        "WIRECHECK_HOST_CPP",
        format!("\"{}\"", host.display()).as_str(),
    );

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

    // The same moc step as crates/solium/build.rs, on the same header; the
    // gate's wirecheck step builds only with it.
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let moc = find_moc(&qt);
    for header in MOC_HEADERS.iter().map(|name| qml.join(name)) {
        let stem = header
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("moc")
            .to_owned();
        let generated = out.join(format!("moc_{stem}.cpp"));
        let includes = qt.include_paths.iter().flat_map(|path| {
            std::iter::once(path.clone())
                .chain(["QtCore", "QtGui", "QtQml", "QtQuick"].map(|module| path.join(module)))
        });
        let status = std::process::Command::new(&moc)
            .args(includes.map(|path| format!("-I{}", path.display())))
            .arg(&header)
            .arg("-o")
            .arg(&generated)
            .status()
            .expect("moc runs; set QT_MOC to its path");
        assert!(status.success(), "moc failed ({status}) on {}", header.display());
        build.file(&generated);
    }

    let egl = pkg_config::Config::new().probe("egl").expect("EGL dev files");
    for path in &egl.include_paths {
        build.include(path);
    }

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

/// Where moc is: the same search as `find_moc` in crates/solium/build.rs.
fn find_moc(qt: &pkg_config::Library) -> PathBuf {
    if let Some(path) = std::env::var_os("QT_MOC") {
        return PathBuf::from(path);
    }
    if let Ok(directory) = pkg_config::get_variable("Qt6Core", "libexecdir") {
        let candidate = PathBuf::from(directory).join("moc");
        if candidate.is_file() {
            return candidate;
        }
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
