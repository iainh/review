use std::ops::Range;

use anyhow::Result;
use egui::{Color32, Key, Modifiers, Rect, Sense, Ui, Vec2};

use crate::{document::PdfDocument, structured_text::PageText};

/// Only the displayed page is cached. Selection survives zoom, not navigation.
#[derive(Default)]
pub struct Selection {
    page: Option<usize>,
    revision: u64,
    text: PageText,
    anchor: usize,
    range: Range<usize>,
}

impl Selection {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn escape(&mut self, ctx: &egui::Context) {
        if !self.range.is_empty()
            && !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.range = 0..0;
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        document: &PdfDocument,
        page: Rect,
        copy_allowed: bool,
    ) -> Result<()> {
        if self.page != Some(document.current_page()) || self.revision != document.text_revision() {
            self.clear();
        }
        if !copy_allowed {
            self.clear();
        } else if self.page.is_none() {
            self.page = Some(document.current_page());
            self.text = document.structured_text(document.current_page())?;
            self.revision = document.text_revision();
        }
        let response = ui.interact(
            page,
            ui.id().with("text_selection"),
            Sense::click_and_drag(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Other,
                ui.is_enabled(),
                format!("PDF page {}", document.current_page() + 1),
            )
        });
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(egui::accesskit::Role::Image)
        });
        let point = |position: egui::Pos2| {
            [
                (position.x - page.min.x) / page.width(),
                (position.y - page.min.y) / page.height(),
            ]
        };
        if copy_allowed {
            if response.hovered() && !self.text.chars.is_empty() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            // Start on press, not drag_started's current position: otherwise the
            // first few characters disappear when the drag crosses the threshold.
            if response.hovered() && ui.input(|input| input.pointer.primary_pressed()) {
                response.surrender_focus();
                ui.memory_mut(|memory| {
                    if let Some(id) = memory.focused() {
                        memory.surrender_focus(id);
                    }
                });
                if let Some(hit) = ui
                    .input(|input| input.pointer.press_origin())
                    .and_then(|p| self.text.hit(point(p)))
                {
                    self.anchor = hit.caret;
                    self.range = hit.caret..hit.caret;
                }
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(hit) = response
                    .interact_pointer_pos()
                    .and_then(|p| self.text.hit(point(p)))
            {
                self.range = self.anchor.min(hit.caret)..self.anchor.max(hit.caret);
                // Keep selection usable beyond the viewport at high zoom.
                if let Some(position) = response.interact_pointer_pos() {
                    ui.scroll_to_rect(Rect::from_center_size(position, Vec2::splat(16.0)), None);
                }
            }
            if (response.double_clicked() || response.triple_clicked())
                && let Some(hit) = response
                    .interact_pointer_pos()
                    .and_then(|p| self.text.hit(point(p)))
            {
                self.range = if response.triple_clicked() {
                    self.text.paragraph(hit.glyph)
                } else {
                    self.text.word(hit.glyph)
                };
                self.anchor = self.range.start;
            }
            if !ui.ctx().egui_wants_keyboard_input() {
                if ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::A)) {
                    self.select_all();
                }
                let copy = ui.input_mut(|input| {
                    let key = input.consume_key(Modifiers::COMMAND, Key::C);
                    let event = input
                        .events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Copy));
                    input
                        .events
                        .retain(|event| !matches!(event, egui::Event::Copy));
                    key || event
                });
                if copy {
                    self.copy(ui.ctx());
                }
            }
        }
        response.context_menu(|ui| {
            if !copy_allowed {
                ui.label("This PDF does not allow copying text.");
            }
            if ui
                .add_enabled(
                    copy_allowed && !self.range.is_empty(),
                    egui::Button::new("Copy"),
                )
                .clicked()
            {
                self.copy(ui.ctx());
                ui.close();
            }
            if ui
                .add_enabled(
                    copy_allowed && !self.text.chars.is_empty(),
                    egui::Button::new("Select all on page"),
                )
                .clicked()
            {
                self.select_all();
                ui.close();
            }
        });
        for ch in &self.text.chars[self.range.clone()] {
            if let Some(quad) = ch.quad {
                let points =
                    quad.map(|p| page.min + Vec2::new(p[0] * page.width(), p[1] * page.height()));
                ui.painter().add(egui::Shape::convex_polygon(
                    points.to_vec(),
                    Color32::from_rgba_unmultiplied(50, 125, 255, 85),
                    egui::Stroke::NONE,
                ));
            }
        }
        Ok(())
    }

    fn select_all(&mut self) {
        self.anchor = 0;
        self.range = 0..self.text.chars.len();
    }

    fn copy(&self, ctx: &egui::Context) {
        if !self.range.is_empty() {
            ctx.copy_text(self.text.text(self.range.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        selection: &mut Selection,
        ctx: &egui::Context,
        document: &PdfDocument,
        events: Vec<egui::Event>,
        time: f64,
        allowed: bool,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(600.0, 600.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                selection
                    .ui(
                        ui,
                        document,
                        Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(300.0, 400.0)),
                        allowed,
                    )
                    .unwrap();
            },
        )
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn command(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }
    }

    fn clipboard(output: egui::FullOutput) -> Option<String> {
        output
            .platform_output
            .commands
            .into_iter()
            .find_map(|cmd| match cmd {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
    }

    #[test]
    fn drag_keeps_press_anchor_and_reverse_drag_copies_same_characters() {
        let ctx = egui::Context::default();
        let document = crate::document::tests::sample_document();
        let mut selection = Selection::default();
        frame(&mut selection, &ctx, &document, vec![], 0.0, true);
        // PDF points, independent of extracted bounds: Alpha at x=40, y=350
        // in MediaBox [10 20 310 420], hence x=30 and baseline y=70.
        let start = egui::pos2(31.0, 64.0);
        let end = egui::pos2(52.0, 64.0);
        for (a, b, time) in [(start, end, 1.0), (end, start, 2.0)] {
            frame(
                &mut selection,
                &ctx,
                &document,
                vec![egui::Event::PointerMoved(a)],
                time,
                true,
            );
            frame(
                &mut selection,
                &ctx,
                &document,
                vec![button(a, true)],
                time + 0.1,
                true,
            );
            frame(
                &mut selection,
                &ctx,
                &document,
                vec![egui::Event::PointerMoved(b)],
                time + 0.2,
                true,
            );
            frame(
                &mut selection,
                &ctx,
                &document,
                vec![button(b, false)],
                time + 0.3,
                true,
            );
            let output = frame(
                &mut selection,
                &ctx,
                &document,
                vec![egui::Event::Copy],
                time + 0.4,
                true,
            );
            assert_eq!(clipboard(output).as_deref(), Some("Alp"));
        }
    }

    #[test]
    fn double_and_triple_click_expand_to_word_and_paragraph() {
        let ctx = egui::Context::default();
        let document = crate::document::tests::sample_document();
        let mut selection = Selection::default();
        let pos = egui::pos2(44.0, 64.0);
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![egui::Event::PointerMoved(pos)],
            0.0,
            true,
        );
        for n in 1..=3 {
            frame(
                &mut selection,
                &ctx,
                &document,
                vec![button(pos, true)],
                n as f64 * 0.1,
                true,
            );
            frame(
                &mut selection,
                &ctx,
                &document,
                vec![button(pos, false)],
                n as f64 * 0.1 + 0.02,
                true,
            );
            if n >= 2 {
                assert_eq!(
                    selection.text.text(selection.range.clone()),
                    if n == 2 {
                        "Alpha"
                    } else {
                        "Alpha alpha\nNeedle\nphrase"
                    }
                );
            }
        }
    }

    #[test]
    fn context_menu_copies_existing_selection() {
        let ctx = egui::Context::default();
        let document = crate::document::tests::sample_document();
        let mut selection = Selection::default();
        frame(&mut selection, &ctx, &document, vec![], 0.0, true);
        selection.range = 0..5;
        let pos = egui::pos2(44.0, 64.0);
        let secondary = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![egui::Event::PointerMoved(pos), secondary(true)],
            1.0,
            true,
        );
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![secondary(false)],
            1.1,
            true,
        );
        let target = egui::pos2(68.0, 81.0);
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![egui::Event::PointerMoved(target)],
            1.2,
            true,
        );
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![button(target, true)],
            1.3,
            true,
        );
        let output = frame(
            &mut selection,
            &ctx,
            &document,
            vec![button(target, false)],
            1.4,
            true,
        );
        assert_eq!(clipboard(output).as_deref(), Some("Alpha"));
    }

    #[test]
    fn copy_denial_clears_cache_and_selection_and_navigation_clears_selection() {
        let ctx = egui::Context::default();
        let mut document = crate::document::tests::sample_document();
        let mut selection = Selection::default();
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![command(Key::A)],
            0.0,
            true,
        );
        assert!(!selection.range.is_empty());
        let denied = frame(
            &mut selection,
            &ctx,
            &document,
            vec![command(Key::A), egui::Event::Copy],
            1.0,
            false,
        );
        assert!(clipboard(denied).is_none());
        assert!(selection.range.is_empty());
        assert!(selection.text.chars.is_empty());
        assert!(selection.page.is_none());
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![command(Key::A)],
            2.0,
            true,
        );
        document.go_to_page(1);
        let new_page = frame(
            &mut selection,
            &ctx,
            &document,
            vec![egui::Event::Copy],
            3.0,
            true,
        );
        assert!(clipboard(new_page).is_none());
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![command(Key::A)],
            4.0,
            true,
        );
        assert_eq!(
            clipboard(frame(
                &mut selection,
                &ctx,
                &document,
                vec![command(Key::C)],
                5.0,
                true
            ))
            .as_deref(),
            Some("Last alpha")
        );
    }

    #[test]
    fn recognition_replaces_an_empty_cache_and_invalidates_old_selection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("blank.pdf");
        std::fs::write(&path, crate::document::tests::sample_pdf("", false)).unwrap();
        let document = PdfDocument::open(&path).unwrap();
        let ctx = egui::Context::default();
        let mut selection = Selection::default();
        frame(&mut selection, &ctx, &document, vec![], 0.0, true);
        assert!(selection.text.chars.is_empty());
        document
            .set_recognized_text(0, crate::ocr::tests::word_text("amber fox"))
            .unwrap();
        frame(
            &mut selection,
            &ctx,
            &document,
            vec![command(Key::A)],
            1.0,
            true,
        );
        assert_eq!(
            clipboard(frame(
                &mut selection,
                &ctx,
                &document,
                vec![command(Key::C)],
                1.1,
                true
            ))
            .as_deref(),
            Some("amber fox")
        );
        document
            .set_recognized_text(0, crate::ocr::tests::word_text("violet river"))
            .unwrap();
        assert!(
            clipboard(frame(
                &mut selection,
                &ctx,
                &document,
                vec![command(Key::C)],
                2.0,
                true
            ))
            .is_none()
        );
        assert!(selection.range.is_empty());
        assert_eq!(selection.text.plain_text(), "violet river");
        assert!(
            clipboard(frame(
                &mut selection,
                &ctx,
                &document,
                vec![command(Key::A), command(Key::C)],
                3.0,
                false
            ))
            .is_none()
        );
        assert!(selection.text.chars.is_empty());
    }
}
