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
            let frame = egui::Frame::new().fill(root.visuals().window_fill);
            egui::Panel::top("document_tabs")
                .frame(frame)
                .show_inside(root, |ui| {
                    if !enabled && ui.is_enabled() {
                        ui.disable();
                    }
                    egui::ScrollArea::horizontal()
                        .id_salt("tab_strip")
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
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
                                        } else {
                                            name.into_owned()
                                        };
                                        let active = Some(tab.id) == self.active_id();
                                        let (selected, close) = document_tab(
                                            ui,
                                            &label,
                                            active,
                                            self.is_dirty(tab.id, viewer),
                                        );
                                        let selected = selected
                                            .on_hover_text(tab.file.path.display().to_string());
                                        if reveal && active {
                                            ui.scroll_to_rect(
                                                selected.rect.union(close.rect),
                                                Some(egui::Align::Center),
                                            );
                                        }
                                        if selected.clicked() {
                                            action = Some(Action::Select(tab.id));
                                        }
                                        if close.clicked() {
                                            action = Some(Action::Close(tab.id));
                                        }
                                    });
                                }
                            });
                        });
                });
        }
        action
    }
}

/// Flat document tab with independent native selection and close controls.
fn document_tab(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    dirty: bool,
) -> (egui::Response, egui::Response) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(label.into(), font, ui.visuals().text_color())
            .size()
            .x
    });
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2((text_width + 60.0).clamp(160.0, 260.0), 36.0),
        egui::Sense::hover(),
    );
    let background = ui.painter().add(egui::Shape::Noop);
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 18.0, rect.center().y),
        egui::vec2(26.0, 26.0),
    );
    let mut label_rect = rect.shrink2(egui::vec2(8.0, 3.0));
    label_rect.max.x = close_rect.left() - if dirty { 16.0 } else { 4.0 };
    let selected = ui.put(
        label_rect,
        egui::Button::selectable(
            active,
            egui::RichText::new(label).color(if active {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            }),
        )
        .frame(false)
        .truncate(),
    );
    selected.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            active,
            if dirty {
                format!("{label} (unsaved changes)")
            } else {
                label.into()
            },
        )
    });
    let close = ui
        .put(
            close_rect,
            egui::Button::new(
                egui::RichText::new(char::from(crate::icons::Icon::X).to_string()).size(14.0),
            )
            .frame_when_inactive(false)
            .corner_radius(4),
        )
        .on_hover_text(format!("Close {label}"));
    close.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Close {label}"),
        )
    });
    let fill = if active {
        ui.visuals().faint_bg_color
    } else if selected.hovered() || close.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().window_fill
    };
    ui.painter()
        .set(background, egui::Shape::rect_filled(rect, 0, fill));
    let accent = if ui.visuals().panel_fill == egui::Color32::BLACK {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().selection.bg_fill
    };
    if dirty {
        ui.painter().circle_filled(
            egui::pos2(close_rect.left() - 8.0, rect.center().y),
            3.0,
            accent,
        );
    }
    if active {
        ui.painter().hline(
            (rect.left() + 8.0)..=(rect.right() - 8.0),
            rect.bottom() - 2.0,
            egui::Stroke::new(2.0_f32, accent),
        );
    }
    if selected.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(2.0),
            2,
            egui::Stroke::new(2.0_f32, accent),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().vline(
        rect.right() - 0.5,
        (rect.top() + 10.0)..=(rect.bottom() - 10.0),
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    (selected, close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_keep_full_accessible_names_and_independent_selection_and_close_targets() {
        let mut tabs = Tabs::default();
        let long_name = "Quarterly financial report with supporting schedules and notes.pdf";
        tabs.restore(&Session {
            files: ["Brief.pdf", long_name]
                .map(|name| SessionFile {
                    path: Path::new("/documents").join(name),
                    reading: Default::default(),
                    sidebar: Default::default(),
                })
                .into(),
            active: 0,
        });
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut draw = |events, enabled| {
            let mut action = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700.0, 300.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| action = tabs.ui(ui, enabled, None),
            );
            (action, output)
        };
        let (_, output) = draw(vec![], true);
        let tree = output.platform_output.accesskit_update.unwrap();
        let name = format!("{long_name} (not opened)");
        let selection = &tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(name.as_str()))
            .unwrap()
            .1;
        assert_eq!(selection.toggled(), Some(egui::accesskit::Toggled::False));
        let close = &tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(format!("Close {name}").as_str()))
            .unwrap()
            .1;
        let centre = |node: &egui::accesskit::Node| {
            let bounds = node.bounds().unwrap();
            egui::pos2(
                ((bounds.x0 + bounds.x1) / 2.0) as f32,
                ((bounds.y0 + bounds.y1) / 2.0) as f32,
            )
        };
        let select_position = centre(selection);
        let close_position = centre(close);
        assert!(
            !selection
                .bounds()
                .unwrap()
                .contains(egui::accesskit::Point::new(
                    close_position.x as f64,
                    close_position.y as f64
                ))
        );
        let click = |position| {
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        assert!(matches!(
            draw(click(select_position), true).0,
            Some(Action::Select(1))
        ));
        assert!(matches!(
            draw(click(close_position), true).0,
            Some(Action::Close(1))
        ));
        assert!(draw(click(close_position), false).0.is_none());
    }

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
