//! cxx-qt bridge: immutable workspace/surface snapshots + actions for QML.
//!
//! cxx-qt generates FFI glue (`unsafe extern`, `export_name`); keep the
//! exception scoped to this module.

#![allow(unsafe_code)]

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QVariant, workspaces, READ, NOTIFY = model_changed)]
        #[qproperty(QString, selected_workspace_id, cxx_name = "selectedWorkspaceId", READ, NOTIFY = model_changed)]
        type AppShell = super::AppShellRust;

        #[qsignal]
        #[cxx_name = "modelChanged"]
        fn model_changed(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "createWorkspace"]
        fn create_workspace(self: Pin<&mut Self>, title: &QString);

        #[qinvokable]
        #[cxx_name = "selectWorkspace"]
        fn select_workspace(self: Pin<&mut Self>, id: &QString);

        #[qinvokable]
        #[cxx_name = "renameWorkspace"]
        fn rename_workspace(self: Pin<&mut Self>, id: &QString, title: &QString);

        #[qinvokable]
        #[cxx_name = "closeWorkspace"]
        fn close_workspace(self: Pin<&mut Self>, id: &QString);

        #[qinvokable]
        #[cxx_name = "createTerminal"]
        fn create_terminal(self: Pin<&mut Self>, workspace_id: &QString);

        #[qinvokable]
        #[cxx_name = "selectSurface"]
        fn select_surface(self: Pin<&mut Self>, workspace_id: &QString, surface_id: &QString);

        #[qinvokable]
        #[cxx_name = "closeSurface"]
        fn close_surface(self: Pin<&mut Self>, workspace_id: &QString, surface_id: &QString);

        #[qinvokable]
        #[cxx_name = "selectedSurfaceId"]
        fn selected_surface_id(self: &Self, workspace_id: &QString) -> QString;
    }

    impl cxx_qt::Initialize for AppShell {}
}

use crate::model::{AppModel, Surface, Workspace};
use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QMap, QMapPair_QString_QVariant, QString, QVariant};

/// QML-facing shell object. Owns the durable model and publishes snapshots.
pub struct AppShellRust {
    model: AppModel,
    workspaces: QVariant,
    selected_workspace_id: QString,
}

impl Default for AppShellRust {
    fn default() -> Self {
        let model = AppModel::default();
        let workspaces = snapshot_workspaces(&model);
        let selected_workspace_id = QString::from(model.selected_workspace_id().unwrap_or(""));
        Self {
            model,
            workspaces,
            selected_workspace_id,
        }
    }
}

impl qobject::AppShell {
    fn publish(mut self: Pin<&mut Self>) {
        let workspaces = snapshot_workspaces(&self.model);
        let selected = QString::from(self.model.selected_workspace_id().unwrap_or(""));
        self.as_mut().rust_mut().workspaces = workspaces;
        self.as_mut().rust_mut().selected_workspace_id = selected;
        self.as_mut().model_changed();
    }

    pub fn create_workspace(mut self: Pin<&mut Self>, title: &QString) {
        let title = title.to_string();
        let title = title.trim();
        let title = if title.is_empty() {
            None
        } else {
            Some(title.to_string())
        };
        self.as_mut().rust_mut().model.create_workspace(title);
        self.publish();
    }

    pub fn select_workspace(mut self: Pin<&mut Self>, id: &QString) {
        if self
            .as_mut()
            .rust_mut()
            .model
            .select_workspace(&id.to_string())
        {
            self.publish();
        }
    }

    pub fn rename_workspace(mut self: Pin<&mut Self>, id: &QString, title: &QString) {
        if self
            .as_mut()
            .rust_mut()
            .model
            .rename_workspace(&id.to_string(), &title.to_string())
        {
            self.publish();
        }
    }

    pub fn close_workspace(mut self: Pin<&mut Self>, id: &QString) {
        if self
            .as_mut()
            .rust_mut()
            .model
            .close_workspace(&id.to_string())
        {
            self.publish();
        }
    }

    pub fn create_terminal(mut self: Pin<&mut Self>, workspace_id: &QString) {
        if self
            .as_mut()
            .rust_mut()
            .model
            .create_terminal(&workspace_id.to_string())
            .is_some()
        {
            self.publish();
        }
    }

    pub fn select_surface(mut self: Pin<&mut Self>, workspace_id: &QString, surface_id: &QString) {
        if self
            .as_mut()
            .rust_mut()
            .model
            .select_surface(&workspace_id.to_string(), &surface_id.to_string())
        {
            self.publish();
        }
    }

    pub fn close_surface(mut self: Pin<&mut Self>, workspace_id: &QString, surface_id: &QString) {
        if self
            .as_mut()
            .rust_mut()
            .model
            .close_surface(&workspace_id.to_string(), &surface_id.to_string())
        {
            self.publish();
        }
    }

    pub fn selected_surface_id(&self, workspace_id: &QString) -> QString {
        self.model
            .workspace_by_id(&workspace_id.to_string())
            .and_then(|ws| ws.pane.selected_surface_id())
            .map(QString::from)
            .unwrap_or_default()
    }
}

impl cxx_qt::Initialize for qobject::AppShell {
    fn initialize(self: Pin<&mut Self>) {
        // Snapshots are already built in Default; emit so early QML bindings refresh.
        self.model_changed();
    }
}

fn snapshot_workspaces(model: &AppModel) -> QVariant {
    let mut list = QList::<QVariant>::default();
    for workspace in model.workspaces() {
        list.append(QVariant::from(&snapshot_workspace(workspace)));
    }
    QVariant::from(&list)
}

fn snapshot_workspace(workspace: &Workspace) -> QMap<QMapPair_QString_QVariant> {
    let mut map = QMap::<QMapPair_QString_QVariant>::default();
    map.insert(
        QString::from("id"),
        QVariant::from(&QString::from(&workspace.id)),
    );
    map.insert(
        QString::from("title"),
        QVariant::from(&QString::from(&workspace.title)),
    );

    let mut surfaces = QList::<QVariant>::default();
    for surface in &workspace.pane.surfaces {
        surfaces.append(QVariant::from(&snapshot_surface(surface)));
    }
    map.insert(QString::from("surfaces"), QVariant::from(&surfaces));

    if let Some(selected) = workspace.pane.selected_surface_id() {
        map.insert(
            QString::from("selectedSurfaceId"),
            QVariant::from(&QString::from(selected)),
        );
    }

    map
}

fn snapshot_surface(surface: &Surface) -> QMap<QMapPair_QString_QVariant> {
    let mut map = QMap::<QMapPair_QString_QVariant>::default();
    map.insert(
        QString::from("id"),
        QVariant::from(&QString::from(&surface.id)),
    );
    map.insert(
        QString::from("title"),
        QVariant::from(&QString::from(&surface.title)),
    );
    map.insert(
        QString::from("panel"),
        QVariant::from(&QString::from(surface.panel.as_str())),
    );
    map
}
