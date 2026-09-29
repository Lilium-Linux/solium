//! Compiles the Qt Quick host shim and links Qt.
//!
//! Qt is discovered through pkg-config rather than hard-coded paths: the
//! container this builds in is Arch, CI is Ubuntu, and their Qt layouts differ.

use std::path::PathBuf;

fn main() {
    // The datadir a packager bakes in, read by `option_env!` in `assets.rs`.
    // Declared here so changing it rebuilds: an `option_env!` whose value has
    // moved on and whose crate has not been touched would otherwise keep the
    // old constant, and the symptom of that is an installed binary looking in
    // the previous prefix.
    println!("cargo:rerun-if-env-changed=SOLIUM_DATADIR");
    println!("cargo:rerun-if-changed=qml/host.cpp");
    println!("cargo:rerun-if-changed=qml/host.h");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .flag_if_supported("-fPIC")
        // Qt headers are not warning-clean under our settings, and they are
        // not ours to fix.
        .flag_if_supported("-Wno-unused-parameter")
        .file("qml/host.cpp");

    // Qt6Quick pulls in Core, Gui and Qml transitively.
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

    build.compile("solium_qml_host");
}
