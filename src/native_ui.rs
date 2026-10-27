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
        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(6.0, 4.0);
            style.spacing.button_padding = egui::vec2(6.0, 4.0);
            style.spacing.interact_size.y = CONTROL_HEIGHT;
        });
        ctx.request_repaint();
    }
}

pub const CONTROL_HEIGHT: f32 = 28.0;

/// Review's subset of Solidcraft's semantic palette. See vendor/README.md for provenance.
/// These colours belong to the shell, never the PDF page or its annotations.
struct Palette {
    chrome: Color32,
    toolbar: Color32,
    panel: Color32,
    canvas: Color32,
    floating: Color32,
    field: Color32,
    border: Color32,
    text: Color32,
    dim: Color32,
    accent: Color32,
    selected: Color32,
    hover: Color32,
    focus: Color32,
    error: Color32,
    warning: Color32,
}

impl Palette {
    fn new(dark: bool) -> Self {
        let rgb = Color32::from_rgb;
        if dark {
            Self {
                chrome: rgb(28, 31, 38),
                toolbar: rgb(37, 41, 49),
                panel: rgb(39, 43, 52),
                canvas: rgb(46, 52, 64),
                floating: rgb(47, 52, 62),
                field: rgb(30, 34, 41),
                border: rgb(63, 69, 82),
                text: rgb(222, 226, 232),
                dim: rgb(148, 155, 168),
                accent: rgb(58, 136, 238),
                selected: rgb(49, 66, 88),
                hover: rgb(54, 61, 74),
                focus: rgb(128, 182, 255),
                error: rgb(240, 100, 96),
                warning: rgb(236, 170, 70),
            }
        } else {
            Self {
                chrome: rgb(229, 232, 238),
                toolbar: rgb(246, 247, 249),
                panel: rgb(250, 251, 252),
                canvas: rgb(223, 228, 235),
                floating: Color32::WHITE,
                field: Color32::WHITE,
                border: rgb(201, 207, 217),
                text: rgb(38, 42, 50),
                dim: rgb(98, 106, 120),
                accent: rgb(26, 100, 190),
                selected: rgb(214, 230, 250),
                hover: rgb(226, 234, 246),
                focus: rgb(26, 100, 190),
                error: rgb(180, 35, 35),
                warning: rgb(134, 81, 10),
            }
        }
    }
}

pub fn chrome_fill(visuals: &Visuals) -> Color32 {
    if visuals.panel_fill == Color32::BLACK {
        visuals.panel_fill
    } else {
        Palette::new(visuals.dark_mode).chrome
    }
}

/// Docked toolbar bands stay square and flush; floating surfaces use egui's themed frames.
pub fn toolbar_frame(style: &egui::Style) -> egui::Frame {
    let fill = if style.visuals.panel_fill == Color32::BLACK {
        style.visuals.panel_fill
    } else {
        Palette::new(style.visuals.dark_mode).toolbar
    };
    egui::Frame::side_top_panel(style)
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(8, 6))
}

/// Keep related commands together when the toolbar wraps on a narrow window.
pub fn toolbar_group(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui)) {
    let id = ui.id().with(label);
    let previous = ui.ctx().data(|data| data.get_temp::<egui::Vec2>(id));
    // Wrapping needs the group's width before painting any child controls.
    // Remember natural sizes without replaying the caller's command dispatch.
    let desired = previous.unwrap_or(egui::vec2(ui.available_width(), CONTROL_HEIGHT));
    let measured = ui
        .allocate_ui_with_layout(desired, egui::Layout::top_down(egui::Align::Min), |ui| {
            egui::Frame::new()
                .fill(chrome_fill(ui.visuals()))
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(5, 4))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(contents);
                        ui.label(
                            egui::RichText::new(label)
                                .size(10.0)
                                .variation(b"wght", 600.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                })
                .response
                .rect
                .size()
        })
        .inner;
    if previous != Some(measured) {
        ui.ctx().data_mut(|data| data.insert_temp(id, measured));
        ui.ctx().request_repaint();
    }
}

fn studio_visuals(dark: bool) -> Visuals {
    let palette = Palette::new(dark);
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.override_text_color = Some(palette.text);
    visuals.weak_text_color = Some(palette.dim);
    visuals.panel_fill = palette.panel;
    visuals.window_fill = palette.floating;
    visuals.faint_bg_color = palette.canvas;
    visuals.extreme_bg_color = palette.field;
    visuals.selection.bg_fill = palette.selected;
    visuals.selection.stroke = Stroke::new(1.0_f32, palette.text);
    visuals.hyperlink_color = palette.accent;
    visuals.error_fg_color = palette.error;
    visuals.warn_fg_color = palette.warning;
    visuals.window_corner_radius = 8.into();
    visuals.menu_corner_radius = 6.into();
    visuals.window_stroke = Stroke::new(1.0_f32, palette.border);
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 55 } else { 30 }),
    };
    visuals.popup_shadow = visuals.window_shadow;
    for (widget, fill) in [
        (&mut visuals.widgets.noninteractive, palette.panel),
        (&mut visuals.widgets.inactive, palette.field),
        (&mut visuals.widgets.hovered, palette.hover),
        (&mut visuals.widgets.active, palette.selected),
        (&mut visuals.widgets.open, palette.selected),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0_f32, palette.border);
        widget.fg_stroke = Stroke::new(1.0_f32, palette.text);
        widget.corner_radius = 4.into();
        widget.expansion = 0.0;
    }
    visuals.widgets.noninteractive.bg_stroke = visuals.window_stroke;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, palette.accent);
    visuals.widgets.active.bg_stroke = Stroke::new(2.0_f32, palette.focus);
    visuals.text_edit_bg_color = Some(visuals.extreme_bg_color);
    visuals
}

/// Native egui states provide hover, disabled and keyboard focus feedback without a second painter.
pub fn studio_button(ui: &mut egui::Ui, button: egui::Button<'_>, primary: bool) -> egui::Response {
    let high_contrast = ui.visuals().panel_fill == Color32::BLACK;
    ui.scope(|ui| {
        if !primary && !high_contrast {
            // Keep the frame's layout space when its inactive paint is hidden.
            let inactive = &mut ui.visuals_mut().widgets.inactive;
            inactive.weak_bg_fill = Color32::TRANSPARENT;
            inactive.bg_stroke.color = Color32::TRANSPARENT;
        }
        ui.add(button.frame_when_inactive(true))
    })
    .inner
}

#[derive(Default)]
pub struct NativeUi {
    pub appearance: Appearance,
    pub settings_open: bool,
    settings_first_frame: bool,
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
        self.help_open || self.settings_open
    }

    pub fn open_settings(&mut self, ctx: &Context) {
        self.restore_focus = ctx.memory(|memory| memory.focused());
        self.settings_open = true;
        self.settings_first_frame = true;
        ctx.request_repaint();
    }

    /// Settings apply immediately; callers persist changes through the normal store.
    pub fn settings(
        &mut self,
        ctx: &Context,
        state: &mut crate::persistence::State,
        path: Option<&std::path::Path>,
    ) -> bool {
        if !self.settings_open {
            return false;
        }
        let before = (
            self.appearance,
            state.restore_session,
            state.hide_single_tab,
            state.default_zoom,
            state.default_layout,
        );
        let viewport = ctx.content_rect();
        let response = egui::Modal::new(Id::new("settings"))
            .frame(egui::Frame::window(&ctx.global_style()).inner_margin(16.0))
            .show(ctx, |ui| {
            ui.set_width((viewport.width() - 64.0).clamp(200.0, 440.0));
            ui.label(egui::RichText::new("Settings").size(18.0).variation(b"wght", 600.0));
            ui.add_space(8.0);
            ui.style_mut().spacing.scroll.floating = false;
            egui::ScrollArea::vertical()
                .id_salt("settings_content")
                .max_height((viewport.height() - 150.0).max(100.0))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Appearance").variation(b"wght", 600.0));
                    ui.horizontal_wrapped(|ui| {
                        for choice in [Appearance::System, Appearance::Light, Appearance::Dark, Appearance::HighContrast] {
                            ui.radio_value(&mut self.appearance, choice, choice.label());
                        }
                    });
                    ui.weak("PDF page colours are unchanged.");
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Documents").variation(b"wght", 600.0));
                    ui.checkbox(&mut state.hide_single_tab, "Hide tabs for a single document");
                    ui.checkbox(&mut state.restore_session, "Restore tabs on startup");
                    ui.weak("Unsaved changes and passwords are not restored.");
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("New document view").variation(b"wght", 600.0));
                    let control_width = (ui.available_width() - 100.0).clamp(100.0, 180.0);
                    egui::Grid::new("default_view").spacing([16.0, 6.0]).show(ui, |ui| {
                        let label = ui.label("Zoom");
                        egui::ComboBox::from_id_salt("default_zoom")
                            .width(control_width)
                            .selected_text(match state.default_zoom {
                                crate::zoom::Zoom::Percent(_) => "Custom".into(),
                                zoom => zoom.label(),
                            })
                            .show_ui(ui, |ui| {
                                use crate::zoom::Zoom;
                                ui.selectable_value(&mut state.default_zoom, Zoom::FitPage, "Fit page");
                                ui.selectable_value(&mut state.default_zoom, Zoom::FitWidth, "Fit width");
                                let percent = match state.default_zoom { Zoom::Percent(value) => value, _ => 1.0 };
                                ui.selectable_value(&mut state.default_zoom, Zoom::Percent(percent), "Custom");
                            }).response.labelled_by(label.id);
                        ui.end_row();
                        if let crate::zoom::Zoom::Percent(value) = &mut state.default_zoom {
                            let label = ui.label("Percentage");
                            let mut percent = *value * 100.0;
                            if ui.add(egui::DragValue::new(&mut percent).range(10.0..=1600.0).suffix("%"))
                                .labelled_by(label.id)
                                .on_hover_text("Zoom percentage, from 10% to 1600%")
                                .changed() {
                                *value = percent / 100.0;
                            }
                            ui.end_row();
                        }
                        let label = ui.label("Page layout");
                        egui::ComboBox::from_id_salt("default_layout")
                            .width(control_width)
                            .selected_text(state.default_layout.label())
                            .show_ui(ui, |ui| {
                                use crate::layout::LayoutMode;
                                for mode in [LayoutMode::Single, LayoutMode::Continuous, LayoutMode::Facing] {
                                    ui.selectable_value(&mut state.default_layout, mode, mode.label());
                                }
                            }).response.labelled_by(label.id);
                        ui.end_row();
                    });
                    ui.weak("Saved reading positions take precedence. Open documents are unchanged.");
                    ui.add_space(12.0);
                    ui.separator();
                    egui::CollapsingHeader::new("Settings file location")
                        .id_salt("settings_file_location")
                        .show(ui, |ui| {
                            if let Some(path) = path {
                                ui.add(egui::Label::new(path.to_string_lossy()).wrap().selectable(true));
                            } else {
                                ui.weak("User configuration directory is unavailable.");
                            }
                        });
                });
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.weak("Changes apply immediately");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = ui.button("Done").on_hover_text("Close settings (Esc)");
                    if self.settings_first_frame {
                        close.request_focus();
                    }
                    close.clicked()
                }).inner
            }).inner
        });
        self.settings_first_frame = false;
        if response.inner || response.should_close() {
            self.settings_open = false;
            ctx.request_repaint();
        }
        if self.appearance != before.0 {
            self.appearance.apply(ctx);
        }
        before
            != (
                self.appearance,
                state.restore_session,
                state.hide_single_tab,
                state.default_zoom,
                state.default_layout,
            )
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
        if !self.help_open && !self.settings_open {
            // Restore after underlying controls were drawn: the previous
            // frame's modal layer still rejects their focus during drawing.
            if let Some(id) = self.restore_focus.take() {
                ctx.memory_mut(|memory| memory.request_focus(id));
                ctx.request_repaint();
            }
            return;
        }
        if !self.help_open {
            return;
        }
        let primary = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        let viewport = ctx.content_rect();
        let response = egui::Modal::new(Id::new("shortcut_help"))
            .frame(egui::Frame::window(&ctx.global_style()).inner_margin(16.0))
            .show(ctx, |ui| {
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
                    (format!("{primary}+,"), "Open settings"),
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
    fn toolbar_hover_borders_do_not_move_buttons_or_neighbours() {
        for appearance in [
            Appearance::Dark,
            Appearance::Light,
            Appearance::HighContrast,
        ] {
            let ctx = Context::default();
            appearance.apply(&ctx);
            crate::icons::install(&ctx);
            let mut rectangles = Vec::new();
            let mut frame = |pointer| {
                for _ in 0..3 {
                    ctx.run_ui(
                        egui::RawInput {
                            events: vec![egui::Event::PointerMoved(pointer)],
                            ..Default::default()
                        },
                        |ui| {
                            rectangles.clear();
                            ui.horizontal(|ui| {
                                rectangles.push(
                                    crate::icons::button(ui, crate::icons::Icon::Search, "Search")
                                        .rect,
                                );
                                rectangles.push(
                                    crate::icons::selectable_button(
                                        ui,
                                        crate::icons::Icon::Maximize,
                                        "Fit page",
                                        true,
                                    )
                                    .rect,
                                );
                                rectangles.push(ui.button("Go").rect);
                            });
                        },
                    )
                    .textures_delta
                    .clear();
                }
                rectangles.clone()
            };
            let baseline = frame(egui::pos2(500.0, 200.0));
            for rect in &baseline {
                assert_eq!(frame(rect.center()), baseline, "{appearance:?}");
            }
            assert_eq!(frame(egui::pos2(500.0, 200.0)), baseline);
        }
    }

    #[test]
    fn toolbar_groups_wrap_without_clipping_and_rejoin_when_widened() {
        let ctx = Context::default();
        for width in [640.0, 320.0, 640.0] {
            let mut buttons = Vec::new();
            for _ in 0..2 {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 300.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        buttons.clear();
                        ui.horizontal_wrapped(|ui| {
                            for label in ["Navigation", "Document"] {
                                toolbar_group(ui, label, |ui| {
                                    buttons.push(
                                        ui.add_sized(
                                            [200.0, CONTROL_HEIGHT],
                                            egui::Button::new(label),
                                        )
                                        .rect,
                                    );
                                });
                            }
                        });
                    },
                )
                .textures_delta
                .clear();
            }
            assert!(buttons.iter().all(|rect| rect.right() <= width));
            if width == 320.0 {
                assert!(buttons[1].top() > buttons[0].bottom());
            } else {
                assert_eq!(buttons[0].top(), buttons[1].top());
            }
        }
    }

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
            ctx.run_ui(
                egui::RawInput {
                    system_theme: Some(theme),
                    ..Default::default()
                },
                |_| {},
            )
            .textures_delta
            .clear();
            assert_eq!(ctx.theme(), theme);
        }
    }
}
