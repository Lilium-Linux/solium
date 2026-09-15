//! Compiles the Qt Quick host shim and links Qt.
//!
//! Qt is discovered through pkg-config rather than hard-coded paths: the
//! container this builds in is Arch, CI is Ubuntu, and their Qt layouts differ.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=qml/host.cpp");
    println!("cargo:rerun-if-changed=qml/host.h");
    println!("cargo:rerun-if-changed=qml/compat.cpp");
    println!("cargo:rerun-if-changed=qml/compat.h");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .flag_if_supported("-fPIC")
        // Qt headers are not warning-clean under our settings, and they are
        // not ours to fix.
        .flag_if_supported("-Wno-unused-parameter")
        .file("qml/host.cpp")
        .file("qml/compat.cpp");

    // Qt6Quick pulls in Core, Gui and Qml transitively.
    // Network too: the compat layer's Socket is a QLocalSocket, and Qt6Quick
    // does not pull it in.
    let _ = pkg_config::Config::new()
        .atleast_version("6.5")
        .probe("Qt6Network");
    let qt = match pkg_config::Config::new()
        .atleast_version("6.5")
        .probe("Qt6Quick")
    {
        Ok(qt) => qt,
        Err(err) => {
            // A build script cannot carry on without its dependency, and a
            // panic backtrace here would bury the one useful line. Say what is
            // missing and what installs it.
            eprintln!("error: Qt 6 Quick development files not found: {err}");
            eprintln!("       Debian/Ubuntu: qt6-base-dev qt6-declarative-dev");
            eprintln!("       Arch:          qt6-base qt6-declarative");
            std::process::exit(1);
        }
    };

    for path in &qt.include_paths {
        build.include(path);
        // Qt's own headers include each other as <QtGui/...> and as bare
        // <qopenglcontext.h>, so both the umbrella and module directories
        // have to be on the path.
        for module in ["QtCore", "QtGui", "QtQml", "QtQuick"] {
            let module_path: PathBuf = path.join(module);
            if module_path.is_dir() {
                build.include(&module_path);
            }
        }
    }

    // EGL, for the GPU scene path in host.cpp: importing the compositor's
    // dmabuf as a texture and fencing the frame afterwards are both EGL, and Qt
    // neither re-exports them nor offers an equivalent. Not optional even though
    // the GPU path is — host.cpp references these symbols unconditionally, so
    // without them the link fails a long way from the cause.
    //
    // libGLESv2 is deliberately *not* linked: the handful of plain GL calls go
    // through QOpenGLFunctions, which is by definition the GL that Qt's own RHI
    // is driving. See import_dmabuf_texture.
    match pkg_config::Config::new().probe("egl") {
        Ok(egl) => {
            for path in &egl.include_paths {
                build.include(path);
            }
        }
        Err(err) => {
            eprintln!("error: EGL development files not found: {err}");
            eprintln!("       Debian/Ubuntu: libegl-dev");
            eprintln!("       Fedora:        mesa-libEGL-devel");
            eprintln!("       Arch:          mesa");
            std::process::exit(1);
        }
    }

    // compat.h declares Q_OBJECT types — properties and signals are the whole
    // point of them — so it needs moc, which host.cpp deliberately never did.
    let out: PathBuf = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    let generated = out.join("moc_compat.cpp");
    let moc = find_moc(&qt);
    let status = std::process::Command::new(&moc)
        .arg("qml/compat.h")
        .arg("-o")
        .arg(&generated)
        .status();
    match status {
        Ok(status) if status.success() => build.file(&generated),
        Ok(status) => {
            eprintln!("error: moc failed ({status}) on qml/compat.h");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("error: could not run moc at {}: {err}", moc.display());
            eprintln!("       set QT_MOC to its path, or install Qt 6's development tools");
            std::process::exit(1);
        }
    };

    build.compile("solium_qml_host");
}

/// Where moc is.
///
/// pkg-config describes libraries, not tools, so Qt's own binaries are not in
/// what it reports. The layout differs by distribution, which is why this
/// looks rather than assumes.
fn find_moc(qt: &pkg_config::Library) -> PathBuf {
    if let Some(path) = std::env::var_os("QT_MOC") {
        return PathBuf::from(path);
    }
    // Alongside the libraries Qt reported, which is where distributions put it.
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
