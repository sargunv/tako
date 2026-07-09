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

        Controls.TabBar {
            Layout.fillWidth: true
            currentIndex: {
                const surfaces = tabBarShell.workspace.surfaces;
                for (let i = 0; i < surfaces.length; ++i) {
                    if (surfaces[i].id === tabBarShell.selectedSurfaceId) {
                        return i;
                    }
                }
                return 0;
            }

            Repeater {
                model: tabBarShell.workspace.surfaces

                Controls.TabButton {
                    id: tabButton

                    required property var modelData

                    text: modelData.title
                    checked: modelData.id === tabBarShell.selectedSurfaceId
                    leftPadding: Kirigami.Units.mediumSpacing
                    rightPadding: Kirigami.Units.smallSpacing
                    topPadding: Kirigami.Units.smallSpacing
                    bottomPadding: Kirigami.Units.smallSpacing
                    onClicked: tabBarShell.shell.selectSurface(tabBarShell.workspace.id, modelData.id)

                    contentItem: RowLayout {
                        spacing: Kirigami.Units.smallSpacing

                        Controls.Label {
                            Layout.fillWidth: true
                            text: tabButton.text
                            elide: Text.ElideRight
                        }

                        Item {
                            id: closeButton

                            Layout.alignment: Qt.AlignVCenter
                            Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium
                            Layout.preferredHeight: Kirigami.Units.iconSizes.smallMedium
                            Accessible.role: Accessible.Button
                            Accessible.name: qsTr("Close Tab")

                            Kirigami.Icon {
                                anchors.centerIn: parent
                                width: Kirigami.Units.iconSizes.small
                                height: Kirigami.Units.iconSizes.small
                                source: "window-close"
                                color: closeMouse.containsMouse ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
                                opacity: closeMouse.containsMouse || tabButton.checked ? 1 : 0.7
                            }

                            MouseArea {
                                id: closeMouse

                                anchors.fill: parent
                                hoverEnabled: true
                                onClicked: tabBarShell.shell.closeSurface(tabBarShell.workspace.id, tabButton.modelData.id)
                            }
                        }
                    }
                }
            }
        }

        Item {
            Layout.fillWidth: true
        }
    }
}
