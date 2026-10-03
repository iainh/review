//! An explicit text alternative to the raster page, with egui text accessibility.
use egui::{Id, Key, Modifiers};

use crate::document::PdfDocument;

#[derive(Default)]
pub struct PageText {
    pub open: bool,
    pub field_id: Option<Id>,
    cached: Option<(usize, u64, Result<String, String>)>,
    focus: bool,
}

impl PageText {
    pub fn invalidate(&mut self) {
        self.cached = None;
    }

    pub fn toggle(&mut self, ctx: &egui::Context) {
        self.open = !self.open;
        self.focus = self.open;
        if !self.open {
            let id = self.field_id.unwrap_or_else(|| Id::new("page_text"));
            ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
    }

    pub fn ui(&mut self, root: &mut egui::Ui, document: &PdfDocument) {
        if !self.open {
            return;
        }
        let field_id = self.field_id.unwrap_or_else(|| Id::new("page_text"));
        let copy_allowed = document.permissions().copy;
        if !copy_allowed && (self.focus || root.ctx().memory(|memory| memory.has_focus(field_id))) {
            root.input_mut(|input| {
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Copy | egui::Event::Cut))
            });
        }
        let page = document.current_page();
        let revision = document.text_revision();
        if self
            .cached
            .as_ref()
            .map(|(page, revision, _)| (*page, *revision))
            != Some((page, revision))
        {
            self.cached = Some((
                page,
                revision,
                document
                    .page_text(page)
                    .map_err(|error| format!("Could not extract page text: {error:#}")),
            ));
        }
        egui::Panel::right("page_text_panel")
            .default_size(340.0)
            .size_range(240.0..=600.0)
            .resizable(true)
            .show_inside(root, |ui| {
                let label = ui.heading(format!("Page {} text", page + 1));
                ui.label("Extracted order may differ from reading order.");
                if !copy_allowed {
                    ui.label("Copying is disabled by this PDF. Assistive reading remains available.");
                }
                if ui.button("Close page text").clicked() {
                    self.open = false;
                }
                ui.separator();
                match &self.cached.as_ref().unwrap().2 {
                    Ok(text) => {
                        if text.trim().is_empty() {
                            ui.label("No text on this page yet. For a scan, open OCR, choose an installed language and recognize this page. Text stays in this session.");
                        }
                        egui::ScrollArea::both().show(ui, |ui| {
                            let mut read_only = text.as_str();
                            let field = ui.add(
                                egui::TextEdit::multiline(&mut read_only)
                                    .id(field_id)
                                    .desired_width(f32::INFINITY),
                            ).labelled_by(label.id);
                            // The immutable buffer prevents editing. Expose that
                            // fact explicitly; egui's text role remains navigable.
                            ui.ctx().accesskit_node_builder(field.id, |node| node.set_read_only());
                            if self.focus && ui.is_enabled() {
                                field.request_focus();
                                ui.ctx().request_repaint();
                                self.focus = false;
                            }
                            if field.has_focus() {
                                ui.painter().rect_stroke(
                                    field.rect,
                                    ui.visuals().widgets.active.corner_radius,
                                    ui.visuals().selection.stroke,
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if ui.is_enabled() && (field.has_focus() || field.lost_focus())
                                && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
                            {
                                field.surrender_focus();
                                self.open = false;
                            }
                        });
                    }
                    Err(error) => { ui.colored_label(ui.visuals().error_fg_color, error); }
                }
            });
        if !self.open {
            root.ctx()
                .memory_mut(|memory| memory.surrender_focus(field_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Key, Modifiers, OutputCommand, RawInput};
    use mupdf::pdf::{Encryption, Permission};

    #[test]
    fn restricted_pdfs_expose_readable_text_but_only_owner_access_allows_copy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restricted.pdf");
        crate::document::tests::encrypted_fixture(
            &path,
            "",
            Permission::empty(),
            Encryption::Aes256,
        );
        for password in [None, Some("owner-secret")] {
            let document = PdfDocument::open_with_password(&path, password)
                .unwrap()
                .unwrap();
            let ctx = Context::default();
            ctx.enable_accesskit();
            let mut pane = PageText::default();
            pane.toggle(&ctx);
            let mut frame = |events| {
                ctx.run_ui(
                    RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960.0, 720.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| pane.ui(ui, &document),
                )
            };
            frame(vec![]);
            let output = frame(vec![]);
            assert!(
                output
                    .platform_output
                    .accesskit_update
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|(_, node)| node
                        .value()
                        .is_some_and(|value| value.contains("Chapter one")))
            );
            frame(vec![Event::Key {
                key: Key::A,
                modifiers: Modifiers::COMMAND,
                physical_key: None,
                pressed: true,
                repeat: false,
            }]);
            for event in [Event::Copy, Event::Cut] {
                let output = frame(vec![event]);
                let copied: Vec<_> = output
                    .platform_output
                    .commands
                    .iter()
                    .filter_map(|command| match command {
                        OutputCommand::CopyText(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                if password.is_some() {
                    assert_eq!(copied, ["Chapter one"]);
                } else {
                    assert!(copied.is_empty());
                }
            }
        }
    }

    #[test]
    fn same_page_ocr_refreshes_readable_accesskit_text_without_bypassing_copy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan.pdf");
        std::fs::write(&path, crate::document::tests::sample_pdf("", false)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let pdf = mupdf::pdf::PdfDocument::try_from(
            mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap(),
        )
        .unwrap();
        let mut options = mupdf::pdf::PdfWriteOptions::default();
        options
            .set_encryption(Encryption::Aes256)
            .set_user_password("")
            .set_owner_password("owner-secret")
            .set_permissions(Permission::empty());
        let restricted = directory.path().join("restricted.pdf");
        pdf.save_with_options(restricted.to_str().unwrap(), options)
            .unwrap();
        for path in [&path, &restricted] {
            let document = PdfDocument::open(path).unwrap();
            let ctx = Context::default();
            ctx.enable_accesskit();
            let mut pane = PageText::default();
            pane.toggle(&ctx);
            let mut frame = |events| {
                ctx.run_ui(
                    RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960.0, 720.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| pane.ui(ui, &document),
                )
            };
            frame(vec![]);
            for word in ["amber fox", "violet river"] {
                document
                    .set_recognized_text(0, crate::ocr::tests::word_text(word))
                    .unwrap();
                let output = frame(vec![]);
                assert!(
                    output
                        .platform_output
                        .accesskit_update
                        .unwrap()
                        .nodes
                        .iter()
                        .any(|(_, node)| node.value() == Some(word)),
                    "same-page OCR must refresh assistive text"
                );
                frame(vec![Event::Key {
                    key: Key::A,
                    modifiers: Modifiers::COMMAND,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                }]);
                for event in [Event::Copy, Event::Cut] {
                    let output = frame(vec![event]);
                    let copied: Vec<_> = output
                        .platform_output
                        .commands
                        .iter()
                        .filter_map(|command| match command {
                            OutputCommand::CopyText(text) => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    if document.permissions().copy {
                        assert_eq!(copied, [word]);
                    } else {
                        assert!(copied.is_empty());
                    }
                }
            }
            document.invalidate_text();
            frame(vec![]);
            assert!(pane.cached.as_ref().unwrap().2.as_ref().unwrap().is_empty());
        }
    }
}
