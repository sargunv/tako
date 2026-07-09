import QtQuick
import org.kde.kirigami as Kirigami

Item {
    id: placeholder

    required property var shell
    required property var surface

    Kirigami.PlaceholderMessage {
        anchors.centerIn: parent
        width: Math.min(parent.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 28)
        icon.name: placeholder.surface.panel === "browser" ? "internet-web-browser" : "text-x-generic"
        text: placeholder.surface.title
        explanation: placeholder.surface.panel === "browser"
            ? qsTr("Browser surfaces come later. This tab is a placeholder.")
            : qsTr("File surfaces come later. This tab is a placeholder.")
    }
}
