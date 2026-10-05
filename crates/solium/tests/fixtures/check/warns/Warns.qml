// A scene that builds, but warns while it does: a binding to a name that does
// not exist. See `check::tests::a_scene_that_warns_while_it_is_built_fails`.

import QtQuick

Item {
    width: nothingIsCalledThis.width
}
