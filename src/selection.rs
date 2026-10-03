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

    pub fn quads(&self, page: usize) -> Vec<crate::structured_text::Quad> {
        if self.page != Some(page) {
            return Vec::new();
        }
        self.text
            .lines
            .iter()
            .filter_map(|line| {
                let start = line.chars.start.max(self.range.start);
                let end = line.chars.end.min(self.range.end);
                if start >= end {
                    return None;
                }
                let glyphs: Vec<_> = self.text.chars[start..end]
                    .iter()
                    .filter_map(|ch| ch.quad)
                    .collect();
                let first = glyphs.iter().find(|q| {
                    let a = [q[1][0] - q[0][0], q[1][1] - q[0][1]];
                    let b = [q[3][0] - q[0][0], q[3][1] - q[0][1]];
                    (a[0] * b[1] - a[1] * b[0]).abs() > 1e-12
                })?;
                let origin = first[0];
                let a = [first[1][0] - origin[0], first[1][1] - origin[1]];
                let b = [first[3][0] - origin[0], first[3][1] - origin[1]];
                let determinant = a[0] * b[1] - a[1] * b[0];
                let mut min = [f32::INFINITY; 2];
                let mut max = [f32::NEG_INFINITY; 2];
                // Bound in the line's glyph basis, not an axis-aligned page box:
                // this preserves rotated/skewed text and reversed glyph order.
                for p in glyphs.iter().flatten() {
                    let p = [p[0] - origin[0], p[1] - origin[1]];
                    let local = [
                        (p[0] * b[1] - p[1] * b[0]) / determinant,
                        (a[0] * p[1] - a[1] * p[0]) / determinant,
                    ];
                    for axis in 0..2 {
                        min[axis] = min[axis].min(local[axis]);
                        max[axis] = max[axis].max(local[axis]);
                    }
                }
                let point = |u: f32, v: f32| {
                    [
                        origin[0] + u * a[0] + v * b[0],
                        origin[1] + u * a[1] + v * b[1],
                    ]
                };
                Some([
                    point(min[0], min[1]),
                    point(max[0], min[1]),
                    point(max[0], max[1]),
                    point(min[0], max[1]),
                ])
            })
            .collect()
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
        let selection_allowed = copy_allowed || document.permissions().annotate;
        if !selection_allowed {
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
        if selection_allowed {
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
                if copy && copy_allowed {
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
                    selection_allowed && !self.text.chars.is_empty(),
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

    #[test]
    fn markup_joins_glyphs_per_line_without_losing_rotation_or_reversed_order() {
        use crate::structured_text::{TextChar, TextLine};
        let mut selection = Selection {
            page: Some(0),
            range: 0..3,
            ..Default::default()
        };
        let quads = [
            [[0.3, 0.6], [0.3, 0.55], [0.25, 0.55], [0.25, 0.6]],
            [[0.3, 0.7], [0.3, 0.65], [0.25, 0.65], [0.25, 0.7]],
            [[0.3, 0.5], [0.3, 0.45], [0.25, 0.45], [0.25, 0.5]],
        ];
        selection.text.chars = quads
            .into_iter()
            .map(|quad| TextChar {
                ch: 'a',
                quad: Some(quad),
                bidi: 0,
            })
            .collect();
        selection.text.lines = vec![TextLine {
            chars: 0..3,
            direction: [0.0, -1.0],
        }];
        let joined = selection.quads(0);
        assert_eq!(joined.len(), 1);
        for (actual, expected) in
            joined[0]
                .into_iter()
                .zip([[0.3, 0.7], [0.3, 0.45], [0.25, 0.45], [0.25, 0.7]])
        {
            for axis in 0..2 {
                assert!((actual[axis] - expected[axis]).abs() < 1e-6);
            }
        }
        assert!(selection.quads(1).is_empty());
        selection.text.lines = vec![
            TextLine {
                chars: 0..1,
                direction: [0.0, -1.0],
            },
            TextLine {
                chars: 1..3,
                direction: [0.0, -1.0],
            },
        ];
        assert_eq!(selection.quads(0).len(), 2);
    }

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
    fn copy_denial_allows_annotation_selection_and_navigation_clears_selection() {
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
        assert!(!selection.range.is_empty());
        assert!(!selection.quads(0).is_empty());
        assert_eq!(selection.page, Some(0));
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
        // Copy denial still permits markup selection when annotations are allowed.
        assert!(document.permissions().annotate);
        assert_eq!(selection.text.plain_text(), "violet river");
        assert!(!selection.quads(0).is_empty());
    }
}
