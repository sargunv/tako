import QtQuick

Item {
    id: content

    required property var shell
    required property var workspace
    required property var surface

    Loader {
        anchors.fill: parent
        sourceComponent: surface ? (surface.panel === "terminal" ? terminalComponent : placeholderComponent) : null
    }

    Component {
        id: terminalComponent

        TerminalSurface {
            shell: content.shell
            surface: content.surface
        }
    }

    Component {
        id: placeholderComponent

        PlaceholderSurface {
            shell: content.shell
            surface: content.surface
        }
    }
}
