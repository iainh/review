use std::path::Path;

use egui::Color32;

use crate::{document::PdfDocument, inspection::Inspection};

#[derive(Default)]
pub struct Inspector {
    pub open: bool,
    tab: usize,
    data: Option<Inspection>,
    save_requested: Option<usize>,
    message: Option<String>,
}

impl Inspector {
    /// Release embedded-stream objects before replacing their source document.
    pub fn invalidate(&mut self) {
        self.data = None;
        self.save_requested = None;
        self.message = None;
    }

    pub fn ui(&mut self, ctx: &egui::Context, document: &mut PdfDocument) -> bool {
        if !self.open {
            return false;
        }
        let data = self.data.get_or_insert_with(|| {
            let mut data = Inspection::read(document.pdf());
            if let Ok(layers) = &mut data.layers {
                for layer in layers {
                    if let Some(enabled) = document.layer_visibility(layer.reference.xref()) {
                        layer.enabled = enabled;
                    }
                }
            }
            data
        });
        let mut changed = false;
        egui::Window::new("Document properties")
            .open(&mut self.open)
            .default_size(egui::vec2(640.0, 520.0))
            .min_size(egui::vec2(380.0, 280.0))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (index, label) in ["Properties", "Attachments", "Layers", "Signatures"].into_iter().enumerate() {
                        ui.selectable_value(&mut self.tab, index, label);
                    }
                });
                ui.separator();
                ui.small("Document scripts and file-launch actions are disabled.");
                ui.separator();
                egui::ScrollArea::vertical().id_salt(self.tab).auto_shrink([false, false]).show(ui, |ui| {
                    // Let long document-controlled values wrap in the window.
                    ui.set_max_width(ui.available_width());
                    match self.tab {
                        0 => {
                            row(ui, "File", &document.name());
                            row(ui, "Location", &document.path().display().to_string());
                            row(ui, "Physical pages", &document.page_count().to_string());
                            row(ui, "Current page", &document.page_description(document.current_page()));
                            if let Ok((w, h)) = document.page_size(document.current_page()) {
                                row(ui, "Page size", &format!("{w:.1} × {h:.1} pt ({:.1} × {:.1} mm)", w * 25.4 / 72.0, h * 25.4 / 72.0));
                            }
                            for (name, value) in &data.metadata {
                                match value { Ok(value) => row(ui, name, if value.is_empty() { "Not specified" } else { value }),
                                    Err(error) => row(ui, name, &format!("Unavailable: {error}")), }
                            }
                            ui.separator();
                            ui.small("Dates are shown as stored in the PDF. Page labels come from MuPDF (up to 127 UTF-8 bytes); physical page numbers remain authoritative.");
                        }
                        1 => {
                            ui.label("Attachments are untrusted. Save only files you intend to inspect; Review never opens them.");
                            ui.small("Lists the embedded-files name tree, including nested entries. Annotation-only and associated-file attachments are not enumerated.");
                            ui.separator();
                            match &data.attachments {
                                Ok(files) if files.is_empty() => { ui.label("No attachments in the embedded-files name tree."); }
                                Ok(files) => for (index, file) in files.iter().enumerate() {
                                    ui.push_id(index, |ui| {
                                        ui.strong(&file.filename);
                                        row(ui, "Entry", &file.name);
                                        if !file.description.is_empty() { row(ui, "Description", &file.description); }
                                        if !file.mime_type.is_empty() { row(ui, "Type", &file.mime_type); }
                                        row(ui, "Declared size", &file.size.map_or_else(|| "Unknown".into(), |size| format!("{size} bytes")));
                                        if ui.button("Save as…").clicked() { self.save_requested = Some(index); }
                                        ui.separator();
                                    });
                                },
                                Err(error) => { ui.colored_label(Color32::LIGHT_RED, error); }
                            }
                            ui.small("Saving is limited to 64 MiB. The binding decodes the complete stream in memory; its declared size may be inaccurate.");
                        }
                        2 => {
                            ui.label("Visibility changes apply only to this viewing session. The original PDF is not saved or modified.");
                            ui.small("Default layer configuration only. Locked layers and radio/usage-controlled configurations are read-only with this MuPDF binding.");
                            ui.separator();
                            match &mut data.layers {
                                Ok(layers) if layers.is_empty() => { ui.label("No optional-content layers."); }
                                Ok(layers) => {
                                    let mut toggled = None;
                                    for (index, layer) in layers.iter_mut().enumerate() {
                                        ui.push_id(index, |ui| {
                                            if ui.add_enabled(layer.controllable, egui::Checkbox::new(&mut layer.enabled, &layer.name)).changed() { toggled = Some(index); }
                                        });
                                    }
                                    if let Some(index) = toggled {
                                        match document.set_layer_visibility(layers) {
                                            Ok(()) => { changed = true; self.message = None; }
                                            Err(error) => {
                                                layers[index].enabled = !layers[index].enabled;
                                                self.message = Some(format!("Could not change layer visibility: {error:#}"));
                                            }
                                        }
                                    }
                                }
                                Err(error) => { ui.colored_label(Color32::LIGHT_RED, error); }
                            }
                        }
                        _ => {
                            ui.colored_label(Color32::YELLOW, "Signatures are not cryptographically verified.");
                            ui.label("Signature presence does not establish validity or trust. Digest, certificate chain, revocation, timestamp and changes after signing are not checked by Review.");
                            ui.small("Inspects AcroForm signature fields, including invisible fields. Document timestamps outside that field tree are not enumerated. Signer names and dates below are unverified claims.");
                            ui.separator();
                            match &data.signatures {
                                Ok(signatures) if signatures.is_empty() => { ui.label("No AcroForm signature fields found."); }
                                Ok(signatures) => for signature in signatures {
                                    ui.strong(&signature.name);
                                    ui.label(signature.status());
                                    for (name, value) in &signature.details { row(ui, name, value); }
                                    ui.separator();
                                },
                                Err(error) => { ui.colored_label(Color32::LIGHT_RED, error); }
                            }
                        }
                    }
                    if let Some(message) = &self.message { ui.separator(); ui.label(message); }
                });
            });
        changed
    }

    pub fn take_save_request(&mut self) -> Option<(usize, String)> {
        let index = self.save_requested.take()?;
        let file = self.data.as_ref()?.attachments.as_ref().ok()?.get(index)?;
        Some((index, file.suggested_name()))
    }

    pub fn save_attachment(&mut self, index: usize, path: &Path, document_path: &Path) {
        if let Some(data) = &self.data
            && let Ok(files) = &data.attachments
            && let Some(file) = files.get(index)
        {
            self.message = Some(match file.save(path, document_path) {
                Ok(()) => format!("Saved to {}. The file was not opened.", path.display()),
                Err(error) => format!("Could not save attachment: {error:#}"),
            });
        }
    }
}

fn row(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(150.0, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(150.0);
                ui.add(egui::Label::new(egui::RichText::new(name).strong()).wrap());
            },
        );
        ui.add(egui::Label::new(value).wrap().selectable(true));
    });
    ui.add_space(5.0);
}
