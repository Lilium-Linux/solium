// A scene that reads the monitor it is on: the check publishes one, so this
// passes. See `check::tests::a_scene_that_reads_its_monitor_passes`.

import QtQuick
import Solium

Text { text: Solium.monitor.name }
