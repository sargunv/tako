import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Item {
    id: workspaceView

    required property var shell
    property var workspace

    readonly property string selectedSurfaceId: workspace
        ? (workspace.selectedSurfaceId || (workspace.surfaces.length > 0 ? workspace.surfaces[0].id : ""))
        : ""

    Kirigami.PlaceholderMessage {
        anchors.centerIn: parent
        width: Math.min(parent.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 28)
        visible: !workspaceView.workspace
        icon.name: "utilities-terminal"
        text: qsTr("No workspace selected")
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0
        visible: !!workspaceView.workspace

        SurfaceTabBar {
            Layout.fillWidth: true
            visible: workspaceView.workspace && workspaceView.workspace.surfaces.length > 1
            Layout.preferredHeight: visible ? implicitHeight : 0
            shell: workspaceView.shell
            workspace: workspaceView.workspace
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true

            Repeater {
                model: workspaceView.workspace ? workspaceView.workspace.surfaces : []

                Item {
                    id: surfaceDelegate

                    required property var modelData

                    anchors.fill: parent
                    visible: modelData.id === workspaceView.selectedSurfaceId

                    SurfaceContent {
                        anchors.fill: parent
                        shell: workspaceView.shell
                        workspace: workspaceView.workspace
                        surface: surfaceDelegate.modelData
                    }
                }
            }
        }
    }
}
