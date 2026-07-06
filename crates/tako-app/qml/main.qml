import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window

import org.tako

ApplicationWindow {
    id: root
    width: 480
    height: 240
    visible: true
    title: qsTr("Tako — Phase 0 spike")
    color: palette.window

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 16
        spacing: 12

        Label {
            text: qsTr("Who to greet?")
            color: palette.text
        }

        TextField {
            id: nameField
            Layout.fillWidth: true
            placeholderText: qsTr("name")
            text: qsTr("world")
            onAccepted: greetButton.clicked()
        }

        Button {
            id: greetButton
            text: qsTr("Greet")
            onClicked: root.greeting.greet(nameField.text)
        }

        Label {
            text: root.greeting.message
            color: palette.text
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
    }

    readonly property Greeting greeting: Greeting {
        message: qsTr("Hello! — from Rust")
    }
}
