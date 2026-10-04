//! Compiles the Qt Quick host shim and links Qt.
//!
//! Qt is discovered through pkg-config rather than hard-coded paths: the build
//! container and CI are Fedora, but whoever builds a package may be on any
//! distribution, and distributions lay Qt out differently.

use std::path::PathBuf;

/// The headers moc runs on: every one that declares a Q_OBJECT type.
/// `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`.
const MOC_HEADERS: &[&str] = &[
    "qml/attached.h",
    "qml/rows.h",
    "qml/keyboard.h",
    "qml/pointer.h",
];

fn main() {
    // The datadir a packager bakes in, read by `option_env!` in `assets.rs`.
    // Declared here so changing it rebuilds: an `option_env!` whose value has
    // moved on and whose crate has not been touched would otherwise keep the
    // old constant, and the symptom of that is an installed binary looking in
    // the previous prefix.
    println!("cargo:rerun-if-env-changed=SOLIUM_DATADIR");
    println!("cargo:rerun-if-env-changed=QT_MOC");
    println!("cargo:rerun-if-changed=qml/host.cpp");
    println!("cargo:rerun-if-changed=qml/host.h");
    println!("cargo:rerun-if-changed=qml/attached.cpp");
    println!("cargo:rerun-if-changed=qml/attached.h");
    println!("cargo:rerun-if-changed=qml/rows.cpp");
    println!("cargo:rerun-if-changed=qml/rows.h");
    println!("cargo:rerun-if-changed=qml/keyboard.cpp");
    println!("cargo:rerun-if-changed=qml/keyboard.h");
    println!("cargo:rerun-if-changed=qml/pointer.cpp");
    println!("cargo:rerun-if-changed=qml/pointer.h");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .flag_if_supported("-fPIC")
        // Qt headers are not warning-clean under our settings, and they are
        // not ours to fix.
        .flag_if_supported("-Wno-unused-parameter")
        .file("qml/host.cpp")
        .file("qml/attached.cpp")
        .file("qml/rows.cpp")
        .file("qml/keyboard.cpp")
        .file("qml/pointer.cpp")
        .include("qml");

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
            eprintln!("error: Qt 6 Quick (6.5 or newer) development files not found: {err}");
            eprintln!("       Fedora:        qt6-qtbase-devel qt6-qtdeclarative-devel");
            eprintln!("       Arch:          qt6-base qt6-declarative");
            eprintln!("       Debian/Ubuntu: qt6-base-dev qt6-declarative-dev");
            eprintln!("                      (Debian 13, Ubuntu 24.10 or newer)");
            eprintln!("       openSUSE:      qt6-base-devel qt6-declarative-devel");
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

    // attached.h, rows.h, keyboard.h and pointer.h declare Q_OBJECT types --
    // the attached `Solium` object, the rows it hands out and the store they
    // are kept in, the `Keyboard` singleton, and `Solium.cursor` -- so they
    // need moc, which host.cpp itself still does not.
    // `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`,
    // `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`,
    // `models::keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`,
    // `qml::pointer::tests::a_published_pointer_reaches_solium_cursor`.
    let out: PathBuf = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    let moc = find_moc(&qt);
    for header in MOC_HEADERS {
        let stem = std::path::Path::new(header)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("moc");
        let generated = out.join(format!("moc_{stem}.cpp"));
        // Qt's headers on moc's path too, so it expands `QML_ATTACHED` and the
        // other registration macros rather than stopping at them.
        // `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`.
        let includes = qt.include_paths.iter().flat_map(|path| {
            std::iter::once(path.clone())
                .chain(["QtCore", "QtGui", "QtQml", "QtQuick"].map(|module| path.join(module)))
        });
        match std::process::Command::new(&moc)
            .args(includes.map(|path| format!("-I{}", path.display())))
            .arg(header)
            .arg("-o")
            .arg(&generated)
            .status()
        {
            Ok(status) if status.success() => {
                build.file(&generated);
            }
            Ok(status) => {
                eprintln!("error: moc failed ({status}) on {header}");
                std::process::exit(1);
            }
            Err(err) => {
                eprintln!("error: could not run moc at {}: {err}", moc.display());
                eprintln!("       set QT_MOC to its path, or install Qt 6's development tools");
                std::process::exit(1);
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
            eprintln!("       Fedora:        mesa-libEGL-devel");
            eprintln!("       Arch:          mesa");
            eprintln!("       Debian/Ubuntu: libegl-dev");
            eprintln!("       openSUSE:      Mesa-libEGL-devel");
            std::process::exit(1);
        }
    }

    build.compile("solium_qml_host");
}

/// Where moc is: `QT_MOC`, then the `libexecdir` Qt6Core's pkg-config file
/// names, so moc is found the way the rest of this script finds Qt, then
/// `qt6/libexec` or `qt6/bin` beside the libraries, then the usual
/// distribution paths, then whatever `moc` is on the path. The crate builds
/// only once it is found:
/// `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`.
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
