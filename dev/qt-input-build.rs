// Both the compositor and wirecheck compile host.cpp. Touch delivery updates
// Qt's persistent event points, which requires headers matching QtBase itself.
pub(crate) fn include_private_input_headers(build: &mut cc::Build) {
    let base = match pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("Qt6Gui")
    {
        Ok(base) => base,
        Err(error) => {
            eprintln!("error: Qt6Gui development files: {error}");
            std::process::exit(1);
        }
    };
    for (module, header) in [("QtCore", "qobject_p.h"), ("QtGui", "qpointingdevice_p.h")] {
        let mut found = false;
        for root in &base.include_paths {
            let versioned = root.join(module).join(&base.version);
            let includes = versioned.join(module);
            if includes.join("private").join(header).is_file() {
                build.include(&versioned).include(&includes);
                found = true;
            }
        }
        if !found {
            eprintln!(
                "error: {module} private input headers for Qt {} are missing; \
                 install QtBase's matching private development package \
                 (Fedora: qt6-qtbase-private-devel; Debian/Ubuntu: qt6-base-private-dev). \
                 For an existing build image, rebuild it from dev/Containerfile.",
                base.version
            );
            std::process::exit(1);
        }
    }
}
