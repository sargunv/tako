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

    property string configFontFamily: ""
    property real configFontPointSize: 13.5
    property string configForegroundColor: ""
    property string configBackgroundColor: ""
    property string configCursorColor: ""
    property var configColorPalette: []
    property int configCursorStyle: TerminalView.BlockCursor
    property bool configCursorBlink: false
    property var configScrollbackLimit: 10000

    TerminalView {
        id: term
        anchors.fill: parent
        focus: true
        scrollbackLimit: root.configScrollbackLimit
        fontFamily: root.configFontFamily
        fontPointSize: root.configFontPointSize
        foregroundColor: root.configForegroundColor
        backgroundColor: root.configBackgroundColor
        cursorColor: root.configCursorColor
        colorPalette: root.configColorPalette
        cursorStyle: root.configCursorStyle
        cursorBlink: root.configCursorBlink
        Component.onCompleted: term.forceActiveFocus()
        onExited: Qt.quit()
    }
}
