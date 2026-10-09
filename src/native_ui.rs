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
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::HighContrast => "High contrast",
        }
    }

    pub fn apply(self, ctx: &Context) {
        ctx.set_visuals_of(Theme::Light, studio_visuals(false));
        ctx.set_visuals_of(Theme::Dark, studio_visuals(true));
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

/// Neutral studio surfaces, sampled from ImageFlow. Keep paper colours separate.
fn studio_visuals(dark: bool) -> Visuals {
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let grey = Color32::from_gray;
    visuals.override_text_color = Some(grey(if dark { 214 } else { 40 }));
    visuals.weak_text_color = Some(grey(if dark { 154 } else { 90 }));
    visuals.panel_fill = grey(if dark { 50 } else { 225 });
    visuals.window_fill = grey(if dark { 40 } else { 238 });
    visuals.faint_bg_color = grey(if dark { 64 } else { 205 });
    visuals.extreme_bg_color = grey(if dark { 25 } else { 250 });
    visuals.selection.bg_fill = Color32::from_rgb(20, 115, 230);
    visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.hyperlink_color = Color32::from_rgb(62, 140, 235);
    visuals.window_corner_radius = 2.into();
    visuals.menu_corner_radius = 2.into();
    visuals.window_stroke = Stroke::new(1.0_f32, grey(if dark { 26 } else { 140 }));
    for (widget, fill) in [
        (
            &mut visuals.widgets.noninteractive,
            if dark { 50 } else { 225 },
        ),
        (&mut visuals.widgets.inactive, if dark { 51 } else { 230 }),
        (&mut visuals.widgets.hovered, if dark { 70 } else { 245 }),
        (&mut visuals.widgets.active, if dark { 42 } else { 195 }),
        (&mut visuals.widgets.open, if dark { 62 } else { 205 }),
    ] {
        widget.bg_fill = grey(fill);
        widget.weak_bg_fill = grey(fill);
        widget.bg_stroke = Stroke::new(1.0_f32, grey(if dark { 35 } else { 150 }));
        widget.fg_stroke = Stroke::new(1.0_f32, grey(if dark { 196 } else { 40 }));
        widget.corner_radius = 2.into();
        widget.expansion = 0.0;
    }
    visuals.widgets.noninteractive.bg_stroke = visuals.window_stroke;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, visuals.selection.bg_fill);
    visuals.text_edit_bg_color = Some(visuals.extreme_bg_color);
    visuals
}

/// A one-pixel raised edge, not grain or a gradient across the workspace.
pub fn panel_bevel(ui: &egui::Ui) {
    if ui.visuals().panel_fill == Color32::BLACK {
        return;
    }
    let rect = ui.max_rect();
    let colour = Color32::from_gray(if ui.visuals().dark_mode { 61 } else { 245 });
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0_f32, colour));
}

/// Keep egui's button input, focus and accessibility; paint only its surface.
pub fn studio_button(ui: &mut egui::Ui, button: egui::Button<'_>, primary: bool) -> egui::Response {
    if ui.visuals().panel_fill == Color32::BLACK {
        return ui.add(button);
    }
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui.add(button.fill(Color32::TRANSPARENT).stroke(Stroke::NONE));
    if ui.is_rect_visible(response.rect) {
        let down = response.is_pointer_button_down_on();
        let hovered = response.hovered();
        let dark = ui.visuals().dark_mode;
        let (top, bottom) = if primary {
            (
                Color32::from_rgb(62, 140, 235),
                Color32::from_rgb(20, 115, 230),
            )
        } else {
            let (top, bottom) = if dark {
                if hovered { (76, 66) } else { (59, 51) }
            } else if hovered {
                (250, 235)
            } else {
                (240, 220)
            };
            (Color32::from_gray(top), Color32::from_gray(bottom))
        };
        let (top, bottom) = if down { (bottom, top) } else { (top, bottom) };
        let border = if response.has_focus() {
            Stroke::new(2.0_f32, Color32::from_rgb(100, 180, 255))
        } else if primary {
            Stroke::new(1.0_f32, Color32::from_rgb(16, 94, 189))
        } else {
            Stroke::new(1.0_f32, Color32::from_gray(if dark { 35 } else { 150 }))
        };
        let rect = response.rect;
        let inner = rect.shrink(1.0);
        let mut mesh = egui::Mesh::default();
        for (pos, colour) in [
            (inner.left_top(), top),
            (inner.right_top(), top),
            (inner.right_bottom(), bottom),
            (inner.left_bottom(), bottom),
        ] {
            mesh.colored_vertex(pos, colour);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        let highlight = Color32::from_white_alpha(if down { 0 } else { 15 });
        ui.painter().set(
            background,
            egui::Shape::Vec(vec![
                egui::Shape::mesh(mesh),
                egui::Shape::rect_stroke(rect, 2, border, egui::StrokeKind::Inside),
                egui::Shape::line_segment(
                    [inner.left_top(), inner.right_top()],
                    Stroke::new(1.0_f32, highlight),
                ),
            ]),
        );
    }
    response
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
        self.help_open
    }

    pub fn open_help(&mut self, ctx: &Context) {
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
        let viewport = ctx.content_rect();
        let response = egui::Modal::new(Id::new("shortcut_help")).show(ctx, |ui| {
            ui.set_width((viewport.width() - 40.0).clamp(200.0, 480.0));
            ui.heading("Keyboard shortcuts");
            ui.style_mut().spacing.scroll.floating = false;
            egui::ScrollArea::vertical()
                .max_height((viewport.height() - 120.0).max(100.0))
                .show(ui, |ui| {
            egui::Grid::new("shortcuts")
                .spacing([24.0, 6.0])
                .max_col_width(((ui.available_width() - 24.0) * 0.5).max(60.0))
                .show(ui, |ui| {
                for (keys, action) in [
                    (format!("{primary}+O"), "Open PDF"),
                    (format!("{primary}+S / {primary}+Shift+S"), "Save / Save As"),
                    (format!("{primary}+Z / {primary}+Shift+Z"), "Undo / redo document edit (outside text fields)"),
                    (format!("{primary}+P"), "Print PDF"),
                    (format!("{primary}+Q"), "Close window"),
                    (format!("{primary}+W"), "Close current document"),
                    ("Ctrl+Tab / Ctrl+Shift+Tab".into(), "Next / previous document"),
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
                    ("Ctrl+F2, arrows, Enter / Space".into(), "Navigate desktop menus; Escape dismisses"),
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
                });
            ui.separator();
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
