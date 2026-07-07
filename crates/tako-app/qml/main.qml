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
    title: qsTr("Tako — Phase 0 §3 terminal spike")
    color: palette.window

    // The live libghostty-vt terminal. Spawns $SHELL on a PTY and renders via
    // QSG. Input/resize arrive in Step D; for now this proves the render path.
    TerminalView {
        anchors.fill: parent
    }

    Label {
        text: qsTr("Phase 0 §3: monochrome QSG render of libghostty-vt — input in Step D")
        color: palette.text
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.margins: 6
    }
}

