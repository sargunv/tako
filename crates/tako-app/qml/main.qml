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
    title: qsTr("Tako — Phase 1 P2 colored render")
    color: palette.window

    // The live libghostty-vt terminal. Click to focus, then type.
    TerminalView {
        id: term
        anchors.fill: parent
        focus: true
        Component.onCompleted: term.forceActiveFocus()
    }

    Label {
        text: qsTr("Phase 1 P2: per-cell color (fg+bg+inverse/faint). Resize/hidpi next.")
        color: palette.text
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.margins: 6
    }
}

