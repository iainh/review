use anyhow::{Context, Result, ensure};
use mupdf::{
    Point, Quad, Rect,
    color::AnnotationColor,
    pdf::{AnnotationFlags, PdfAnnotation, PdfAnnotationType},
};

use crate::{document::PdfDocument, structured_text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Highlight,
    Underline,
    StrikeOut,
    Note,
    Ink,
}

impl Kind {
    pub const ALL: [Self; 5] = [
        Self::Highlight,
        Self::Underline,
        Self::StrikeOut,
        Self::Note,
        Self::Ink,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::StrikeOut => "Strike-through",
            Self::Note => "Note",
            Self::Ink => "Ink",
        }
    }

    fn pdf_type(self) -> PdfAnnotationType {
        match self {
            Self::Highlight => PdfAnnotationType::Highlight,
            Self::Underline => PdfAnnotationType::Underline,
            Self::StrikeOut => PdfAnnotationType::StrikeOut,
            Self::Note => PdfAnnotationType::Text,
            Self::Ink => PdfAnnotationType::Ink,
        }
    }

    fn from_pdf(kind: PdfAnnotationType) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.pdf_type() == kind)
    }

    pub fn markup(self) -> bool {
        matches!(self, Self::Highlight | Self::Underline | Self::StrikeOut)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    Markup(Vec<structured_text::Quad>),
    Note([f32; 2]),
    Ink(Vec<Vec<[f32; 2]>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub xref: i32,
    pub kind: Kind,
    /// Normalized display bounds, only used for hit testing and selection.
    pub bounds: [f32; 4],
    pub contents: String,
    pub colour: [f32; 3],
    pub width: f32,
    pub editable: bool,
}

fn writable(annotation: &PdfAnnotation) -> Result<bool> {
    Ok(!annotation.flags()?.intersects(
        AnnotationFlags::IS_READ_ONLY
            | AnnotationFlags::IS_LOCKED
            | AnnotationFlags::IS_LOCKED_CONTENTS,
    ))
}

impl PdfDocument {
    pub fn annotations(&self, number: usize) -> Result<Vec<Annotation>> {
        ensure!(number < self.page_count(), "page is out of range");
        let page = self.document.load_pdf_page(number as i32)?;
        let bounds = page.bounds()?;
        let mut result = Vec::new();
        for annotation in page.annotations() {
            let Some(kind) = Kind::from_pdf(annotation.r#type()?) else {
                continue;
            };
            let rect = annotation.bounds()?;
            let colour = match annotation.color()? {
                Some(AnnotationColor::Rgb { red, green, blue }) => [red, green, blue],
                Some(AnnotationColor::Gray(g)) => [g; 3],
                Some(AnnotationColor::Cmyk {
                    cyan,
                    magenta,
                    yellow,
                    key,
                }) => [
                    1.0 - (cyan + key).min(1.0),
                    1.0 - (magenta + key).min(1.0),
                    1.0 - (yellow + key).min(1.0),
                ],
                None => [0.0; 3],
            };
            result.push(Annotation {
                xref: annotation.xref()?,
                kind,
                bounds: [
                    (rect.x0 - bounds.x0) / bounds.width(),
                    (rect.y0 - bounds.y0) / bounds.height(),
                    (rect.x1 - bounds.x0) / bounds.width(),
                    (rect.y1 - bounds.y0) / bounds.height(),
                ],
                contents: annotation.contents()?.unwrap_or("").to_owned(),
                colour,
                width: if kind == Kind::Ink {
                    annotation.border_width()?
                } else {
                    2.0
                },
                editable: self.permissions().annotate && writable(&annotation)?,
            });
        }
        Ok(result)
    }

    pub fn add_annotation(
        &mut self,
        number: usize,
        kind: Kind,
        geometry: Geometry,
        contents: &str,
        colour: [f32; 3],
        width: f32,
    ) -> Result<()> {
        ensure!(
            self.permissions().annotate,
            "This PDF does not allow annotations"
        );
        ensure!(number < self.page_count(), "page is out of range");
        validate_style(contents, colour, width)?;
        validate_geometry(kind, &geometry)?;
        self.edit(|document| {
            let mut page = document.load_pdf_page(number as i32)?;
            let bounds = page.bounds()?;
            let point = |p: [f32; 2]| {
                Point::new(
                    bounds.x0 + p[0] * bounds.width(),
                    bounds.y0 + p[1] * bounds.height(),
                )
            };
            let mut annotation = page.create_annotation(kind.pdf_type())?;
            annotation.set_flags(AnnotationFlags::IS_PRINT)?;
            match geometry {
                Geometry::Markup(quads) => annotation.set_quad_points(
                    quads
                        .into_iter()
                        .map(|q| Quad::new(point(q[0]), point(q[1]), point(q[3]), point(q[2])))
                        .collect::<Vec<_>>(),
                )?,
                Geometry::Note(p) => {
                    let p = point(p);
                    annotation.set_rect(Rect::new(p.x, p.y, p.x + 20.0, p.y + 20.0))?;
                    annotation.set_icon_name("Note")?;
                }
                Geometry::Ink(strokes) => annotation.set_ink_list(
                    strokes
                        .into_iter()
                        .map(|s| s.into_iter().map(point).collect::<Vec<_>>()),
                )?,
            }
            set_style(&mut annotation, contents, colour, width)?;
            annotation.update()?;
            Ok(())
        })
    }

    pub fn update_annotation(
        &mut self,
        number: usize,
        xref: i32,
        contents: &str,
        colour: [f32; 3],
        width: f32,
    ) -> Result<()> {
        ensure!(
            self.permissions().annotate,
            "This PDF does not allow annotations"
        );
        ensure!(number < self.page_count(), "page is out of range");
        validate_style(contents, colour, width)?;
        self.edit(|document| {
            let page = document.load_pdf_page(number as i32)?;
            let mut annotation = page
                .annotations()
                .find(|a| a.xref().ok() == Some(xref))
                .context("Annotation no longer exists")?;
            ensure!(
                Kind::from_pdf(annotation.r#type()?).is_some() && writable(&annotation)?,
                "Annotation is read-only or unsupported"
            );
            set_style(&mut annotation, contents, colour, width)?;
            annotation.update()?;
            Ok(())
        })
    }

    pub fn delete_annotation(&mut self, number: usize, xref: i32) -> Result<()> {
        ensure!(
            self.permissions().annotate,
            "This PDF does not allow annotations"
        );
        ensure!(number < self.page_count(), "page is out of range");
        self.edit(|document| {
            let mut page = document.load_pdf_page(number as i32)?;
            let annotation = page
                .annotations()
                .find(|a| a.xref().ok() == Some(xref))
                .context("Annotation no longer exists")?;
            ensure!(
                Kind::from_pdf(annotation.r#type()?).is_some() && writable(&annotation)?,
                "Annotation is read-only or unsupported"
            );
            page.delete_annotation(annotation)?;
            Ok(())
        })
    }
}

fn set_style(
    annotation: &mut PdfAnnotation,
    contents: &str,
    colour: [f32; 3],
    width: f32,
) -> Result<()> {
    annotation.set_contents(contents)?;
    annotation.set_color(AnnotationColor::Rgb {
        red: colour[0],
        green: colour[1],
        blue: colour[2],
    })?;
    if annotation.r#type()? == PdfAnnotationType::Ink {
        annotation.set_border_width(width)?;
    }
    Ok(())
}

fn validate_style(contents: &str, colour: [f32; 3], width: f32) -> Result<()> {
    ensure!(
        !contents.contains('\0'),
        "Note contains an invalid null character"
    );
    ensure!(
        colour
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
        "Invalid annotation colour"
    );
    ensure!(
        width.is_finite() && (0.5..=20.0).contains(&width),
        "Ink width must be between 0.5 and 20 points"
    );
    Ok(())
}

fn validate_geometry(kind: Kind, geometry: &Geometry) -> Result<()> {
    let valid_point = |p: &[f32; 2]| p.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v));
    let valid = match geometry {
        Geometry::Markup(quads) => {
            kind.markup() && !quads.is_empty() && quads.iter().all(|q| q.iter().all(valid_point))
        }
        Geometry::Note(p) => kind == Kind::Note && valid_point(p),
        Geometry::Ink(strokes) => {
            kind == Kind::Ink
                && !strokes.is_empty()
                && strokes
                    .iter()
                    .all(|s| s.len() >= 2 && s.iter().all(valid_point))
        }
    };
    ensure!(valid, "Invalid annotation geometry");
    Ok(())
}

pub struct AnnotationUi {
    pub open: bool,
    pub tool: Option<Kind>,
    page: Option<usize>,
    entries: Vec<Annotation>,
    selected: Option<i32>,
    contents: String,
    colour: [f32; 3],
    width: f32,
    stroke: Vec<[f32; 2]>,
}

impl Default for AnnotationUi {
    fn default() -> Self {
        Self {
            open: false,
            tool: None,
            page: None,
            entries: Vec::new(),
            selected: None,
            contents: String::new(),
            colour: [1.0, 0.8, 0.0],
            width: 2.0,
            stroke: Vec::new(),
        }
    }
}

impl AnnotationUi {
    pub fn invalidate(&mut self) {
        self.page = None;
        self.stroke.clear();
    }

    fn refresh(&mut self, document: &PdfDocument) -> Result<()> {
        let page = document.current_page();
        if self.page != Some(page) {
            let entries = document.annotations(page)?;
            if let Some(entry) = entries.iter().find(|a| Some(a.xref) == self.selected) {
                self.contents = entry.contents.clone();
                self.colour = entry.colour;
                self.width = entry.width.clamp(0.5, 20.0);
            } else {
                self.selected = None;
            }
            self.entries = entries;
            self.page = Some(page);
            self.stroke.clear();
        }
        Ok(())
    }

    pub fn add_markup(
        &mut self,
        document: &mut PdfDocument,
        kind: Kind,
        quads: Vec<structured_text::Quad>,
    ) -> Result<()> {
        document.add_annotation(
            document.current_page(),
            kind,
            Geometry::Markup(quads),
            "",
            self.colour,
            self.width,
        )?;
        self.open = true;
        self.invalidate();
        Ok(())
    }

    pub fn panel(
        &mut self,
        root: &mut egui::Ui,
        document: &mut PdfDocument,
        mut before_edit: impl FnMut(),
    ) -> Result<bool> {
        if !self.open {
            return Ok(false);
        }
        self.refresh(document)?;
        let mut apply = false;
        let mut delete = false;
        egui::Panel::right("annotations").default_size(260.0).size_range(220.0..=380.0).show_inside(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Annotations");
                if ui.button("Close").clicked() { self.open = false; self.tool = None; self.stroke.clear(); }
            });
            ui.label(format!("Page {}", document.current_page() + 1));
            if !document.permissions().annotate {
                ui.label("This PDF does not allow annotation changes.");
            }
            ui.add_enabled_ui(document.permissions().annotate, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.tool.is_none(), "Select").clicked() { self.tool = None; self.stroke.clear(); }
                    for kind in [Kind::Note, Kind::Ink] {
                        if ui.selectable_label(self.tool == Some(kind), kind.label()).clicked() {
                            self.tool = Some(kind);
                            self.selected = None;
                            self.contents.clear();
                            self.stroke.clear();
                        }
                    }
                });
            });
            match self.tool {
                Some(Kind::Note) => { ui.label("Click the page to place a note."); },
                Some(Kind::Ink) => { ui.label("Drag on the page to draw a stroke."); },
                _ => { ui.label("Select text, then use a markup button. Select an annotation below to edit it."); },
            }
            ui.separator();
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                if self.entries.is_empty() { ui.label("No supported annotations on this page."); }
                for entry in &self.entries {
                    let label = format!("{} · {}{}", entry.kind.label(), entry.contents.lines().next().filter(|s| !s.is_empty()).unwrap_or("(no comment)"), if entry.editable { "" } else { " · read-only" });
                    if ui.selectable_label(self.selected == Some(entry.xref), label).clicked() {
                        self.selected = Some(entry.xref);
                        self.tool = None;
                        self.contents = entry.contents.clone();
                        self.colour = entry.colour;
                        self.width = entry.width.clamp(0.5, 20.0);
                    }
                }
            });
            ui.separator();
            let editable = self.selected.and_then(|id| self.entries.iter().find(|a| a.xref == id)).map_or(document.permissions().annotate, |a| a.editable);
            ui.add_enabled_ui(editable, |ui| {
                let comment_label = ui.label("Comment");
                ui.add(egui::TextEdit::multiline(&mut self.contents).desired_rows(5).desired_width(f32::INFINITY).id_salt("annotation_comment")).labelled_by(comment_label.id);
                ui.horizontal(|ui| { let label = ui.label("Colour"); ui.color_edit_button_rgb(&mut self.colour).labelled_by(label.id); });
                if self.tool == Some(Kind::Ink) || self.selected.is_some_and(|id| self.entries.iter().any(|a| a.xref == id && a.kind == Kind::Ink)) {
                    ui.add(egui::Slider::new(&mut self.width, 0.5..=20.0).text("Ink width (pt)"));
                }
                ui.horizontal(|ui| {
                    apply = ui.add_enabled(self.selected.is_some(), egui::Button::new("Apply changes")).clicked();
                    delete = ui.add_enabled(self.selected.is_some(), egui::Button::new("Delete")).clicked();
                });
            });
        });
        if let Some(xref) = self.selected {
            if apply || delete {
                before_edit();
            }
            if apply {
                document.update_annotation(
                    document.current_page(),
                    xref,
                    &self.contents,
                    self.colour,
                    self.width,
                )?;
            }
            if delete {
                document.delete_annotation(document.current_page(), xref)?;
                self.selected = None;
            }
        }
        if apply || delete {
            self.invalidate();
        }
        Ok(apply || delete)
    }

    pub fn page_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &mut PdfDocument,
        page: egui::Rect,
        mut before_edit: impl FnMut(),
    ) -> Result<bool> {
        if !self.open {
            return Ok(false);
        }
        if !ui.is_enabled() {
            self.stroke.clear();
            return Ok(false);
        }
        self.refresh(document)?;
        let point = |p: egui::Pos2| {
            [
                ((p.x - page.min.x) / page.width()).clamp(0.0, 1.0),
                ((p.y - page.min.y) / page.height()).clamp(0.0, 1.0),
            ]
        };
        let screen = |p: [f32; 2]| page.min + egui::vec2(p[0] * page.width(), p[1] * page.height());
        if let Some(tool) = self.tool {
            let response = ui.interact(
                page,
                ui.id().with("annotation_canvas"),
                egui::Sense::click_and_drag(),
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if tool == Kind::Note
                && response.clicked()
                && let Some(p) = response.interact_pointer_pos()
            {
                before_edit();
                document.add_annotation(
                    document.current_page(),
                    tool,
                    Geometry::Note(point(p)),
                    &self.contents,
                    self.colour,
                    self.width,
                )?;
                self.tool = None;
                self.invalidate();
                self.refresh(document)?;
                self.selected = self.entries.last().map(|a| a.xref);
                return Ok(true);
            }
            if tool == Kind::Ink {
                if response.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
                    self.stroke.clear();
                    if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                        self.stroke.push(point(p));
                    }
                }
                if response.dragged()
                    && let Some(p) = response.interact_pointer_pos()
                {
                    let p = point(p);
                    if self.stroke.last() != Some(&p) {
                        self.stroke.push(p);
                    }
                }
                if ui.input(|i| i.pointer.primary_released()) && self.stroke.len() >= 2 {
                    let stroke = std::mem::take(&mut self.stroke);
                    before_edit();
                    document.add_annotation(
                        document.current_page(),
                        Kind::Ink,
                        Geometry::Ink(vec![stroke]),
                        &self.contents,
                        self.colour,
                        self.width,
                    )?;
                    self.invalidate();
                    return Ok(true);
                }
                let colour = egui::Color32::from_rgb(
                    (self.colour[0] * 255.0) as u8,
                    (self.colour[1] * 255.0) as u8,
                    (self.colour[2] * 255.0) as u8,
                );
                if self.stroke.len() >= 2 {
                    ui.painter().add(egui::Shape::line(
                        self.stroke.iter().copied().map(screen).collect(),
                        egui::Stroke::new(
                            self.width * page.width()
                                / document.page_size(document.current_page())?.0,
                            colour,
                        ),
                    ));
                }
            }
        } else {
            for entry in &self.entries {
                let [x0, y0, x1, y1] = entry.bounds;
                let rect = egui::Rect::from_min_max(screen([x0, y0]), screen([x1, y1]));
                let response = ui.interact(
                    rect.intersect(page),
                    ui.id().with(("annotation", entry.xref)),
                    egui::Sense::click(),
                );
                if response.clicked() {
                    self.selected = Some(entry.xref);
                    self.contents = entry.contents.clone();
                    self.colour = entry.colour;
                    self.width = entry.width.clamp(0.5, 20.0);
                }
                response.on_hover_text(format!("{}: {}", entry.kind.label(), entry.contents));
                if self.selected == Some(entry.xref) {
                    ui.painter().rect_stroke(
                        rect.expand(3.0),
                        0.0,
                        egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(45, 130, 240)),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::tests::{encrypted_fixture, sample_document, sample_pdf};
    use mupdf::pdf::{Encryption, Permission};

    #[test]
    fn disabled_canvas_discards_partial_ink_without_editing() {
        let mut document = sample_document();
        let mut annotations = AnnotationUi {
            open: true,
            tool: Some(Kind::Ink),
            page: Some(0),
            stroke: vec![[0.2, 0.3], [0.7, 0.6]],
            ..Default::default()
        };
        let context = egui::Context::default();
        let mut input = egui::RawInput::default();
        for pressed in [true, false] {
            input.events.push(egui::Event::PointerButton {
                pos: egui::pos2(150.0, 100.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let _ = context.run_ui(input, |root| {
            assert!(root.input(|i| i.pointer.primary_released()));
            root.disable();
            assert!(
                !annotations
                    .page_ui(root, &mut document, root.max_rect(), || {})
                    .unwrap()
            );
        });
        assert!(annotations.stroke.is_empty());
        assert!(!document.is_dirty());
        assert!(document.annotations(0).unwrap().is_empty());
    }

    fn add_note(document: &mut PdfDocument, comment: &str) {
        document
            .add_annotation(
                0,
                Kind::Note,
                Geometry::Note([0.27, 0.63]),
                comment,
                [0.9, 0.2, 0.1],
                2.0,
            )
            .unwrap();
    }

    #[test]
    fn layer_visibility_survives_edits_and_undo_but_never_enters_saved_source() {
        use crate::inspection::{Inspection, tests::fixture};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layers.pdf");
        std::fs::write(&path, fixture()).unwrap();
        let mut document = PdfDocument::open(&path).unwrap();
        let mut layers = Inspection::read(document.pdf()).layers.unwrap();
        layers[0].enabled = false;
        document.set_layer_visibility(&layers).unwrap();
        assert!(!document.is_dirty());
        drop(layers);
        add_note(&mut document, "visible despite hidden draft layer");
        assert!(document.structured_text(0).unwrap().plain_text().is_empty());
        let worker = document.worker_source().open().unwrap();
        assert!(worker.structured_text(0).unwrap().plain_text().is_empty());
        assert_eq!(worker.annotations(0).unwrap().len(), 1);
        document.undo().unwrap();
        assert!(document.structured_text(0).unwrap().plain_text().is_empty());
        assert!(document.annotations(0).unwrap().is_empty());
        document.redo().unwrap();
        assert!(document.structured_text(0).unwrap().plain_text().is_empty());
        document.save(&path).unwrap();
        let saved = PdfDocument::open(&path).unwrap();
        assert_eq!(saved.structured_text(0).unwrap().plain_text(), "Draft text");
        assert_eq!(saved.annotations(0).unwrap().len(), 1);
        let inspected = Inspection::read(saved.pdf());
        assert!(inspected.layers.unwrap()[0].enabled);
        assert_eq!(inspected.attachments.unwrap().len(), 1);
        assert!(document.structured_text(0).unwrap().plain_text().is_empty());
    }

    #[test]
    fn all_annotation_types_round_trip_geometry_comments_appearances_and_undo() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("annotations.pdf");
        let mut document = sample_document();
        let before = document.render_at_scale(0, 1.0).unwrap().rgba;
        let print_pixels = |document: &PdfDocument| {
            document
                .print_page(0)
                .unwrap()
                .to_pixmap(
                    &mupdf::Matrix::IDENTITY,
                    &mupdf::Colorspace::device_rgb(),
                    false,
                    true,
                )
                .unwrap()
                .samples()
                .to_vec()
        };
        let print_before = print_pixels(&document);
        // A skewed quad catches swapped lower corners and missing normalization.
        let quad = [[0.12, 0.16], [0.63, 0.19], [0.62, 0.24], [0.11, 0.21]];
        for kind in [Kind::Highlight, Kind::Underline, Kind::StrikeOut] {
            document
                .add_annotation(
                    0,
                    kind,
                    Geometry::Markup(vec![quad]),
                    kind.label(),
                    [0.8, 0.3, 0.1],
                    2.0,
                )
                .unwrap();
        }
        add_note(&mut document, "Note résumé 日本語");
        document
            .add_annotation(
                1,
                Kind::Ink,
                Geometry::Ink(vec![
                    vec![[0.18, 0.37], [0.53, 0.68], [0.81, 0.41]],
                    vec![[0.22, 0.31], [0.33, 0.29]],
                ]),
                "two strokes",
                [0.1, 0.2, 0.7],
                3.5,
            )
            .unwrap();
        assert!(document.is_dirty());
        assert_ne!(document.render_at_scale(0, 1.0).unwrap().rgba, before);
        assert_ne!(
            print_pixels(&document),
            print_before,
            "Printing must use unsaved annotations, not the original file"
        );
        let expected = document.annotations(0).unwrap();
        document.save(&path).unwrap();
        assert!(!document.is_dirty());
        let reopened = PdfDocument::open(&path).unwrap();
        assert_eq!(reopened.annotations(0).unwrap(), expected);
        assert_eq!(reopened.annotations(1).unwrap()[0].width, 3.5);
        assert_ne!(reopened.render_at_scale(0, 1.0).unwrap().rgba, before);
        let page = reopened.document.load_pdf_page(0).unwrap();
        for annotation in page.annotations().take(3) {
            let q = annotation.quad_points().unwrap().remove(0);
            assert!((q.ul.x - 36.0).abs() < 0.001);
            assert!((q.ur.y - 76.0).abs() < 0.001);
            assert!((q.ll.x - 33.0).abs() < 0.001);
            assert!((q.lr.y - 96.0).abs() < 0.001);
        }
        let page = reopened.document.load_pdf_page(1).unwrap();
        let ink = page.annotations().next().unwrap();
        let strokes = ink.ink_list().unwrap();
        assert_eq!(strokes.len(), 2);
        assert_eq!(strokes[0][0], Point::new(72.0, 74.0));
        assert_eq!(strokes[0][2], Point::new(324.0, 82.0));
        document.undo().unwrap();
        assert!(document.annotations(1).unwrap().is_empty());
        assert!(document.is_dirty());
        document.redo().unwrap();
        assert!(!document.is_dirty());
        let note = expected.last().unwrap();
        document
            .update_annotation(0, note.xref, "Revised note", [0.0, 0.5, 0.7], 2.0)
            .unwrap();
        assert_eq!(
            document.annotations(0).unwrap().last().unwrap().contents,
            "Revised note"
        );
        document.delete_annotation(0, note.xref).unwrap();
        assert_eq!(document.annotations(0).unwrap().len(), 3);
        document.undo().unwrap();
        assert_eq!(
            document.annotations(0).unwrap().last().unwrap().contents,
            "Revised note"
        );
        document.undo().unwrap();
        assert!(!document.is_dirty());
        assert_eq!(
            document.annotations(0).unwrap().last().unwrap().contents,
            "Note résumé 日本語"
        );
    }

    #[test]
    fn annotation_permission_is_independent_of_copy_and_encryption_survives_save_as_and_replace() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("encrypted.pdf");
        let copy = directory.path().join("encrypted copy.pdf");
        for encryption in [Encryption::Aes256, Encryption::Aes128, Encryption::Rc4_128] {
            encrypted_fixture(
                &path,
                "open-secret",
                Permission::ANNOTATE | Permission::PRINT,
                encryption,
            );
            let mut document = PdfDocument::open_with_password(&path, Some("open-secret"))
                .unwrap()
                .unwrap();
            let permissions = document.permissions();
            assert!(permissions.annotate && !permissions.copy);
            add_note(&mut document, "encrypted comment");
            document.save(&copy).unwrap();
            assert!(
                PdfDocument::open_with_password(&copy, None)
                    .unwrap()
                    .is_none()
            );
            assert!(
                PdfDocument::open_with_password(&copy, Some("wrong"))
                    .unwrap()
                    .is_none()
            );
            let reopened = PdfDocument::open_with_password(&copy, Some("open-secret"))
                .unwrap()
                .unwrap();
            assert_eq!(reopened.permissions(), permissions);
            assert_eq!(
                reopened.annotations(0).unwrap()[0].contents,
                "encrypted comment"
            );
            let owner = PdfDocument::open_with_password(&copy, Some("owner-secret"))
                .unwrap()
                .unwrap();
            assert!(owner.permissions().copy && owner.permissions().annotate);
            let mut raw = mupdf::Document::open(copy.to_str().unwrap()).unwrap();
            raw.authenticate("open-secret").unwrap();
            assert_eq!(
                raw.metadata(mupdf::MetadataName::Encryption).unwrap(),
                document
                    .document
                    .metadata(mupdf::MetadataName::Encryption)
                    .unwrap()
            );
            document.save(&path).unwrap();
            assert_eq!(
                PdfDocument::open_with_password(&path, Some("open-secret"))
                    .unwrap()
                    .unwrap()
                    .annotations(0)
                    .unwrap()
                    .len(),
                1
            );
            // These readers intentionally remain live while the original is replaced.
            assert_eq!(reopened.page_count(), 2);
            assert!(document.print_page(0).is_ok());
            encrypted_fixture(&path, "", Permission::COPY, encryption);
            let mut denied = PdfDocument::open(&path).unwrap();
            assert!(denied.permissions().copy && !denied.permissions().annotate);
            assert!(
                denied
                    .add_annotation(
                        0,
                        Kind::Note,
                        Geometry::Note([0.2, 0.3]),
                        "denied",
                        [1.0; 3],
                        2.0
                    )
                    .is_err()
            );
            assert!(!denied.is_dirty());
            assert!(denied.annotations(0).unwrap().is_empty());
        }
    }

    #[test]
    fn read_only_and_locked_annotations_cannot_be_edited_or_deleted() {
        for flag in [
            AnnotationFlags::IS_READ_ONLY,
            AnnotationFlags::IS_LOCKED,
            AnnotationFlags::IS_LOCKED_CONTENTS,
        ] {
            let mut document = sample_document();
            add_note(&mut document, "locked note");
            document
                .edit(|pdf| {
                    let page = pdf.load_pdf_page(0)?;
                    page.annotations().next().unwrap().set_flags(flag)?;
                    Ok(())
                })
                .unwrap();
            let annotation = document.annotations(0).unwrap().remove(0);
            assert!(!annotation.editable);
            assert!(
                document
                    .update_annotation(0, annotation.xref, "changed", [0.5; 3], 2.0)
                    .is_err()
            );
            assert!(document.delete_annotation(0, annotation.xref).is_err());
            assert_eq!(document.annotations(0).unwrap()[0].contents, "locked note");
        }
    }

    #[test]
    fn worker_snapshot_captures_unsaved_state_without_original_file_readers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.pdf");
        std::fs::write(&path, sample_pdf("", false)).unwrap();
        let mut document = PdfDocument::open(&path).unwrap();
        let old = document.worker_source();
        add_note(&mut document, "new state");
        let current = document.worker_source();
        std::fs::remove_file(&path).unwrap();
        std::thread::spawn(move || {
            assert!(old.open().unwrap().annotations(0).unwrap().is_empty());
            assert_eq!(
                current.open().unwrap().annotations(0).unwrap()[0].contents,
                "new state"
            );
        })
        .join()
        .unwrap();
        document.save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn successful_edits_and_undo_share_monotonic_ocr_invalidation_with_old_workers() {
        use crate::ocr::tests::word_text;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan.pdf");
        std::fs::write(&path, sample_pdf("", false)).unwrap();
        let mut document = PdfDocument::open(&path).unwrap();
        document
            .set_recognized_text(0, word_text("first scan"))
            .unwrap();
        let old_worker = document.worker_source().open().unwrap();
        add_note(&mut document, "mutation clears OCR");
        assert_eq!(document.text_revision(), 2);
        assert_eq!(old_worker.text_revision(), 2);
        assert!(
            old_worker
                .structured_text(0)
                .unwrap()
                .plain_text()
                .is_empty()
        );
        document
            .set_recognized_text(0, word_text("edited scan"))
            .unwrap();
        assert_eq!(
            old_worker.structured_text(0).unwrap().plain_text(),
            "edited scan"
        );
        assert_eq!(document.worker_source().open().unwrap().text_revision(), 3);
        document.undo().unwrap();
        assert_eq!(document.text_revision(), 4);
        assert_eq!(old_worker.text_revision(), 4);
        document
            .set_recognized_text(0, word_text("retry scan"))
            .unwrap();
        assert!(
            document
                .update_annotation(0, -1, "missing", [0.5; 3], 2.0)
                .is_err()
        );
        assert_eq!(document.text_revision(), 5);
        assert_eq!(
            old_worker.structured_text(0).unwrap().plain_text(),
            "retry scan"
        );
        document.redo().unwrap();
        assert_eq!(document.text_revision(), 6);
        assert_eq!(old_worker.text_revision(), 6);
        assert!(document.structured_text(0).unwrap().plain_text().is_empty());
        document
            .set_recognized_text(0, word_text("unsaved session text"))
            .unwrap();
        document.save(&path).unwrap();
        assert_eq!(document.text_revision(), 7);
        assert_eq!(
            document.structured_text(0).unwrap().plain_text(),
            "unsaved session text"
        );
        assert!(
            PdfDocument::open(&path)
                .unwrap()
                .structured_text(0)
                .unwrap()
                .plain_text()
                .is_empty()
        );
    }

    #[test]
    fn failed_edits_and_saves_keep_source_and_history_and_new_edit_discards_redo() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.pdf");
        let mut document = sample_document();
        add_note(&mut document, "first");
        assert!(
            document
                .update_annotation(0, -7, "absent", [0.5; 3], 2.0)
                .is_err()
        );
        assert!(
            document
                .add_annotation(
                    0,
                    Kind::Ink,
                    Geometry::Ink(vec![vec![[0.2, 0.3]]]),
                    "",
                    [0.5; 3],
                    2.0
                )
                .is_err()
        );
        assert_eq!(document.annotations(0).unwrap()[0].contents, "first");
        document.save(&path).unwrap();
        add_note(&mut document, "second");
        let saved = std::fs::read(&path).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(document.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert!(document.is_dirty());
        document.undo().unwrap();
        assert!(!document.is_dirty());
        add_note(&mut document, "third");
        assert!(!document.can_redo());
        assert!(document.redo().is_err());
        assert_eq!(document.annotations(0).unwrap().len(), 2);
    }

    #[test]
    #[ignore = "reopens native Wayland output from REVIEW_FIXTURE_DIR"]
    fn verify_native_annotation_output() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        let document = PdfDocument::open(directory.join("native-save.pdf")).unwrap();
        let entries = document.annotations(0).unwrap();
        assert_eq!(entries.len(), 6);
        assert_eq!(entries.iter().filter(|a| a.kind == Kind::Note).count(), 2);
        assert!(entries.iter().any(|a| a.contents == "Edited native note"));
        for kind in [Kind::Highlight, Kind::Underline, Kind::StrikeOut, Kind::Ink] {
            assert_eq!(entries.iter().filter(|a| a.kind == kind).count(), 1);
        }
        let page = document.document.load_pdf_page(0).unwrap();
        for annotation in page.annotations() {
            if Kind::from_pdf(annotation.r#type().unwrap()).is_some_and(Kind::markup) {
                assert_eq!(annotation.quad_points().unwrap().len(), 1);
            }
        }
        let source = PdfDocument::open(directory.join("annotations.pdf")).unwrap();
        assert_eq!(source.annotations(0).unwrap().len(), 1);
        assert_eq!(
            source.annotations(0).unwrap()[0].contents,
            "Edited native note"
        );
        let encrypted = PdfDocument::open(directory.join("annotate-only.pdf")).unwrap();
        assert!(!encrypted.permissions().copy && encrypted.permissions().annotate);
        assert_eq!(encrypted.annotations(0).unwrap()[0].kind, Kind::Highlight);
        assert!(
            PdfDocument::open_with_password(
                directory.join("annotate-only.pdf"),
                Some("owner-secret")
            )
            .unwrap()
            .unwrap()
            .permissions()
            .copy
        );
    }

    #[test]
    #[ignore = "exports annotation fixtures for native Wayland tests"]
    fn export_annotation_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        let path = directory.join("annotations.pdf");
        std::fs::write(
            &path,
            sample_pdf(
                "BT /F1 16 Tf 40 350 Td (Annotation sample text) Tj ET",
                false,
            ),
        )
        .unwrap();
        let mut document = PdfDocument::open(&path).unwrap();
        add_note(&mut document, "Existing note");
        document.save(&path).unwrap();
        encrypted_fixture(
            &directory.join("annotate-only.pdf"),
            "",
            Permission::ANNOTATE | Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
    }
}
