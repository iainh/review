//! Personal-library shortcuts and confirmation UI.
use crate::{persistence::State, viewer::Viewer};

#[derive(Default)]
pub struct Library {
    confirm_clear: bool,
    focus_clear: bool,
}

impl Library {
    pub fn ui(
        &mut self,
        root: &mut egui::Ui,
        state: &mut State,
        viewer: Option<&Viewer>,
        enabled: bool,
    ) -> bool {
        let ctx = root.ctx().clone();
        let toggle = enabled
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::B));
        if toggle && let Some(viewer) = viewer {
            state.toggle_bookmark(viewer.state_key().to_path_buf(), viewer.reading_state());
        }
        let mut clear_history = false;
        if self.confirm_clear && enabled {
            egui::Window::new("Clear reading history?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("Remove reading history and the saved tab session.");
                    ui.label("Personal bookmarks and window/sidebar preferences are kept.");
                    ui.horizontal(|ui| {
                        let clear = ui.button("Clear history");
                        if self.focus_clear {
                            clear.request_focus();
                            self.focus_clear = false;
                        }
                        if clear.clicked() {
                            clear_history = true;
                            self.confirm_clear = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_clear = false;
                        }
                    });
                });
        }
        clear_history
    }

    pub fn confirm_clear(&mut self) {
        self.confirm_clear = true;
        self.focus_clear = true;
    }
}
