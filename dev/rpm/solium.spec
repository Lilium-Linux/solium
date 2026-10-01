# Solium as a Fedora package (#66): a development snapshot of one commit.
#
# dev/rpm.sh builds it: the release binary in the build container, then this
# spec on the host `--with prebuilt`, which packages that binary. Without it,
# %build compiles the source tarball, as COPR will. Either way %install is
# dev/install.sh with a system prefix, so the package holds exactly what that
# installs: dev/install-check.sh's "the Fedora package" compares %files with
# it both ways, and rpmbuild itself fails on a file one has and the other not.

%bcond prebuilt 0

# dev/rpm.sh defines both from the commit it packages. dev/install-check.sh
# asserts the spec refuses to parse without them.
%{!?commit:%{error:define commit, the short hash of the commit packaged (dev/rpm.sh does)}}
%{!?commitdate:%{error:define commitdate, that commit's date as YYYYMMDD (dev/rpm.sh does)}}

# The prebuilt binary, from the release profile, has no debug information to
# put in a debuginfo package, and rpmbuild fails on an empty one: dev/rpm.sh
# builds this.
%global debug_package %{nil}
# No %changelog: a snapshot's history is its commit's. rpmbuild would warn that
# it has no date to take from one, and dev/rpm.sh repeats any warning.
%global source_date_epoch_from_changelog 0

Name:           solium
Version:        0.0.0~git%{commitdate}.%{commit}
Release:        1%{?dist}
Summary:        Wayland compositor of Lilium DE, configured in Lua, with QML chrome
# GPL-3.0-only for the code, CC-BY-SA-4.0 for the wallpaper: dev/install-check.sh
# asserts every .license file installed names a licence listed here.
License:        GPL-3.0-only AND CC-BY-SA-4.0
URL:            https://github.com/Lilium-Linux/solium
Source0:        solium-%{version}.tar.gz
%if %{with prebuilt}
Source1:        solium
%endif

BuildRequires:  systemd-rpm-macros
%if %{without prebuilt}
BuildRequires:  cargo >= 1.88
BuildRequires:  rust >= 1.88
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  make
BuildRequires:  pkgconf-pkg-config
BuildRequires:  wayland-devel
BuildRequires:  libinput-devel
BuildRequires:  systemd-devel
BuildRequires:  libseat-devel
BuildRequires:  libxkbcommon-devel
BuildRequires:  mesa-libgbm-devel
BuildRequires:  mesa-libEGL-devel
BuildRequires:  libdrm-devel
BuildRequires:  qt6-qtbase-devel
BuildRequires:  qt6-qtdeclarative-devel
%endif

# rpm finds the libraries the binary links. These it cannot see, and
# dev/install-check.sh's "the Fedora package" asserts each is the package that
# provides it: the QML modules the shipped QML imports, Xwayland, and the
# flock solium-session takes its lock with. Qt's symbols are all versioned
# Qt_6, so the Qt the binary was built against, or newer, is spelt out here:
# dev/rpm.sh defines _qt6_version as the build image's, and a source build has
# it from qt6-qtbase-devel. dev/install-check.sh asserts the version clause.
Requires:       qt6-qtdeclarative%{?_isa}%{?_qt6_version: >= %{_qt6_version}}
Requires:       xorg-x11-server-Xwayland
Requires:       util-linux-core
# What reads lilium-portals.conf, and each backend it names: the same check.
Recommends:     xdg-desktop-portal
Recommends:     xdg-desktop-portal-gtk
Recommends:     xdg-desktop-portal-wlr
# A terminal from init.lua's list, which super+return opens. The shipped
# configuration has no launcher, and Fedora Workstation's ptyxis is not in the
# list, so without one a session there can start no program: the same check.
Recommends:     foot

%description
Solium is the Wayland compositor of Lilium DE, written in Rust on Smithay and
configured in Lua. Its window frames, its wallpaper and the desktop shell it
hosts are QML running inside the compositor.

This is a development snapshot of one commit, built for one Fedora release,
for starting Solium from the login screen or a text console.

%prep
%autosetup
%if %{with prebuilt}
install -Dm755 %{SOURCE1} target/install/release/solium
%endif

%build
%if %{without prebuilt}
export SOLIUM_DATADIR=%{_datadir}/solium
export CARGO_TARGET_DIR=target/install
cargo build --locked --release -p solium
%endif

%install
%if %{with prebuilt}
DESTDIR=%{buildroot} dev/install.sh --no-build --prefix %{_prefix}
%else
DESTDIR=%{buildroot} dev/install.sh --no-build --no-check --prefix %{_prefix}
%endif

%if %{without prebuilt}
%check
mkdir -p check-config
XDG_CONFIG_HOME="$PWD/check-config" target/install/release/solium --check
%endif

%files
%license LICENSE LICENSES/CC-BY-SA-4.0.txt
%doc README.md THIRD_PARTY.md
%{_bindir}/solium
%{_bindir}/solium-session
%{_datadir}/solium/
%{_datadir}/wayland-sessions/solium.desktop
%{_userunitdir}/solium-session.target
%{_userunitdir}/solium-autostart.target
%dir %{_datadir}/xdg-desktop-portal
%{_datadir}/xdg-desktop-portal/lilium-portals.conf
