//! Window-wide appearance and shortcut help, independent of PDF rendering.
use egui::{Color32, Context, Id, Key, Modifiers, Stroke, Theme, ThemePreference, Visuals};

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
    HighContrast,
}

impl Appearance {
    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::HighContrast => "High contrast",
        }
    }

    pub fn apply(self, ctx: &Context) {
        ctx.set_visuals_of(Theme::Light, Visuals::light());
        ctx.set_visuals_of(Theme::Dark, Visuals::dark());
        let theme = match self {
            Self::System => ThemePreference::System,
            Self::Light => ThemePreference::Light,
            Self::Dark | Self::HighContrast => ThemePreference::Dark,
        };
        if self == Self::HighContrast {
            let mut visuals = Visuals::dark();
            visuals.override_text_color = Some(Color32::WHITE);
            visuals.panel_fill = Color32::BLACK;
            visuals.window_fill = Color32::BLACK;
            visuals.extreme_bg_color = Color32::BLACK;
            visuals.faint_bg_color = Color32::BLACK;
            visuals.selection.bg_fill = Color32::from_rgb(0, 60, 120);
            visuals.selection.stroke = Stroke::new(2.0_f32, Color32::YELLOW);
            visuals.hyperlink_color = Color32::from_rgb(100, 200, 255);
            visuals.error_fg_color = Color32::from_rgb(255, 150, 150);
            visuals.warn_fg_color = Color32::YELLOW;
            for widget in [
                &mut visuals.widgets.noninteractive,
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
                &mut visuals.widgets.open,
            ] {
                widget.bg_fill = Color32::BLACK;
                widget.weak_bg_fill = Color32::BLACK;
                widget.bg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
                widget.fg_stroke = Stroke::new(2.0_f32, Color32::WHITE);
            }
            visuals.widgets.active.bg_stroke = Stroke::new(3.0_f32, Color32::YELLOW);
            visuals.widgets.hovered.bg_stroke = Stroke::new(2.0_f32, Color32::YELLOW);
            ctx.set_visuals_of(Theme::Dark, visuals);
        }
        ctx.set_theme(theme);
        ctx.request_repaint();
    }
}

#[derive(Default)]
pub struct NativeUi {
    pub appearance: Appearance,
    pub help_open: bool,
    help_first_frame: bool,
    restore_focus: Option<Id>,
}

impl NativeUi {
    /// Returns true while help owns keyboard input.
    pub fn begin(&mut self, root: &mut egui::Ui) -> bool {
        let ctx = root.ctx().clone();
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F1)) {
            if self.help_open {
                self.close_help(&ctx);
            } else {
                self.open_help(&ctx);
            }
        }
        egui::Panel::bottom("native_controls").show_inside(root, |ui| {
            ui.horizontal_wrapped(|ui| {
                let previous = self.appearance;
                egui::ComboBox::from_label("Appearance")
                    .selected_text(self.appearance.label())
                    .show_ui(ui, |ui| {
                        for choice in [
                            Appearance::System,
                            Appearance::Light,
                            Appearance::Dark,
                            Appearance::HighContrast,
                        ] {
                            if ui
                                .selectable_value(&mut self.appearance, choice, choice.label())
                                .clicked()
                            {
                                ui.close();
                            }
                        }
                    });
                if previous != self.appearance {
                    self.appearance.apply(&ctx);
                }
                if ui.button("Shortcut help").on_hover_text("F1").clicked() {
                    self.open_help(&ctx);
                }
                ui.label("Tab / Shift+Tab to move focus");
            });
        });
        self.help_open
    }

    fn open_help(&mut self, ctx: &Context) {
        self.restore_focus = ctx.memory(|memory| memory.focused());
        self.help_open = true;
        self.help_first_frame = true;
    }

    fn close_help(&mut self, ctx: &Context) {
        self.help_open = false;
        ctx.request_repaint();
    }

    pub fn finish(&mut self, ctx: &Context) {
        if !self.help_open {
            // Restore after underlying controls were drawn: the previous
            // frame's modal layer still rejects their focus during drawing.
            if let Some(id) = self.restore_focus.take() {
                ctx.memory_mut(|memory| memory.request_focus(id));
                ctx.request_repaint();
            }
            return;
        }
        let primary = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        let response = egui::Modal::new(Id::new("shortcut_help")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("Keyboard shortcuts");
            egui::Grid::new("shortcuts").spacing([24.0, 6.0]).show(ui, |ui| {
                for (keys, action) in [
                    (format!("{primary}+O"), "Open PDF"),
                    (format!("{primary}+S / {primary}+Shift+S"), "Save / Save As"),
                    (format!("{primary}+Z / {primary}+Shift+Z"), "Undo / redo document edit (outside text fields)"),
                    (format!("{primary}+P"), "Print PDF"),
                    (format!("{primary}+Q / {primary}+W"), "Close window"),
                    (format!("{primary}+G"), "Go to page field"),
                    (format!("{primary}+L"), "Select zoom percentage field"),
                    (format!("{primary}+F"), "Find in document"),
                    (format!("{primary}+D"), "Show / hide document properties"),
                    (format!("{primary}+B"), "Add / remove personal page bookmark"),
                    ("Enter / Shift+Enter".into(), "Next / previous match in search field"),
                    ("F3 / Shift+F3".into(), "Next / previous match"),
                    ("Left / Page Up, Right / Page Down".into(), "Previous / next page¹"),
                    ("Home / End".into(), "First / last page¹"),
                    ("Alt+Left / Alt+Right".into(), "Back / forward in navigation history"),
                    (format!("{primary}++ / {primary}+- / {primary}+0"), "Zoom in / out / fit page"),
                    ("+ / - / 0".into(), "Zoom in / out / fit page¹"),
                    ("1 / 2".into(), "Actual size (100%) / fit width¹"),
                    ("Ctrl+wheel / pinch".into(), "Zoom around the pointer on a page"),
                    ("R / Shift+R".into(), "Rotate clockwise / counterclockwise¹"),
                    ("H".into(), "Toggle text selection / hand pan¹"),
                    ("F11".into(), "Enter / leave fullscreen"),
                    ("F9".into(), "Show / hide sidebar"),
                    (format!("{primary}+A / {primary}+C"), "Select all page text / copy selection"),
                    (format!("{primary}+Shift+T"), "Show / hide current page text"),
                    ("F6 / Shift+F6".into(), "Cycle page, zoom, search, page-text and form text fields"),
                    ("Tab / Shift+Tab".into(), "Next / previous control"),
                    ("Enter / Space".into(), "Activate focused button"),
                    ("Escape".into(), "Dismiss UI, clear selection or leave fullscreen; never quit"),
                    ("F1".into(), "Show / hide shortcut help"),
                ] {
                    ui.label(keys);
                    ui.label(action);
                    ui.end_row();
                }
            });
            ui.separator();
            ui.label("¹ Unmodified document keys work only when no control has focus. Click the page background to release focus.");
            ui.label("Page text uses the PDF text layer, not OCR. Extraction order may differ from reading order; PDF tags and structure are not exposed.");
            let close = ui.button("Close help");
            if self.help_first_frame {
                close.request_focus();
            }
            close.clicked()
        });
        self.help_first_frame = false;
        if response.inner || response.should_close() {
            self.close_help(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_restores_standard_visuals_and_follows_system_changes() {
        let ctx = Context::default();
        Appearance::HighContrast.apply(&ctx);
        assert_eq!(ctx.global_style().visuals.panel_fill, Color32::BLACK);
        assert_eq!(ctx.global_style().visuals.text_color(), Color32::WHITE);
        assert_eq!(
            ctx.global_style().visuals.widgets.active.bg_stroke.width,
            3.0
        );
        Appearance::Light.apply(&ctx);
        assert_eq!(ctx.theme(), Theme::Light);
        assert_ne!(ctx.global_style().visuals.panel_fill, Color32::BLACK);
        Appearance::System.apply(&ctx);
        for theme in [Theme::Dark, Theme::Light] {
            let _ = ctx.run_ui(
                egui::RawInput {
                    system_theme: Some(theme),
                    ..Default::default()
                },
                |_| {},
            );
            assert_eq!(ctx.theme(), theme);
        }
    }
}
