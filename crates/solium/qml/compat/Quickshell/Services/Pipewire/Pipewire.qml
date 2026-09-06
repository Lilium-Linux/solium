// Audio devices and streams.
//
// A shim with the shape but not the substance: the shell reads volumes and
// mute state from here, and until Solium has an audio service to answer with,
// a silent default is better than a module that will not load. This is the
// single missing module that was failing seventy-two files.
pragma Singleton
import QtQuick

QtObject {
    property var nodes: ({ values: [] })
    property var links: ({ values: [] })
    property var defaultAudioSink: null
    property var defaultAudioSource: null
    property bool ready: false
}
