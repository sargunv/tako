//! Durable app shell state: workspaces, one implicit pane each, and surfaces.
//!
//! QML consumes immutable snapshots of this model via the cxx-qt bridge; all
//! mutations go through actions on the bridge.

/// Kind of panel behind a surface. Only terminals are live for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    Terminal,
}

impl PanelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
        }
    }
}

/// One selectable tab inside a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    pub id: String,
    pub title: String,
    pub panel: PanelKind,
}

/// A tiled leaf that holds a stack of surfaces. Each workspace starts with one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: String,
    pub surfaces: Vec<Surface>,
    /// Index into `surfaces` for the focused tab.
    pub selected: usize,
}

impl Pane {
    pub fn selected_surface_id(&self) -> Option<&str> {
        self.surfaces.get(self.selected).map(|s| s.id.as_str())
    }

    pub fn select_surface(&mut self, surface_id: &str) -> bool {
        if let Some(index) = self.surfaces.iter().position(|s| s.id == surface_id) {
            self.selected = index;
            true
        } else {
            false
        }
    }
}

/// A sidebar row: one workspace owns one main pane for now (splits come later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    pub title: String,
    pub pane: Pane,
}

/// Root window model: ordered workspaces plus selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppModel {
    workspaces: Vec<Workspace>,
    selected_workspace: usize,
    next_id: u64,
}

impl Default for AppModel {
    fn default() -> Self {
        let mut model = Self {
            workspaces: Vec::new(),
            selected_workspace: 0,
            next_id: 1,
        };
        model.create_workspace(Some("Workspace".to_string()));
        model
    }
}

impl AppModel {
    fn alloc_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{}", self.next_id);
        self.next_id += 1;
        id
    }

    fn new_terminal_surface(&mut self, title: String) -> Surface {
        Surface {
            id: self.alloc_id("surface"),
            title,
            panel: PanelKind::Terminal,
        }
    }

    fn next_terminal_title(&self, workspace_id: Option<&str>) -> String {
        let existing = workspace_id
            .and_then(|id| self.workspace_by_id(id))
            .map(|ws| ws.pane.surfaces.len())
            .unwrap_or(0);
        if existing == 0 {
            "Terminal".to_string()
        } else {
            format!("Terminal {}", existing + 1)
        }
    }

    fn new_pane_with_terminal(&mut self) -> Pane {
        let surface = self.new_terminal_surface("Terminal".to_string());
        Pane {
            id: self.alloc_id("pane"),
            surfaces: vec![surface],
            selected: 0,
        }
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn selected_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.selected_workspace)
    }

    pub fn selected_workspace_id(&self) -> Option<&str> {
        self.selected_workspace().map(|ws| ws.id.as_str())
    }

    pub fn workspace_by_id(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|ws| ws.id == id)
    }

    fn workspace_index(&self, id: &str) -> Option<usize> {
        self.workspaces.iter().position(|ws| ws.id == id)
    }

    /// Create a workspace with one pane and one terminal tab. Selects it.
    pub fn create_workspace(&mut self, title: Option<String>) -> &Workspace {
        let n = self.workspaces.len() + 1;
        let title = title.unwrap_or_else(|| format!("Workspace {n}"));
        let workspace = Workspace {
            id: self.alloc_id("ws"),
            title,
            pane: self.new_pane_with_terminal(),
        };
        self.workspaces.push(workspace);
        self.selected_workspace = self.workspaces.len() - 1;
        &self.workspaces[self.selected_workspace]
    }

    pub fn select_workspace(&mut self, id: &str) -> bool {
        if let Some(index) = self.workspace_index(id) {
            self.selected_workspace = index;
            true
        } else {
            false
        }
    }

    pub fn rename_workspace(&mut self, id: &str, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == id) {
            ws.title = title.to_string();
            true
        } else {
            false
        }
    }

    /// Close a workspace. Ensures at least one workspace remains.
    pub fn close_workspace(&mut self, id: &str) -> bool {
        let Some(index) = self.workspace_index(id) else {
            return false;
        };

        if self.workspaces.len() == 1 {
            // Replace the sole workspace with a fresh default rather than
            // leaving the shell empty.
            self.workspaces.clear();
            self.selected_workspace = 0;
            self.create_workspace(Some("Workspace".to_string()));
            return true;
        }

        self.workspaces.remove(index);
        if self.selected_workspace >= self.workspaces.len() {
            self.selected_workspace = self.workspaces.len() - 1;
        } else if index < self.selected_workspace {
            self.selected_workspace -= 1;
        }
        true
    }

    /// Add a terminal tab to the workspace's implicit pane and select it.
    pub fn create_terminal(&mut self, workspace_id: &str) -> Option<&Surface> {
        let index = self.workspace_index(workspace_id)?;
        let title = self.next_terminal_title(Some(workspace_id));
        let surface = self.new_terminal_surface(title);
        let pane = &mut self.workspaces[index].pane;
        pane.surfaces.push(surface);
        pane.selected = pane.surfaces.len() - 1;
        Some(&pane.surfaces[pane.selected])
    }

    pub fn select_surface(&mut self, workspace_id: &str, surface_id: &str) -> bool {
        let Some(index) = self.workspace_index(workspace_id) else {
            return false;
        };
        self.workspaces[index].pane.select_surface(surface_id)
    }

    /// Close a surface tab. If it was the last tab, spawn a replacement terminal.
    pub fn close_surface(&mut self, workspace_id: &str, surface_id: &str) -> bool {
        let Some(ws_index) = self.workspace_index(workspace_id) else {
            return false;
        };
        let Some(surf_index) = self.workspaces[ws_index]
            .pane
            .surfaces
            .iter()
            .position(|s| s.id == surface_id)
        else {
            return false;
        };

        {
            let pane = &mut self.workspaces[ws_index].pane;
            pane.surfaces.remove(surf_index);

            if !pane.surfaces.is_empty() {
                if pane.selected >= pane.surfaces.len() {
                    pane.selected = pane.surfaces.len() - 1;
                } else if surf_index < pane.selected {
                    pane.selected -= 1;
                }
                return true;
            }
        }

        let replacement = self.new_terminal_surface("Terminal".to_string());
        let pane = &mut self.workspaces[ws_index].pane;
        pane.surfaces.push(replacement);
        pane.selected = 0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_one_workspace_with_one_terminal() {
        let model = AppModel::default();
        assert_eq!(model.workspaces().len(), 1);
        let ws = model.selected_workspace().unwrap();
        assert_eq!(ws.pane.surfaces.len(), 1);
        assert_eq!(ws.pane.surfaces[0].panel, PanelKind::Terminal);
        assert_eq!(ws.pane.selected, 0);
    }

    #[test]
    fn create_and_select_workspace() {
        let mut model = AppModel::default();
        let first_id = model.selected_workspace_id().unwrap().to_string();
        model.create_workspace(Some("Agents".to_string()));
        let second_id = model.selected_workspace_id().unwrap().to_string();
        assert_ne!(first_id, second_id);
        assert_eq!(model.workspaces().len(), 2);
        assert!(model.select_workspace(&first_id));
        assert_eq!(model.selected_workspace_id(), Some(first_id.as_str()));
    }

    #[test]
    fn rename_and_close_workspace() {
        let mut model = AppModel::default();
        model.create_workspace(Some("Temp".to_string()));
        let id = model.selected_workspace_id().unwrap().to_string();
        assert!(model.rename_workspace(&id, "Renamed"));
        assert_eq!(model.workspace_by_id(&id).unwrap().title, "Renamed");
        assert!(!model.rename_workspace(&id, "   "));
        assert!(model.close_workspace(&id));
        assert_eq!(model.workspaces().len(), 1);
        assert!(model.workspace_by_id(&id).is_none());
    }

    #[test]
    fn closing_last_workspace_replaces_it() {
        let mut model = AppModel::default();
        let old_id = model.selected_workspace_id().unwrap().to_string();
        assert!(model.close_workspace(&old_id));
        assert_eq!(model.workspaces().len(), 1);
        assert_ne!(model.selected_workspace_id(), Some(old_id.as_str()));
    }

    #[test]
    fn terminal_tabs_add_select_close() {
        let mut model = AppModel::default();
        let ws_id = model.selected_workspace_id().unwrap().to_string();
        let first = model.selected_workspace().unwrap().pane.surfaces[0]
            .id
            .clone();
        let second = model.create_terminal(&ws_id).unwrap().id.clone();
        assert_eq!(
            model.workspace_by_id(&ws_id).unwrap().pane.surfaces.len(),
            2
        );
        assert_eq!(
            model
                .workspace_by_id(&ws_id)
                .unwrap()
                .pane
                .selected_surface_id(),
            Some(second.as_str())
        );
        assert!(model.select_surface(&ws_id, &first));
        assert!(model.close_surface(&ws_id, &first));
        assert_eq!(
            model.workspace_by_id(&ws_id).unwrap().pane.surfaces.len(),
            1
        );
        assert_eq!(
            model
                .workspace_by_id(&ws_id)
                .unwrap()
                .pane
                .selected_surface_id(),
            Some(second.as_str())
        );
    }

    #[test]
    fn closing_last_tab_spawns_replacement() {
        let mut model = AppModel::default();
        let ws_id = model.selected_workspace_id().unwrap().to_string();
        let only = model.selected_workspace().unwrap().pane.surfaces[0]
            .id
            .clone();
        assert!(model.close_surface(&ws_id, &only));
        let pane = &model.workspace_by_id(&ws_id).unwrap().pane;
        assert_eq!(pane.surfaces.len(), 1);
        assert_ne!(pane.surfaces[0].id, only);
        assert_eq!(pane.surfaces[0].panel, PanelKind::Terminal);
    }
}
