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

    AppShell {
        id: appShell
    }

    readonly property var workspaces: appShell.workspaces || []
    readonly property string selectedWorkspaceId: appShell.selectedWorkspaceId
    readonly property var selectedWorkspace: workspaceById(selectedWorkspaceId)

    function workspaceById(id) {
        for (const workspace of root.workspaces) {
            if (workspace.id === id) {
                return workspace;
            }
        }
        return root.workspaces.length > 0 ? root.workspaces[0] : null;
    }

    function selectWorkspace(id) {
        appShell.selectWorkspace(id);
    }

    function selectSurface(workspaceId, surfaceId) {
        appShell.selectSurface(workspaceId, surfaceId);
    }

    function createWorkspace(title) {
        appShell.createWorkspace(title || "");
    }

    function renameWorkspace(id, title) {
        appShell.renameWorkspace(id, title);
    }

    function closeWorkspace(id) {
        appShell.closeWorkspace(id);
    }

    function createTerminal(workspaceId) {
        appShell.createTerminal(workspaceId || root.selectedWorkspaceId);
    }

    function closeSurface(workspaceId, surfaceId) {
        appShell.closeSurface(workspaceId, surfaceId);
    }

    function openRenameDialog() {
        if (!root.selectedWorkspace) {
            return;
        }
        renameField.text = root.selectedWorkspace.title;
        renameDialog.open();
        renameField.forceActiveFocus();
        renameField.selectAll();
    }

    pageStack.initialPage: Kirigami.Page {
        title: root.selectedWorkspace ? root.selectedWorkspace.title : qsTr("Tako")
        padding: 0
        globalToolBarStyle: Kirigami.ApplicationHeaderStyle.ToolBar
        actions: [
            Kirigami.Action {
                text: qsTr("New Workspace")
                icon.name: "list-add"
                onTriggered: root.createWorkspace("")
            },
            Kirigami.Action {
                text: qsTr("New Terminal")
                icon.name: "utilities-terminal"
                enabled: !!root.selectedWorkspace
                onTriggered: root.createTerminal(root.selectedWorkspaceId)
            },
            Kirigami.Action {
                text: qsTr("Rename Workspace")
                icon.name: "edit-rename"
                enabled: !!root.selectedWorkspace
                onTriggered: root.openRenameDialog()
            },
            Kirigami.Action {
                text: qsTr("Close Workspace")
                icon.name: "window-close"
                enabled: !!root.selectedWorkspace
                onTriggered: root.closeWorkspace(root.selectedWorkspaceId)
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

        Kirigami.PromptDialog {
            id: renameDialog
            title: qsTr("Rename Workspace")
            standardButtons: Kirigami.Dialog.Ok | Kirigami.Dialog.Cancel
            onAccepted: {
                root.renameWorkspace(root.selectedWorkspaceId, renameField.text);
            }

            Controls.TextField {
                id: renameField
                placeholderText: qsTr("Workspace name")
                onAccepted: renameDialog.accept()
            }
        }
    }
}
