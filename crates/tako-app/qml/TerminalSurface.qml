import QtQuick
import org.tako.terminal 1.0

Item {
    id: terminalSurface

    required property var shell
    required property var surface

    TerminalView {
        id: terminal

        anchors.fill: parent
        focus: terminalSurface.visible
        scrollbackLimit: 10000
        fontPointSize: 13.5
        cursorStyle: TerminalView.BlockCursor
        cursorBlink: true

        Component.onCompleted: {
            if (terminalSurface.visible) {
                forceActiveFocus();
            }
        }

        onVisibleChanged: {
            if (visible) {
                forceActiveFocus();
            }
        }

        onExited: terminalSurface.shell.showPassiveNotification(qsTr("Terminal exited"))
    }
}
