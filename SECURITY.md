# Security policy

Solium is a compositor: it holds the keyboard, the screen and the lock screen
for a whole session. A bug that lets a client see what it should not, or gets
past the lock, matters more here than in most programs, so please report it
privately.

## Supported versions

| Version | Supported |
|---|---|
| The latest release | yes |
| `stage`, the development trunk | yes |
| Anything older | no — upgrade to the latest release |

Fixes land on `stage` first and reach users in the next release.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting: open the repository's
**Security** tab and choose **Report a vulnerability**, or go straight to
<https://github.com/Lilium-Linux/solium/security/advisories/new>. The report is
visible only to you and the maintainers.

**Please do not open a public issue or pull request for a vulnerability**, and
do not describe it anywhere public until it is fixed.

A useful report says:

- what an attacker can do, and from where (an ordinary Wayland client, an X11
  client through XWayland, someone at the keyboard of a locked session);
- the Solium commit or release, and whether it ran nested or on a TTY;
- how to reproduce it, as a program or a script if there is one;
- the relevant part of `~/.local/state/solium/session.log`.

Examples of what counts: getting input or pixels past the lock screen, the
session unlocking without the locking client's say-so, one client reading
another's input or contents without a protocol that grants it, and anything
that runs code as the user from outside the session.

## What happens next

- You get an acknowledgement, and the report is confirmed or questioned, in
  the advisory's private thread.
- The fix is developed privately where that is possible, and merged to `stage`.
- **The vulnerability is disclosed with the fix**: the advisory is published
  when the fix is merged, not before and not long after, and it credits you
  unless you ask otherwise.
