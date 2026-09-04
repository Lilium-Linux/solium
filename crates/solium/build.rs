//! Compiles the Qt Quick host shim and links Qt.
//!
//! Qt is discovered through pkg-config rather than hard-coded paths: the
//! container this builds in is Arch, CI is Ubuntu, and their Qt layouts differ.

use std::path::PathBuf;

fn main() {
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

    build.compile("solium_qml_host");
}
