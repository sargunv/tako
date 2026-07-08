import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window

import org.tako
import org.tako.terminal 1.0

ApplicationWindow {
    id: root
    width: 900
    height: 480
    visible: true
    title: qsTr("Tako")
    color: palette.window

    // The live libghostty-vt terminal. Click to focus, then type.
    TerminalView {
        id: term
        anchors.fill: parent
        focus: true
        Component.onCompleted: term.forceActiveFocus()
    }
}
