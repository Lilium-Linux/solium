// A scene with a required property the configuration does not pass: it is
// never built. See `check::tests::a_surface_missing_a_required_property_fails`.

import QtQuick

Item {
    required property string label
}
