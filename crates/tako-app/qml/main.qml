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
    title: term.title.length > 0 ? term.title : qsTr("Tako")
    color: palette.window

    TerminalView {
        id: term
        anchors.fill: parent
        focus: true
        scrollbackLimit: 10000
        fontPointSize: 13.5
        cursorStyle: TerminalView.BlockCursor
        cursorBlink: false
        Component.onCompleted: term.forceActiveFocus()
        onExited: Qt.quit()
    }
}
