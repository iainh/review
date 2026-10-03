//! Recent-file and personal-bookmark controls, separate from PDF outline bookmarks.
use std::path::PathBuf;

use crate::{
    persistence::{Bookmark, MAX_BOOKMARKS, State},
    viewer::Viewer,
};

pub enum Action {
    Open(PathBuf),
    Bookmark(Bookmark),
    ClearHistory,
}

#[derive(Default)]
pub struct Library {
    confirm_clear: bool,
}

impl Library {
    pub fn ui(
        &mut self,
        root: &mut egui::Ui,
        state: &mut State,
        viewer: Option<&Viewer>,
        enabled: bool,
    ) -> Option<Action> {
        let ctx = root.ctx().clone();
        let mut action = None;
        let mut toggle = enabled
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::B));
        egui::Panel::top("library").show_inside(root, |ui| {
            if !enabled {
                ui.disable();
            }
            ui.horizontal(|ui| {
                ui.menu_button("Recent files", |ui| {
                    if state.recent.is_empty() {
                        ui.label("No recent files");
                    }
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for recent in &state.recent {
                                let name = recent
                                    .path
                                    .file_name()
                                    .unwrap_or(recent.path.as_os_str())
                                    .to_string_lossy();
                                if ui
                                    .button(format!(
                                        "{name} · Page {}",
                                        recent.reading.page.saturating_add(1)
                                    ))
                                    .on_hover_text(recent.path.display().to_string())
                                    .clicked()
                                {
                                    action = Some(Action::Open(recent.path.clone()));
                                    ui.close();
                                }
                            }
                        });
                    ui.separator();
                    if ui
                        .add_enabled(
                            !state.recent.is_empty(),
                            egui::Button::new("Clear history…"),
                        )
                        .clicked()
                    {
                        self.confirm_clear = true;
                        ui.close();
                    }
                });
                ui.menu_button("Personal bookmarks", |ui| {
                    let bookmarked = viewer.is_some_and(|viewer| {
                        state.bookmarks.iter().any(|bookmark| {
                            bookmark.path == viewer.state_key()
                                && bookmark.reading.page == viewer.reading_state().page
                        })
                    });
                    let label = if bookmarked {
                        "Remove this page bookmark"
                    } else {
                        "Bookmark this page"
                    };
                    if ui
                        .add_enabled(
                            viewer.is_some()
                                && (bookmarked || state.bookmarks.len() < MAX_BOOKMARKS),
                            egui::Button::new(label),
                        )
                        .on_hover_text("Ctrl+B / Cmd+B")
                        .clicked()
                    {
                        toggle = true;
                        ui.close();
                    }
                    if state.bookmarks.len() >= MAX_BOOKMARKS {
                        ui.label("Bookmark limit reached; remove one to add another.");
                    }
                    ui.separator();
                    if state.bookmarks.is_empty() {
                        ui.label("No personal bookmarks");
                    }
                    let mut remove = None;
                    egui::ScrollArea::vertical()
                        .id_salt("personal_bookmarks")
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for (index, bookmark) in state.bookmarks.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    let name = bookmark
                                        .path
                                        .file_name()
                                        .unwrap_or(bookmark.path.as_os_str())
                                        .to_string_lossy();
                                    if ui
                                        .button(format!(
                                            "{name} · Page {}",
                                            bookmark.reading.page.saturating_add(1)
                                        ))
                                        .on_hover_text(bookmark.path.display().to_string())
                                        .clicked()
                                    {
                                        action = Some(Action::Bookmark(bookmark.clone()));
                                        ui.close();
                                    }
                                    if ui
                                        .small_button("×")
                                        .on_hover_text("Remove bookmark")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                });
                            }
                        });
                    if let Some(index) = remove {
                        state.bookmarks.remove(index);
                    }
                });
            });
        });
        if toggle && let Some(viewer) = viewer {
            state.toggle_bookmark(viewer.state_key().to_path_buf(), viewer.reading_state());
        }
        if self.confirm_clear && enabled {
            egui::Window::new("Clear reading history?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("Remove recent files and their saved reading positions.");
                    ui.label("Personal bookmarks and window/sidebar preferences are kept.");
                    ui.horizontal(|ui| {
                        if ui.button("Clear history").clicked() {
                            action = Some(Action::ClearHistory);
                            self.confirm_clear = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_clear = false;
                        }
                    });
                });
        }
        action
    }
}
