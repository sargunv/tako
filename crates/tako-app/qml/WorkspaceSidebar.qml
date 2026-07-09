import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Controls.Pane {
    id: sidebar

    required property var shell
    implicitWidth: Math.max(implicitContentWidth, Kirigami.Units.gridUnit * 14)

    property var expandedProjects: ({
        "project-tako": true,
        "project-kde": true
    })

    padding: 0

    function workspaceIcon(workspace) {
        if (!workspace || workspace.surfaces.length === 0) {
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

    function setExpanded(projectId, expanded) {
        const next = {};
        for (const key in expandedProjects) {
            next[key] = expandedProjects[key];
        }
        next[projectId] = expanded;
        expandedProjects = next;
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
                    model: sidebar.shell.projects

                    ColumnLayout {
                        id: projectGroup

                        required property var modelData
                        readonly property var project: modelData

                        Layout.fillWidth: true
                        spacing: 0
                        visible: project.workspaces.length > 0

                        Controls.ItemDelegate {
                            Layout.fillWidth: true
                            text: projectGroup.project.title
                            icon.name: sidebar.expandedProjects[projectGroup.project.id] ? "arrow-down" : "arrow-right"
                            onClicked: sidebar.setExpanded(projectGroup.project.id, !sidebar.expandedProjects[projectGroup.project.id])
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 0
                            visible: sidebar.expandedProjects[projectGroup.project.id]

                            Repeater {
                                model: projectGroup.project.workspaces

                                Controls.ItemDelegate {
                                    required property var modelData

                                    Layout.fillWidth: true
                                    leftPadding: Kirigami.Units.largeSpacing + Kirigami.Units.iconSizes.smallMedium
                                    text: modelData.title
                                    icon.name: sidebar.workspaceIcon(modelData)
                                    highlighted: sidebar.shell.selectedWorkspaceId === modelData.id
                                    onClicked: sidebar.shell.selectWorkspace(modelData.id)
                                }
                            }
                        }
                    }
                }

                Repeater {
                    model: sidebar.shell.freeWorkspaces

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
