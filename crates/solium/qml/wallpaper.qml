// The wallpaper.
//
// Drawn by the compositor rather than by a client, which is a choice and not
// the obvious one. Every other Wayland desktop leaves this to a program on the
// `background` layer — `swaybg`, `hyprpaper`, a shell — and Solium supports
// that too: a layer surface anchored to all four edges still lands underneath
// everything and still wins over this.
//
// It is here anyway because of what a compositor looks like with nothing
// running. A desktop whose first frame is flat grey until a helper starts is
// one where every crash, every misconfiguration and every first boot looks the
// same as a broken session. This is the picture that is there before anything
// else is, and on a machine with no shell installed it is the picture that
// stays.
//
// Everything below is ordinary QML, so it is `~/.config/solium/qml/wallpaper.qml`
// away from being anything else: a gradient, a video, a clock, a shader.
// `super+shift+r` reloads it while the session runs.

import QtQuick

Item {
    id: paper

    // The image to show, from `config.wallpaper` by way of `lua/wallpaper.lua`.
    // A relative path resolves against this file, which is how the shipped
    // image is found; an absolute one is taken as given. Empty means the
    // colour below and nothing else, which is also where a path that will not
    // load ends up.
    required property string source

    // Underneath the image, always, and not merely when there is no image.
    // An image that has not finished loading, one with transparency in it, or
    // one whose aspect leaves a sliver at an edge would otherwise show
    // whatever the renderer last had in that buffer — and on a first frame
    // that is undefined memory rather than a colour.
    Rectangle {
        anchors.fill: parent
        color: "#0d0d0f"
    }

    Image {
        anchors.fill: parent
        source: paper.source
        // Cover the screen and crop, rather than fit and letterbox. A
        // wallpaper with bars down the side reads as a broken wallpaper.
        fillMode: Image.PreserveAspectCrop
        // The scene is rasterised at device pixels and this is already the
        // size it will be drawn at, so let Qt do the filtering once here
        // rather than the GPU doing it every frame.
        smooth: true
        mipmap: true
        cache: false
        asynchronous: false

        onStatusChanged: {
            if (status === Image.Error) {
                // Says which file, because "the wallpaper is black" has one
                // cause worth distinguishing from all the others.
                console.warn("wallpaper: could not load " + paper.source)
            }
        }
    }
}
