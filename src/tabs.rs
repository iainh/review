//! Document lifetime and tab order. Session entries contain metadata, never a Viewer.
use std::path::Path;

use crate::{
    persistence::{MAX_SESSION, Session, SessionFile},
    viewer::Viewer,
};

pub enum Action {
    Select(u64),
    Close(u64),
}

struct Tab {
    id: u64,
    file: SessionFile,
    // The active Viewer lives in App; only inactive viewers live here.
    viewer: Option<Viewer>,
}

#[derive(Default)]
pub struct Tabs {
    entries: Vec<Tab>,
    active: Option<usize>,
    next_id: u64,
    reveal_active: bool,
}

impl Tabs {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn active_id(&self) -> Option<u64> {
        self.active.map(|index| self.entries[index].id)
    }

    pub fn is_dirty(&self, id: u64, active: Option<&Viewer>) -> bool {
        if Some(id) == self.active_id() {
            active.is_some_and(Viewer::is_dirty)
        } else {
            self.entries
                .iter()
                .find(|tab| tab.id == id)
                .and_then(|tab| tab.viewer.as_ref())
                .is_some_and(Viewer::is_dirty)
        }
    }

    pub fn dirty_ids(&self, active: Option<&Viewer>) -> Vec<u64> {
        self.entries
            .iter()
            .filter(|tab| self.is_dirty(tab.id, active))
            .map(|tab| tab.id)
            .collect()
    }

    pub fn find(&self, path: &Path) -> Option<u64> {
        self.entries
            .iter()
            .find(|tab| tab.file.path == path)
            .map(|tab| tab.id)
    }

    pub fn file(&self, id: u64) -> Option<&SessionFile> {
        self.entries
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| &tab.file)
    }

    fn push(&mut self, file: SessionFile) -> usize {
        let index = self.entries.len();
        self.entries.push(Tab {
            id: self.next_id,
            file,
            viewer: None,
        });
        self.next_id += 1;
        index
    }

    pub fn restore(&mut self, session: &Session) {
        for file in session.files.iter().take(MAX_SESSION) {
            self.push(file.clone());
        }
        self.active = (!self.entries.is_empty())
            .then_some(session.active.min(self.entries.len().saturating_sub(1)));
        self.reveal_active = true;
    }

    pub fn capture(&mut self, viewer: Option<&Viewer>) {
        if let Some(viewer) = viewer {
            let file = SessionFile {
                path: viewer.state_key().to_path_buf(),
                reading: viewer.reading_state(),
                sidebar: viewer.sidebar_state(),
            };
            if let Some(index) = self.active {
                self.entries[index].file = file;
            } else {
                self.active = Some(self.push(file));
            }
        }
    }

    pub fn session(&self) -> Session {
        Session {
            files: self.entries.iter().map(|tab| tab.file.clone()).collect(),
            active: self.active.unwrap_or(0),
        }
    }

    /// Returns false for a restored placeholder that still needs to be opened.
    pub fn select(&mut self, id: u64, viewer: &mut Option<Viewer>) -> bool {
        let Some(index) = self.entries.iter().position(|tab| tab.id == id) else {
            return false;
        };
        if self.active == Some(index) && viewer.is_some() {
            return true;
        }
        if self.entries[index].viewer.is_none() {
            return false;
        }
        self.capture(viewer.as_ref());
        if let Some(active) = self.active {
            self.entries[active].viewer = viewer.take();
        }
        *viewer = self.entries[index].viewer.take();
        self.active = Some(index);
        self.reveal_active = true;
        true
    }

    pub fn opened(&mut self, new_viewer: Viewer, viewer: &mut Option<Viewer>) {
        self.capture(viewer.as_ref());
        if let Some(active) = self.active {
            self.entries[active].viewer = viewer.take();
        }
        let index = self
            .entries
            .iter()
            .position(|tab| tab.file.path == new_viewer.state_key())
            .unwrap_or_else(|| {
                self.push(SessionFile {
                    path: new_viewer.state_key().to_path_buf(),
                    reading: new_viewer.reading_state(),
                    sidebar: new_viewer.sidebar_state(),
                })
            });
        self.active = Some(index);
        self.entries[index].viewer = None;
        self.reveal_active = true;
        *viewer = Some(new_viewer);
        self.capture(viewer.as_ref());
    }

    /// Returns the neighbour to activate, including a not-yet-opened restored tab.
    pub fn close(&mut self, id: u64, viewer: &mut Option<Viewer>) -> Option<u64> {
        let index = self.entries.iter().position(|tab| tab.id == id)?;
        let was_active = self.active == Some(index);
        self.capture(viewer.as_ref());
        self.entries.remove(index);
        if was_active {
            *viewer = None;
            self.active = None;
            self.entries
                .get(index.min(self.entries.len().saturating_sub(1)))
                .map(|tab| tab.id)
        } else {
            if let Some(active) = &mut self.active
                && index < *active
            {
                *active -= 1;
            }
            None
        }
    }

    pub fn ui(
        &mut self,
        root: &mut egui::Ui,
        enabled: bool,
        viewer: Option<&Viewer>,
    ) -> Option<Action> {
        let has_viewer = viewer.is_some();
        let mut action = None;
        if enabled {
            root.ctx().input_mut(|input| {
                if input.consume_key(egui::Modifiers::COMMAND, egui::Key::W) {
                    action = self.active_id().map(Action::Close);
                }
                let previous = input.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::Tab,
                );
                if (previous || input.consume_key(egui::Modifiers::CTRL, egui::Key::Tab))
                    && !self.entries.is_empty()
                {
                    let current = self.active.unwrap_or(0);
                    let index = if previous {
                        (current + self.len() - 1) % self.len()
                    } else {
                        (current + 1) % self.len()
                    };
                    action = Some(Action::Select(self.entries[index].id));
                }
            });
        }
        // The client titlebar carries the app brand; keep document identity
        // and its close action visible even with a single tab.
        if !self.entries.is_empty() {
            let reveal = std::mem::take(&mut self.reveal_active);
            egui::Panel::top("document_tabs").show_inside(root, |ui| {
                if !enabled {
                    ui.disable();
                }
                egui::ScrollArea::horizontal()
                    .id_salt("tab_strip")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for tab in &self.entries {
                                ui.push_id(tab.id, |ui| {
                                    let name = tab
                                        .file
                                        .path
                                        .file_name()
                                        .unwrap_or(tab.file.path.as_os_str())
                                        .to_string_lossy();
                                    let unopened = tab.viewer.is_none()
                                        && (Some(tab.id) != self.active_id() || !has_viewer);
                                    let label = if unopened {
                                        format!("{name} (not opened)")
                                    } else if self.is_dirty(tab.id, viewer) {
                                        format!("{name} *")
                                    } else {
                                        name.into_owned()
                                    };
                                    let selected = ui
                                        .selectable_label(Some(tab.id) == self.active_id(), label)
                                        .on_hover_text(tab.file.path.display().to_string());
                                    if reveal && Some(tab.id) == self.active_id() {
                                        selected.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if selected.clicked() {
                                        action = Some(Action::Select(tab.id));
                                    }
                                    if ui
                                        .small_button("×")
                                        .on_hover_text("Close document")
                                        .clicked()
                                    {
                                        action = Some(Action::Close(tab.id));
                                    }
                                    ui.separator();
                                });
                            }
                        });
                    });
            });
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_wrap_and_modal_tabs_do_not_consume_close_or_switch() {
        let directory = tempfile::tempdir().unwrap();
        let mut tabs = Tabs::default();
        tabs.restore(&Session {
            files: ["first.pdf", "second.pdf", "third.pdf"]
                .map(|name| SessionFile {
                    path: directory.path().join(name),
                    reading: Default::default(),
                    sidebar: Default::default(),
                })
                .into(),
            active: 2,
        });
        let ctx = egui::Context::default();
        let mut draw = |key, modifiers, enabled| {
            let mut action = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(960.0, 720.0),
                    )),
                    events: vec![egui::Event::Key {
                        key,
                        modifiers,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                    }],
                    ..Default::default()
                },
                |ui| action = tabs.ui(ui, enabled, None),
            );
            action
        };
        assert!(matches!(
            draw(egui::Key::Tab, egui::Modifiers::CTRL, true),
            Some(Action::Select(0))
        ));
        assert!(matches!(
            draw(
                egui::Key::Tab,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                true
            ),
            Some(Action::Select(1))
        ));
        assert!(matches!(
            draw(egui::Key::W, egui::Modifiers::COMMAND, true),
            Some(Action::Close(2))
        ));
        assert!(draw(egui::Key::W, egui::Modifiers::COMMAND, false).is_none());
        assert!(draw(egui::Key::Tab, egui::Modifiers::CTRL, false).is_none());
    }
}
