# Where it stands

Solium is alpha. It runs on hardware and is being readied for daily use, but
the trial on real hardware that decides whether it is ready has not happened
yet, and it is not yet anyone's daily desktop. What is being worked on, and in
what order, lives on GitHub rather than in these pages, because it changes
daily:

- The [`daily-drive` label](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20state%3Aopen%20label%3Adaily-drive)
  is the list of what matters most: the issues that stop an application being
  used at all, first.
- The [project board](https://github.com/orgs/Lilium-Linux/projects/2) shows
  which of them are being worked on, waiting for review, or done.

It has been checked on the hardware at commit `dfc95ce`, on a desktop with an
NVIDIA RTX 3070 and on a Microsoft Surface Pro 7 (Intel Ice Lake), both on
Fedora 44. QML on the GPU, animations, the screens going off when idle,
`swaylock`, and the Caps Lock and layout pill all work there. On the Surface
Pro 7 the screen also went off when the lid closed, though Solium has no lid
handling of its own. Touch reaches applications, but not what Solium draws
([#181](https://github.com/Lilium-Linux/solium/issues/181)).

The first release, v0.1.0, will be a preview release, cut only once the
desktop is daily-drivable (the `daily-drive` label), the native preview shell
is written (a bar at the bottom, a dock at the top, quick search, desktop
icons, widgets with real data and a native lock screen), and there are
packages: the point where it is ready for an open beta. [The road to a public
preview](beta.md) is that road.

The pages in this section are the longer view:

- [Roadmap](roadmap.md): the epics, in the order they depend on each other.
- [Everything not built yet](gaps.md): the whole list of what is missing.
- [The road to a public preview](beta.md): what has to be true first.
