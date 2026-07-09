import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Controls.Control {
    id: tabBarShell

    required property var shell
    required property var workspace

    readonly property string selectedSurfaceId: tabBarShell.workspace.selectedSurfaceId
        || (tabBarShell.workspace.surfaces.length > 0 ? tabBarShell.workspace.surfaces[0].id : "")

    contentItem: RowLayout {
        spacing: 0

        Repeater {
            model: tabBarShell.workspace.surfaces

            Controls.ItemDelegate {
                id: tabButton

                required property var modelData

                Layout.preferredHeight: Kirigami.Units.gridUnit * 2
                Layout.maximumWidth: Kirigami.Units.gridUnit * 12
                padding: Kirigami.Units.smallSpacing
                highlighted: modelData.id === tabBarShell.selectedSurfaceId
                onClicked: tabBarShell.shell.selectSurface(tabBarShell.workspace.id, modelData.id)

                contentItem: RowLayout {
                    spacing: Kirigami.Units.smallSpacing

                    Controls.Label {
                        Layout.fillWidth: true
                        text: tabButton.modelData.title
                        elide: Text.ElideRight
                        font.bold: tabButton.highlighted
                    }

                    Controls.ToolButton {
                        id: closeButton

                        Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium
                        Layout.preferredHeight: Kirigami.Units.iconSizes.smallMedium
                        flat: true
                        focusPolicy: Qt.NoFocus
                        icon.name: "window-close"
                        Accessible.name: qsTr("Close Tab")
                        onClicked: tabBarShell.shell.closeSurface(tabBarShell.workspace.id, tabButton.modelData.id)

                        // Keep the parent ItemDelegate from also selecting when closing.
                        onPressed: closeButton.forceActiveFocus()
                    }
                }
            }
        }

        Item {
            Layout.fillWidth: true
        }
    }

    background: Rectangle {
        color: Kirigami.Theme.backgroundColor
    }
}
