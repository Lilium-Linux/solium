# Public preview criteria

The planned v0.1.0 is an alpha public preview. It should let someone install
Solium, use its core desktop workflows and report useful bugs. It is not a
promise of production stability or complete phone/tablet support.

The [project board](https://github.com/orgs/Lilium-Linux/projects/2) is the
source of work status. This page defines what release readiness means;
[known limitations](gaps.md) explains compatibility and the
[roadmap](roadmap.md) covers work beyond the preview.

## What already exists

The window-management modes, QML frames and scene hosting, reloadable
configuration and user effect folders are implemented. The preview shell has a
bottom bar, top dock, search and desktop icons. Checkout installation and
Fedora RPM build tooling exist. Logind lock/sleep handling and per-device
input settings are implemented too.

These capabilities still need testing as one installed desktop. Their presence
in the repository is not release acceptance.

## Requirements before release

| Area | Evidence required |
|---|---|
| Daily use | Core application and window-management workflows survive real use; remaining release blockers in the daily-drive queue are resolved |
| Desktop | The preview shell works with real services and data, including the intended widgets and native lock UI |
| Hardware | Recorded long-session and suspend/resume checks, plus input, output hotplug, scaling and GPU behavior on the supported machines |
| Installation | A fresh system can install, select the session, upgrade and remove the release using its documented packages |
| Session services | Locking, idle, autostart, authentication and portal workflows are exercised in the installed session |
| Configuration | Defaults work, reload preserves the session, and errors give useful diagnostics |
| Quality checks | The required gate and CI pass; docs generate and all internal site links resolve |
| Documentation | Setup, recovery and known limitations match the release revision |

No date is promised. The readiness decision belongs to the maintainer after
the evidence is available; this page does not replace the issue queue or
create a second fixed priority order.

## Limits to name in the preview

Missing IME/on-screen-keyboard support, compositor touch/gestures, external
shell protocols and unfinished graphics compatibility must be visible before
installation. The release notes should state which machines, drivers and
application workflows were actually checked. Do not infer support for another
GPU or device class from a nested run.

Some gaps may remain documented limitations; others may block release when
real use shows their impact. Track that decision on the corresponding issue
rather than leaving a closed issue listed here as unfinished work.

## How to help establish readiness

Follow [Contributing](../CONTRIBUTING.md) and the
[development guide](../dev/README.md) for nested checks and hardware test
instructions. Include the revision, configuration, device/driver information,
reproduction steps and session log in a bug report. A hardware run needs a
person at the console and a way back to the existing desktop.
