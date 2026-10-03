use std::{collections::BTreeMap, ops::Range};

use anyhow::Result;
use egui::{Color32, Key, Modifiers, Rect, Sense, Ui, Vec2};

use crate::{document::PdfDocument, layout::PageTransform, structured_text::PageText};

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Endpoint {
    page: usize,
    caret: usize,
}

/// Only visible pages are cached. Endpoints survive scrolling and rotation;
/// explicit navigation clears them. Copy extracts intervening pages on demand.
#[derive(Default)]
pub struct Selection {
    cache: BTreeMap<usize, PageText>,
    revision: u64,
    anchor: Endpoint,
    end: Endpoint,
}

impl Selection {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn quads(&self, page: usize) -> Vec<crate::structured_text::Quad> {
        let Some(text) = self.cache.get(&page) else {
            return Vec::new();
        };
        let range = self.range(page, text.chars.len());
        text.lines
            .iter()
            .filter_map(|line| {
                let start = line.chars.start.max(range.start);
                let end = line.chars.end.min(range.end);
                if start >= end {
                    return None;
                }
                let glyphs: Vec<_> = text.chars[start..end]
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
        if self.anchor != self.end
            && !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.end = self.anchor;
        }
    }

    #[cfg(test)]
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        document: &PdfDocument,
        page: Rect,
        copy_allowed: bool,
    ) -> Result<()> {
        if !self.cache.contains_key(&document.current_page()) {
            self.clear();
        }
        self.pages_ui(
            ui,
            document,
            &[(
                document.current_page(),
                PageTransform {
                    rect: page,
                    rotation: Default::default(),
                },
            )],
            copy_allowed,
            true,
        )
    }

    pub fn pages_ui(
        &mut self,
        ui: &mut Ui,
        document: &PdfDocument,
        pages: &[(usize, PageTransform)],
        copy_allowed: bool,
        selecting: bool,
    ) -> Result<()> {
        let selection_allowed = copy_allowed || document.permissions().annotate;
        if !selection_allowed || self.revision != document.text_revision() {
            self.clear();
            self.revision = document.text_revision();
        }
        if selection_allowed {
            self.cache
                .retain(|page, _| pages.iter().any(|(p, _)| p == page));
            for &(page, _) in pages {
                if let std::collections::btree_map::Entry::Vacant(entry) = self.cache.entry(page) {
                    entry.insert(document.structured_text(page)?);
                }
            }
        }
        for &(number, page) in pages {
            let response = ui.interact(
                page.rect,
                ui.id().with(("pdf_page", number)),
                Sense::hover(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Other,
                    ui.is_enabled(),
                    format!("PDF page {}", number + 1),
                )
            });
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(egui::accesskit::Role::Image)
            });
        }
        let response = ui.interact(
            pages
                .iter()
                .fold(Rect::NOTHING, |rect, (_, p)| rect.union(p.rect)),
            ui.id().with("text_selection"),
            if selecting {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            },
        );
        let nearest = |position: egui::Pos2| {
            pages.iter().min_by(|(_, a), (_, b)| {
                a.rect
                    .distance_to_pos(position)
                    .total_cmp(&b.rect.distance_to_pos(position))
            })
        };
        if selection_allowed && selecting {
            if response.hovered()
                && response
                    .hover_pos()
                    .and_then(nearest)
                    .is_some_and(|(p, t)| {
                        response.hover_pos().is_some_and(|pos| t.rect.contains(pos))
                            && !self.cache[p].chars.is_empty()
                    })
            {
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
                if let Some((page, hit)) =
                    ui.input(|input| input.pointer.press_origin())
                        .and_then(|pos| {
                            let (p, t) = nearest(pos)?;
                            t.rect.contains(pos).then_some(())?;
                            Some((*p, self.cache[p].hit(t.normalized(pos))?))
                        })
                {
                    self.anchor = Endpoint {
                        page,
                        caret: hit.caret,
                    };
                    self.end = self.anchor;
                }
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some((page, hit)) = response.interact_pointer_pos().and_then(|pos| {
                    let (p, t) = nearest(pos)?;
                    Some((*p, self.cache[p].hit(t.normalized(pos))?))
                })
            {
                self.end = Endpoint {
                    page,
                    caret: hit.caret,
                };
                // Keep selection usable beyond the viewport at high zoom.
                if let Some(position) = response.interact_pointer_pos() {
                    ui.scroll_to_rect(Rect::from_center_size(position, Vec2::splat(16.0)), None);
                }
            }
            if (response.double_clicked() || response.triple_clicked())
                && let Some((page, hit)) = response.interact_pointer_pos().and_then(|pos| {
                    let (p, t) = nearest(pos)?;
                    Some((*p, self.cache[p].hit(t.normalized(pos))?))
                })
            {
                let text = &self.cache[&page];
                let range = if response.triple_clicked() {
                    text.paragraph(hit.glyph)
                } else {
                    text.word(hit.glyph)
                };
                self.anchor = Endpoint {
                    page,
                    caret: range.start,
                };
                self.end = Endpoint {
                    page,
                    caret: range.end,
                };
            }
        }
        if selection_allowed && ui.is_enabled() && !ui.ctx().egui_wants_keyboard_input() {
            if ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::A)) {
                self.select_all(document.current_page());
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
                self.copy(ui.ctx(), document)?;
            }
        }
        let mut copy = false;
        response.context_menu(|ui| {
            if !copy_allowed {
                ui.label("This PDF does not allow copying text.");
            }
            if ui
                .add_enabled(
                    copy_allowed && self.anchor != self.end,
                    egui::Button::new("Copy"),
                )
                .clicked()
            {
                copy = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    selection_allowed && self.cache.values().any(|p| !p.chars.is_empty()),
                    egui::Button::new("Select all on page"),
                )
                .clicked()
            {
                let page = response
                    .interact_pointer_pos()
                    .and_then(nearest)
                    .map_or(document.current_page(), |(p, _)| *p);
                self.select_all(page);
                ui.close();
            }
        });
        if copy {
            self.copy(ui.ctx(), document)?;
        }
        for &(number, page) in pages {
            let Some(text) = self.cache.get(&number) else {
                continue;
            };
            for ch in &text.chars[self.range(number, text.chars.len())] {
                if let Some(quad) = ch.quad {
                    let points = quad.map(|p| page.screen(p));
                    ui.painter().add(egui::Shape::convex_polygon(
                        points.to_vec(),
                        Color32::from_rgba_unmultiplied(50, 125, 255, 85),
                        egui::Stroke::NONE,
                    ));
                }
            }
        }
        Ok(())
    }

    fn select_all(&mut self, page: usize) {
        self.anchor = Endpoint { page, caret: 0 };
        self.end = Endpoint {
            page,
            caret: self.cache.get(&page).map_or(0, |text| text.chars.len()),
        };
    }

    fn range(&self, page: usize, len: usize) -> Range<usize> {
        let start = self.anchor.min(self.end);
        let end = self.anchor.max(self.end);
        if page < start.page || page > end.page {
            return 0..0;
        }
        let a = if page == start.page { start.caret } else { 0 };
        let b = if page == end.page { end.caret } else { len };
        a.min(len)..b.min(len)
    }

    fn copy(&self, ctx: &egui::Context, document: &PdfDocument) -> Result<()> {
        // Check again at the extraction boundary, before touching any page.
        if document.permissions().copy && self.anchor != self.end {
            let mut output = String::new();
            let start = self.anchor.min(self.end).page;
            let end = self.anchor.max(self.end).page;
            for page in start..=end {
                let extracted;
                let text = if let Some(text) = self.cache.get(&page) {
                    text
                } else {
                    extracted = document.structured_text(page)?;
                    &extracted
                };
                if page != start {
                    output.push('\n');
                }
                output.push_str(&text.text(self.range(page, text.chars.len())));
            }
            ctx.copy_text(output);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_joins_glyphs_per_line_without_losing_rotation_or_reversed_order() {
        use crate::structured_text::{TextChar, TextLine};
        let mut selection = Selection {
            end: Endpoint { page: 0, caret: 3 },
            ..Default::default()
        };
        let quads = [
            [[0.3, 0.6], [0.3, 0.55], [0.25, 0.55], [0.25, 0.6]],
            [[0.3, 0.7], [0.3, 0.65], [0.25, 0.65], [0.25, 0.7]],
            [[0.3, 0.5], [0.3, 0.45], [0.25, 0.45], [0.25, 0.5]],
        ];
        let text = selection.cache.entry(0).or_default();
        text.chars = quads
            .into_iter()
            .map(|quad| TextChar {
                ch: 'a',
                quad: Some(quad),
                bidi: 0,
            })
            .collect();
        text.lines = vec![TextLine {
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
        selection.cache.get_mut(&0).unwrap().lines = vec![
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
                    selection.cache[&0].text(selection.range(0, selection.cache[&0].chars.len())),
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
        selection.end.caret = 5;
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
        assert!(selection.anchor != selection.end);
        let denied = frame(
            &mut selection,
            &ctx,
            &document,
            vec![command(Key::A), egui::Event::Copy],
            1.0,
            false,
        );
        assert!(clipboard(denied).is_none());
        assert!(selection.anchor != selection.end);
        assert!(!selection.quads(0).is_empty());
        assert!(selection.cache.contains_key(&0));
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
    fn copy_streams_uncached_endpoints_in_document_order_and_denial_never_extracts() {
        let document = crate::document::tests::sample_document();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("denied.pdf");
        crate::document::tests::encrypted_fixture(
            &path,
            "",
            mupdf::pdf::Permission::empty(),
            mupdf::pdf::Encryption::Aes256,
        );
        let denied = PdfDocument::open(&path).unwrap();
        assert!(!denied.permissions().copy && !denied.permissions().annotate);
        let ctx = egui::Context::default();
        let a = Endpoint { page: 0, caret: 2 };
        let b = Endpoint { page: 1, caret: 5 };
        for (anchor, end) in [(a, b), (b, a)] {
            let mut selection = Selection {
                anchor,
                end,
                ..Default::default()
            };
            let output = ctx.run_ui(Default::default(), |ui| {
                selection.copy(ui.ctx(), &document).unwrap()
            });
            assert_eq!(
                clipboard(output).as_deref(),
                Some("pha alpha\nNeedle\nphrase\nLast ")
            );
            assert!(selection.cache.is_empty());
            let output = ctx.run_ui(Default::default(), |ui| {
                selection
                    .pages_ui(
                        ui,
                        &denied,
                        &[(
                            9999,
                            PageTransform {
                                rect: Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(300.0)),
                                rotation: Default::default(),
                            },
                        )],
                        false,
                        true,
                    )
                    .unwrap()
            });
            assert!(clipboard(output).is_none());
            assert!(selection.cache.is_empty());
            assert!(selection.anchor == selection.end);
        }
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
        assert!(selection.cache[&0].chars.is_empty());
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
        assert!(selection.anchor == selection.end);
        assert_eq!(selection.cache[&0].plain_text(), "violet river");
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
        assert_eq!(selection.cache[&0].plain_text(), "violet river");
        assert!(!selection.quads(0).is_empty());
    }
}
