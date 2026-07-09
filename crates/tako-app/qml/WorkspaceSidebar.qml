import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Controls.Pane {
    id: sidebar

    required property var shell
    implicitWidth: Math.max(implicitContentWidth, Kirigami.Units.gridUnit * 14)

    padding: 0

    function workspaceIcon(workspace) {
        if (!workspace || !workspace.surfaces || workspace.surfaces.length === 0) {
            return "utilities-terminal";
        }
        switch (workspace.surfaces[0].panel) {
        case "browser":
            return "internet-web-browser";
        case "file":
            return "text-x-generic";
        default:
            return "utilities-terminal";
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Kirigami.InlineViewHeader {
            Layout.fillWidth: true
            text: qsTr("Workspaces")
        }

        Controls.ScrollView {
            id: sidebarList

            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width
                spacing: 0

                Repeater {
                    model: sidebar.shell.workspaces

                    Controls.ItemDelegate {
                        required property var modelData

                        Layout.fillWidth: true
                        text: modelData.title
                        icon.name: sidebar.workspaceIcon(modelData)
                        highlighted: sidebar.shell.selectedWorkspaceId === modelData.id
                        onClicked: sidebar.shell.selectWorkspace(modelData.id)
                    }
                }
            }
        }
    }
}
