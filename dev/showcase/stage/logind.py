#!/usr/bin/env python3
"""A stand-in for logind on the showcase's private D-Bus, so a capture can
end Solium's own lock the way `loginctl unlock-session` does, with no password.

It owns `org.freedesktop.login1` on the bus it is given, answers what
`crates/solium/src/logind.rs` asks (`GetSession`, `GetSessionByPID`,
`Inhibit`, `InhibitDelayMaxUSec`, `SetLockedHint`), and sends the session's
`Lock` and `Unlock` signals at the moments it is told, in seconds after it
starts. Solium hears signals only from the name's owner, so these are believed
exactly as logind's would be, and nothing reaches the real logind: Solium is
pointed at this bus with `SOLIUM_LOGIND_BUS`.

    logind.py --bus <address> [--lock 6.5] [--unlock 9.0 ...]
"""

import argparse
import os
import sys

import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

NAME = "org.freedesktop.login1"
MANAGER_PATH = "/org/freedesktop/login1"
SESSION_PATH = "/org/freedesktop/login1/session/showcase"
MANAGER = "org.freedesktop.login1.Manager"
SESSION = "org.freedesktop.login1.Session"


class Manager(dbus.service.Object):
    """Every session asked for is the one session, and sleep is never held."""

    def __init__(self, bus):
        super().__init__(bus, MANAGER_PATH)
        # The read ends of the inhibitors handed out, so they stay open until
        # Solium closes its own end, as logind's do.
        self.held = []

    @dbus.service.method(MANAGER, in_signature="s", out_signature="o")
    def GetSession(self, _id):
        return dbus.ObjectPath(SESSION_PATH)

    @dbus.service.method(MANAGER, in_signature="u", out_signature="o")
    def GetSessionByPID(self, _pid):
        return dbus.ObjectPath(SESSION_PATH)

    @dbus.service.method(MANAGER, in_signature="ssss", out_signature="h")
    def Inhibit(self, _what, _who, _why, _mode):
        read, write = os.pipe()
        self.held.append(read)
        return dbus.types.UnixFd(write)

    @dbus.service.method(dbus.PROPERTIES_IFACE, in_signature="ss", out_signature="v")
    def Get(self, interface, prop):
        if interface == MANAGER and prop == "InhibitDelayMaxUSec":
            return dbus.UInt64(5_000_000)
        raise dbus.exceptions.DBusException(
            f"no property {interface}.{prop}",
            name="org.freedesktop.DBus.Error.UnknownProperty",
        )


class Session(dbus.service.Object):
    def __init__(self, bus):
        super().__init__(bus, SESSION_PATH)

    @dbus.service.method(SESSION, in_signature="b", out_signature="")
    def SetLockedHint(self, locked):
        print(f"logind: locked hint {bool(locked)}", file=sys.stderr, flush=True)

    @dbus.service.signal(SESSION, signature="")
    def Lock(self):
        print("logind: Lock", file=sys.stderr, flush=True)

    @dbus.service.signal(SESSION, signature="")
    def Unlock(self):
        print("logind: Unlock", file=sys.stderr, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--bus", required=True, help="the private bus's address")
    parser.add_argument("--lock", type=float, action="append", default=[])
    parser.add_argument("--unlock", type=float, action="append", default=[])
    args = parser.parse_args()

    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.bus.BusConnection(args.bus)
    _name = dbus.service.BusName(NAME, bus)
    _manager = Manager(bus)
    session = Session(bus)

    def once(signal):
        signal()
        return False

    for at in args.lock:
        GLib.timeout_add(int(at * 1000), once, session.Lock)
    for at in args.unlock:
        GLib.timeout_add(int(at * 1000), once, session.Unlock)
    print("logind: ready", file=sys.stderr, flush=True)
    GLib.MainLoop().run()


if __name__ == "__main__":
    main()
