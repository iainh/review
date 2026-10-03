//! Desktop chrome. The app, not menu callbacks, owns document operations.
use std::sync::mpsc::{self, Receiver, Sender};

use egui_desktop::{KeyboardShortcut, MenuItem, SubMenuItem, TitleBar, TitleBarOptions};

use crate::native_ui::Appearance;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Open,
    Save(bool),
    Print,
    CloseTab,
    Quit,
    Appearance(Appearance),
    Fullscreen,
    Help,
}

pub struct Desktop {
    bar: TitleBar,
    actions: Receiver<Action>,
}

fn item(
    tx: &Sender<Action>,
    label: &str,
    action: Action,
    shortcut: Option<KeyboardShortcut>,
) -> SubMenuItem {
    let tx = tx.clone();
    let mut item = SubMenuItem::new(label).with_callback(Box::new(move || {
        let _ = tx.send(action);
    }));
    item.shortcut = shortcut;
    item
}

impl Default for Desktop {
    fn default() -> Self {
        let (tx, actions) = mpsc::channel();
        let key = |key, shift| {
            Some(KeyboardShortcut {
                key,
                modifiers: egui::Modifiers {
                    shift,
                    ..egui::Modifiers::COMMAND
                },
            })
        };
        let file = MenuItem::new("File")
            .add_subitem(
                item(&tx, "Open…", Action::Open, key(egui::Key::O, false)).with_separator(),
            )
            .add_subitem(item(
                &tx,
                "Save",
                Action::Save(false),
                key(egui::Key::S, false),
            ))
            .add_subitem(
                item(&tx, "Save As…", Action::Save(true), key(egui::Key::S, true)).with_separator(),
            )
            .add_subitem(
                item(&tx, "Print…", Action::Print, key(egui::Key::P, false)).with_separator(),
            )
            .add_subitem(item(
                &tx,
                "Close tab",
                Action::CloseTab,
                key(egui::Key::W, false),
            ))
            .add_subitem(item(&tx, "Quit", Action::Quit, key(egui::Key::Q, false)));
        let mut appearance = SubMenuItem::new("Appearance");
        for choice in [
            Appearance::System,
            Appearance::Light,
            Appearance::Dark,
            Appearance::HighContrast,
        ] {
            appearance =
                appearance.add_child(item(&tx, choice.label(), Action::Appearance(choice), None));
        }
        let view = MenuItem::new("View")
            .add_subitem(appearance)
            .add_subitem(item(
                &tx,
                "Fullscreen",
                Action::Fullscreen,
                Some(KeyboardShortcut::new(egui::Key::F11)),
            ));
        let help = MenuItem::new("Help").add_subitem(item(
            &tx,
            "Keyboard shortcuts",
            Action::Help,
            Some(KeyboardShortcut::new(egui::Key::F1)),
        ));
        let bar = TitleBar::new(
            TitleBarOptions::new()
                .with_title("Review")
                .with_title_visibility(true, true, true),
        )
        .add_menu_with_submenu(file)
        .add_menu_with_submenu(view)
        .add_menu_with_submenu(help);
        Self { bar, actions }
    }
}

impl Desktop {
    fn owns_input(&self) -> bool {
        self.bar.keyboard_navigation_active
            || self.bar.hamburger_menu_open
            || self.bar.render_state.is_any_menu_open()
    }

    /// Return the requested action and whether the menu owns this frame's input.
    /// Shortcut hints never dispatch; App/Viewer retain their focused-field gates.
    pub fn show(
        &mut self,
        root: &mut egui::Ui,
        document: bool,
        printable: bool,
        appearance: Appearance,
        blocked: bool,
    ) -> (Option<Action>, bool) {
        let was_open = self.owns_input();
        let ctx = root.ctx().clone();
        if blocked
            || (was_open
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)))
        {
            self.bar.close_all_menus();
        }
        let file = &mut self.bar.menu_items_with_submenus[0];
        for index in [1, 2, 4] {
            file.subitems[index].enabled = document;
        }
        file.subitems[3].enabled = printable;
        for (item, choice) in self.bar.menu_items_with_submenus[1].subitems[0]
            .children
            .iter_mut()
            .zip([
                Appearance::System,
                Appearance::Light,
                Appearance::Dark,
                Appearance::HighContrast,
            ])
        {
            item.label = format!(
                "{}{}",
                if appearance == choice { "✓ " } else { "" },
                choice.label()
            );
        }
        // winit/egui is the system-theme authority. Match the entire shell to
        // Review's visuals, including its high-contrast palette.
        let visuals = root.visuals();
        let foreground = visuals.text_color();
        let hover = if appearance == Appearance::HighContrast {
            visuals.selection.bg_fill
        } else {
            visuals.widgets.hovered.bg_fill
        };
        self.bar.background_color = visuals.panel_fill;
        self.bar.title_color = foreground;
        self.bar.menu_text_color = foreground;
        self.bar.close_icon_color = foreground;
        self.bar.maximize_icon_color = foreground;
        self.bar.restore_icon_color = foreground;
        self.bar.minimize_icon_color = foreground;
        self.bar.hover_color = hover;
        self.bar.menu_hover_color = hover;
        self.bar.keyboard_selection_color = visuals.selection.bg_fill;
        self.bar.submenu_background_color = visuals.window_fill;
        self.bar.submenu_text_color = foreground;
        self.bar.submenu_hover_color = hover;
        self.bar.submenu_disabled_color = visuals.weak_text_color();
        self.bar.submenu_shortcut_color = foreground;
        self.bar.submenu_border_color = visuals.widgets.noninteractive.bg_stroke.color;
        self.bar.submenu_keyboard_selection_color = visuals.selection.bg_fill;
        root.add_enabled_ui(!blocked, |ui| self.bar.show(ui));
        let owns_input = was_open || self.owns_input();
        let action = self.actions.try_iter().last();
        if action.is_some() {
            self.bar.close_all_menus();
            ctx.request_repaint();
        }
        if !ctx.input(|i| {
            i.viewport().fullscreen.unwrap_or(false) || i.viewport().maximized.unwrap_or(false)
        }) {
            egui_desktop::render_resize_handles(&ctx);
        }
        (action.filter(|_| !blocked), owns_input)
    }
}

/// A custom titlebar close must follow exactly the native close-request path.
pub fn close_requested(output: &egui::FullOutput) -> bool {
    output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .is_some_and(|viewport| {
            viewport
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Close))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Key, Modifiers, RawInput};

    fn frame(
        ctx: &Context,
        desktop: &mut Desktop,
        width: f32,
        events: Vec<Event>,
        blocked: bool,
    ) -> (Option<Action>, bool) {
        let mut result = (None, false);
        let _ = ctx.run_ui(
            RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let next = desktop.show(ui, false, false, Appearance::HighContrast, blocked);
                result.0 = next.0.or(result.0);
                result.1 |= next.1;
            },
        );
        result
    }

    fn key(key: Key, modifiers: Modifiers) -> Vec<Event> {
        vec![
            Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
            Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            },
        ]
    }

    #[test]
    fn shortcut_hints_leave_dispatch_with_host_and_alt_does_not_open_menus() {
        let ctx = Context::default();
        let mut desktop = Desktop::default();
        frame(&ctx, &mut desktop, 900.0, vec![], false);
        for (key, modifiers) in [
            (Key::O, Modifiers::COMMAND),
            (Key::S, Modifiers::COMMAND),
            (Key::ArrowLeft, Modifiers::ALT),
        ] {
            assert_eq!(
                frame(&ctx, &mut desktop, 900.0, self::key(key, modifiers), false),
                (None, false)
            );
        }
        assert!(!desktop.bar.menu_items_with_submenus[0].subitems[1].enabled);
        assert!(!desktop.bar.menu_items_with_submenus[0].subitems[3].enabled);
    }

    #[test]
    fn unavailable_file_actions_expose_disabled_accesskit_nodes() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let mut desktop = Desktop::default();
        frame(&ctx, &mut desktop, 900.0, vec![], false);
        frame(
            &ctx,
            &mut desktop,
            900.0,
            key(Key::F2, Modifiers::CTRL),
            false,
        );
        frame(
            &ctx,
            &mut desktop,
            900.0,
            key(Key::Enter, Modifiers::NONE),
            false,
        );
        let output = ctx.run_ui(RawInput::default(), |ui| {
            desktop.show(ui, false, false, Appearance::System, false);
        });
        let tree = output.platform_output.accesskit_update.unwrap();
        for (label, disabled) in [
            ("Open…", false),
            ("Save", true),
            ("Save As…", true),
            ("Print…", true),
            ("Close tab", true),
            ("Quit", false),
        ] {
            let node = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label))
                .unwrap();
            assert_eq!(node.1.is_disabled(), disabled, "{label}");
        }
    }

    #[test]
    fn keyboard_menus_invoke_real_callbacks_and_modal_blocks_activation() {
        let ctx = Context::default();
        let mut desktop = Desktop::default();
        frame(&ctx, &mut desktop, 900.0, vec![], false);
        assert!(
            !frame(
                &ctx,
                &mut desktop,
                900.0,
                key(Key::F2, Modifiers::CTRL),
                true
            )
            .1
        );
        assert!(
            frame(
                &ctx,
                &mut desktop,
                900.0,
                key(Key::F2, Modifiers::CTRL),
                false
            )
            .1
        );
        for _ in 0..2 {
            frame(
                &ctx,
                &mut desktop,
                900.0,
                key(Key::ArrowRight, Modifiers::NONE),
                false,
            );
        }
        frame(
            &ctx,
            &mut desktop,
            900.0,
            key(Key::Enter, Modifiers::NONE),
            false,
        );
        assert_eq!(
            frame(
                &ctx,
                &mut desktop,
                900.0,
                key(Key::Enter, Modifiers::NONE),
                false
            )
            .0,
            Some(Action::Help)
        );
        frame(
            &ctx,
            &mut desktop,
            900.0,
            key(Key::F2, Modifiers::CTRL),
            false,
        );
        assert!(
            frame(
                &ctx,
                &mut desktop,
                900.0,
                key(Key::Escape, Modifiers::NONE),
                false
            )
            .1
        );
        assert!(!desktop.owns_input());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn maximize_controls_follow_real_viewport_state_and_emit_native_commands() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let mut desktop = Desktop::default();
        for maximized in [false, true] {
            let mut input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                ..Default::default()
            };
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .maximized = Some(maximized);
            let output = ctx.run_ui(input.clone(), |ui| {
                desktop.show(ui, false, false, Appearance::System, false);
            });
            let label = if maximized {
                "Restore window"
            } else {
                "Maximize window"
            };
            let tree = output.platform_output.accesskit_update.unwrap();
            let target = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label))
                .unwrap()
                .0;
            input.events.push(Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: egui::accesskit::TreeId::ROOT,
                    target_node: target,
                    data: None,
                },
            ));
            let output = ctx.run_ui(input, |ui| {
                desktop.show(ui, false, false, Appearance::System, false);
            });
            assert!(output.viewport_output[&egui::ViewportId::ROOT].commands.iter().any(|command| {
                matches!(command, egui::ViewportCommand::Maximized(value) if *value == !maximized)
            }));
        }
    }

    #[test]
    fn responsive_bar_preserves_menu_order_and_matches_high_contrast() {
        let ctx = Context::default();
        Appearance::HighContrast.apply(&ctx);
        let mut desktop = Desktop::default();
        frame(&ctx, &mut desktop, 900.0, vec![], false);
        assert_eq!(desktop.bar.items_fitted.len(), 3);
        frame(&ctx, &mut desktop, 240.0, vec![], false);
        assert!(desktop.bar.items_fitted.len() < 3);
        assert_eq!(desktop.bar.menu_order.len(), 3);
        assert_eq!(desktop.bar.background_color, egui::Color32::BLACK);
        assert_eq!(desktop.bar.submenu_text_color, egui::Color32::WHITE);
        assert_eq!(
            desktop.bar.menu_items_with_submenus[1].subitems[0].children[3].label,
            "✓ High contrast"
        );
    }

    #[test]
    fn border_drag_emits_native_resize() {
        let ctx = Context::default();
        let mut desktop = Desktop::default();
        let mut run = |events| {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    desktop.show(ui, false, false, Appearance::System, false);
                },
            )
        };
        run(vec![]);
        run(vec![]); // Settle the initial Area sizing pass before interaction.
        let position = egui::pos2(898.0, 598.0);
        run(vec![
            Event::PointerMoved(position),
            Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        let output = run(vec![Event::PointerMoved(egui::pos2(888.0, 588.0))]);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| {
                    matches!(
                        command,
                        egui::ViewportCommand::BeginResize(egui::ResizeDirection::SouthEast)
                    )
                })
        );
    }

    #[test]
    fn only_root_close_requests_document_protection() {
        let ctx = Context::default();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            ui.ctx().send_viewport_cmd_to(
                egui::ViewportId::from_hash_of("other"),
                egui::ViewportCommand::Close,
            );
        });
        assert!(!close_requested(&output));
        let output = ctx.run_ui(RawInput::default(), |ui| {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close)
        });
        assert!(close_requested(&output));
    }

    #[test]
    #[ignore = "reopens the PDF saved through the native desktop File menu"]
    fn verify_native_desktop_save() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        let document = crate::document::PdfDocument::open(directory.join("forms.pdf")).unwrap();
        let fields = document.form_fields(0).unwrap();
        let name = fields
            .iter()
            .find(|field| field.label == "Full name")
            .unwrap();
        assert_eq!(name.value, "Mina desktop");
    }
}
