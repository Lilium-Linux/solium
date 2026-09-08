// The scene the GPU pre-flight draws, once, at startup.
//
// `SOLIUM_QML_GPU` commits the whole process to Qt's OpenGL scene graph, and
// that commitment is one-way: Qt fixes its backend inside `QGuiApplication` and
// there is no software path left afterwards. So the compositor renders this
// before anything on screen depends on the answer — see `qml::start_on_gpu`. If
// this reaches the buffer, the allocation, the dmabuf import, the render and
// the fence all work on this machine.
//
// Deliberately the dullest QML in the tree. No imports beyond QtQuick, no
// theme, no fonts, no images, nothing that could fail for a reason of its own
// and make a working GPU path look broken. Two rectangles: one filling the
// buffer, one covering a quarter of it, so a frame that arrives rotated,
// mirrored or half-written is not a uniform colour and cannot be mistaken for a
// clean one.
//
// No anchors: the host sets the root item's width and height itself, and an
// item that also anchors to its parent fights that.

import QtQuick

Rectangle {
    color: "#204060"

    Rectangle {
        width: parent.width / 2
        height: parent.height / 2
        color: "#c04020"
    }
}
