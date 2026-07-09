import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

import org.tako

Kirigami.ApplicationWindow {
    id: root

    width: Kirigami.Units.gridUnit * 66
    height: Kirigami.Units.gridUnit * 40
    minimumWidth: Kirigami.Units.gridUnit * 42
    minimumHeight: Kirigami.Units.gridUnit * 27
    visible: true
    title: selectedWorkspace ? selectedWorkspace.title + qsTr(" - Tako") : qsTr("Tako")

    property string selectedWorkspaceId: "ws-shell"
    property var selectedSurfaceByWorkspace: ({
        "ws-shell": "surface-shell-terminal"
    })

    readonly property var takoProject: ({
        id: "project-tako",
        title: qsTr("tako"),
        workspaces: [
            {
                id: "ws-shell",
                title: qsTr("Kirigami App Shell"),
                surfaces: [
                    {
                        id: "surface-shell-terminal",
                        title: qsTr("Terminal"),
                        panel: "terminal"
                    },
                    {
                        id: "surface-shell-main",
                        title: qsTr("main.qml"),
                        panel: "file"
                    },
                    {
                        id: "surface-shell-notes",
                        title: qsTr("Notes"),
                        panel: "file"
                    }
                ]
            },
            {
                id: "ws-terminal",
                title: qsTr("Terminal Polish"),
                surfaces: [
                    {
                        id: "surface-terminal-core",
                        title: qsTr("Terminal"),
                        panel: "terminal"
                    }
                ]
            },
            {
                id: "ws-persistence",
                title: qsTr("Session Persistence"),
                surfaces: [
                    {
                        id: "surface-persistence-notes",
                        title: qsTr("Notes"),
                        panel: "file"
                    }
                ]
            }
        ]
    })

    readonly property var kdeProject: ({
        id: "project-kde",
        title: qsTr("kde-experiments"),
        workspaces: [
            {
                id: "ws-hig-layout",
                title: qsTr("Layout and Navigation"),
                surfaces: [
                    {
                        id: "surface-hig-layout",
                        title: qsTr("Layout"),
                        panel: "browser"
                    }
                ]
            },
            {
                id: "ws-hig-content",
                title: qsTr("Displaying Content"),
                surfaces: [
                    {
                        id: "surface-hig-content",
                        title: qsTr("Content"),
                        panel: "browser"
                    }
                ]
            },
            {
                id: "ws-hig-status",
                title: qsTr("Status Patterns"),
                surfaces: [
                    {
                        id: "surface-hig-status",
                        title: qsTr("Status"),
                        panel: "browser"
                    }
                ]
            }
        ]
    })

    readonly property var freeWorkspaces: [
        {
            id: "ws-scratch",
            title: qsTr("Scratch Shell"),
            surfaces: [
                {
                    id: "surface-scratch-terminal",
                    title: qsTr("Shell"),
                    panel: "terminal"
                }
            ]
        },
        {
            id: "ws-remote",
            title: qsTr("Remote Spike"),
            surfaces: [
                {
                    id: "surface-remote-notes",
                    title: qsTr("Notes"),
                    panel: "file"
                }
            ]
        }
    ]

    readonly property var projects: [takoProject, kdeProject]
    readonly property var selectedWorkspace: workspaceById(selectedWorkspaceId)

    function allWorkspaces() {
        let result = [];
        for (const project of projects) {
            result = result.concat(project.workspaces);
        }
        return result.concat(freeWorkspaces);
    }

    function workspaceById(id) {
        for (const workspace of allWorkspaces()) {
            if (workspace.id === id) {
                return workspace;
            }
        }
        return takoProject.workspaces[0];
    }

    function selectWorkspace(id) {
        selectedWorkspaceId = id;
    }

    function selectedSurfaceId(workspaceId, fallbackId) {
        return selectedSurfaceByWorkspace[workspaceId] || fallbackId;
    }

    function selectSurface(workspaceId, surfaceId) {
        const next = {};
        for (const key in selectedSurfaceByWorkspace) {
            next[key] = selectedSurfaceByWorkspace[key];
        }
        next[workspaceId] = surfaceId;
        selectedSurfaceByWorkspace = next;
    }

    function actionMessage(label) {
        showPassiveNotification(qsTr("%1 is not wired to the model yet.").arg(label));
    }

    pageStack.initialPage: Kirigami.Page {
        title: root.selectedWorkspace ? root.selectedWorkspace.title : qsTr("Tako")
        padding: 0
        globalToolBarStyle: Kirigami.ApplicationHeaderStyle.ToolBar
        actions: [
            Kirigami.Action {
                text: qsTr("New Workspace")
                icon.name: "list-add"
                onTriggered: root.actionMessage(text)
            },
            Kirigami.Action {
                text: qsTr("New Terminal")
                icon.name: "utilities-terminal"
                onTriggered: root.actionMessage(text)
            },
            Kirigami.Action {
                text: qsTr("Rename Workspace")
                icon.name: "edit-rename"
                onTriggered: root.actionMessage(text)
            },
            Kirigami.Action {
                text: qsTr("Close Workspace")
                icon.name: "window-close"
                onTriggered: root.actionMessage(text)
            }
        ]

        RowLayout {
            anchors.fill: parent
            spacing: 0

            WorkspaceSidebar {
                Layout.preferredWidth: implicitWidth
                Layout.fillHeight: true
                shell: root
            }

            Kirigami.Separator {
                Layout.fillHeight: true
            }

            WorkspaceView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                shell: root
                workspace: root.selectedWorkspace
            }
        }
    }
}
