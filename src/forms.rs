//! Existing AcroForm values only. Never dispatch widget actions or enable JS.
use anyhow::{Context, Result, ensure};
use mupdf::pdf::{AnnotationFlags, FieldFlags, PdfObject, PdfWidget, WidgetType};

use crate::document::PdfDocument;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Checkbox,
    Radio,
    Combo,
    List,
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct Choice {
    pub export: String,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub xref: i32,
    pub label: String,
    pub kind: Kind,
    pub flags: FieldFlags,
    pub bounds: [f32; 4],
    pub editable: bool,
    pub value: String,
    pub checked: bool,
    pub on_value: Option<String>,
    pub choices: Vec<Choice>,
    pub selected: Vec<usize>,
    pub max_len: Option<usize>,
}

#[derive(Clone, Debug)]
pub enum Value {
    Text(String),
    Toggle(bool),
    Choice(Vec<usize>),
}

fn text(object: &PdfObject) -> Result<String> {
    // PDF strings use UTF-16BE with a BOM, UTF-8 with a BOM, or PDFDocEncoding.
    let bytes = object.as_bytes()?;
    if bytes.starts_with(&[0xfe, 0xff]) {
        ensure!(bytes.len() % 2 == 0, "Invalid UTF-16 field string");
        let units: Vec<_> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .collect();
        return Ok(String::from_utf16(&units)?);
    }
    if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return Ok(String::from_utf8(bytes.to_vec())?);
    }
    const LOW: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    const HIGH: [char; 31] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚',
        '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž',
    ];
    Ok(bytes
        .into_iter()
        .map(|b| match b {
            24..=31 => LOW[(b - 24) as usize],
            128..=158 => HIGH[(b - 128) as usize],
            160 => '€',
            127 | 159 | 173 => '�',
            _ => char::from(b),
        })
        .collect())
}

fn field_head(mut object: PdfObject) -> Result<PdfObject> {
    // Match MuPDF's field-group head: nearest direct /T, not inherited /T.
    for _ in 0..64 {
        if object.get_dict("T")?.is_some() {
            return Ok(object);
        }
        match object.get_dict("Parent")? {
            Some(parent) => object = parent,
            None => return Ok(object),
        }
    }
    anyhow::bail!("Invalid cyclic/deep form hierarchy")
}

fn set_button_states(
    mut object: PdfObject,
    value: &str,
    selected: Option<i32>,
    depth: usize,
) -> Result<()> {
    ensure!(depth < 64, "Invalid cyclic/deep button hierarchy");
    if let Some(kids) = object.get_dict("Kids")? {
        for kid in kids.array_iter()? {
            set_button_states(kid?, value, selected, depth + 1)?;
        }
    } else {
        let normal = object
            .get_dict("AP")?
            .map(|ap| ap.get_dict("N"))
            .transpose()?
            .flatten();
        let on = value != "Off"
            && selected.is_none_or(|xref| object.as_indirect().ok() == Some(xref))
            && normal
                .as_ref()
                .is_some_and(|n| n.get_dict(value).ok().flatten().is_some());
        let state = if on { value } else { "Off" };
        let previous = object.get_dict("AS")?.map(|s| s.as_name()).transpose()?;
        if previous.as_deref() != Some(state.as_bytes()) {
            let flags = object
                .get_dict("F")?
                .map(|f| f.as_int())
                .transpose()?
                .unwrap_or(0);
            ensure!(
                flags
                    & (AnnotationFlags::IS_READ_ONLY
                        | AnnotationFlags::IS_LOCKED
                        | AnnotationFlags::IS_LOCKED_CONTENTS)
                        .bits()
                    == 0,
                "A radio/checkbox group member is locked"
            );
            let field_flags = object
                .get_dict_inheritable("Ff")?
                .map(|f| f.as_int())
                .transpose()?
                .unwrap_or(0);
            ensure!(
                field_flags & FieldFlags::READ_ONLY.bits() == 0,
                "A radio/checkbox group member is read-only"
            );
            object.dict_put("AS", PdfObject::new_name(state)?)?;
        }
    }
    Ok(())
}

fn read_field(widget: &PdfWidget, page: mupdf::Rect, permitted: bool) -> Result<Field> {
    let object = widget.annotation().object();
    let flags = widget.field_flags()?;
    let annotation_flags = widget.annotation().flags()?;
    let kind = match widget.r#type()? {
        WidgetType::Text => Kind::Text,
        WidgetType::Checkbox => Kind::Checkbox,
        WidgetType::RadioButton => Kind::Radio,
        WidgetType::Combobox => Kind::Combo,
        WidgetType::Listbox => Kind::List,
        _ => Kind::Unsupported,
    };
    let mut choices = Vec::new();
    if matches!(kind, Kind::Combo | Kind::List)
        && let Some(options) = object.get_dict_inheritable("Opt")?
    {
        for option in options.array_iter()? {
            let option = option?;
            if option.is_array()? {
                choices.push(Choice {
                    export: text(&option.get_array(0)?.context("Missing choice export")?)?,
                    label: text(&option.get_array(1)?.context("Missing choice label")?)?,
                });
            } else {
                let label = text(&option)?;
                choices.push(Choice {
                    export: label.clone(),
                    label,
                });
            }
        }
    }
    let mut selected = Vec::new();
    if let Some(indices) = object.get_dict_inheritable("I")? {
        for index in indices.array_iter()? {
            let index = usize::try_from(index?.as_int()?)?;
            if index < choices.len() {
                selected.push(index);
            }
        }
    } else if let Some(value) = object.get_dict_inheritable("V")? {
        let values = if value.is_array()? {
            value
                .array_iter()?
                .map(|v| text(&v?))
                .collect::<Result<Vec<_>>>()?
        } else if value.is_string()? {
            vec![text(&value)?]
        } else {
            Vec::new()
        };
        selected = choices
            .iter()
            .enumerate()
            .filter(|(_, c)| values.contains(&c.export))
            .map(|(i, _)| i)
            .collect();
    }
    let mut on_value = None;
    if matches!(kind, Kind::Checkbox | Kind::Radio)
        && let Some(normal) = object
            .get_dict("AP")?
            .and_then(|ap| ap.get_dict("N").ok().flatten())
        && normal.is_dict()?
    {
        for pair in normal.dict_iter()? {
            let (key, _) = pair?;
            let name = String::from_utf8(key.as_name()?)?;
            if name != "Off" {
                on_value = Some(name);
                break;
            }
        }
    }
    let checked = object
        .get_dict("AS")?
        .map(|state| state.as_name())
        .transpose()?
        .is_some_and(|s| on_value.as_ref().is_some_and(|on| s == on.as_bytes()));
    let bounds = widget.annotation().bounds()?;
    let max_len = object
        .get_dict_inheritable("MaxLen")?
        .map(|n| n.as_int())
        .transpose()?
        .filter(|n| *n > 0)
        .map(|n| n as usize);
    let name = widget
        .name()?
        .unwrap_or_else(|| format!("Field {}", widget.xref().unwrap_or(0)));
    let label = widget.label()?.filter(|s| !s.is_empty()).unwrap_or(name);
    Ok(Field {
        xref: widget.xref()?,
        label,
        kind,
        flags,
        bounds: [
            (bounds.x0 - page.x0) / page.width(),
            (bounds.y0 - page.y0) / page.height(),
            (bounds.x1 - page.x0) / page.width(),
            (bounds.y1 - page.y0) / page.height(),
        ],
        editable: permitted
            && kind != Kind::Unsupported
            && !flags.contains(FieldFlags::READ_ONLY)
            && !(kind == Kind::Text
                && flags.intersects(FieldFlags::FILE_SELECT | FieldFlags::PASSWORD))
            && !annotation_flags.intersects(
                AnnotationFlags::IS_READ_ONLY
                    | AnnotationFlags::IS_LOCKED
                    | AnnotationFlags::IS_LOCKED_CONTENTS
                    | AnnotationFlags::IS_HIDDEN
                    | AnnotationFlags::IS_INVISIBLE
                    | AnnotationFlags::NO_VIEW,
            )
            && (!matches!(kind, Kind::Checkbox | Kind::Radio) || on_value.is_some()),
        value: if matches!(kind, Kind::Text | Kind::Combo) {
            widget.value()?.unwrap_or_default()
        } else {
            String::new()
        },
        checked,
        on_value,
        choices,
        selected,
        max_len,
    })
}

impl PdfDocument {
    pub fn has_xfa(&self) -> Result<bool> {
        let Some(root) = self.document.trailer()?.get_dict("Root")? else {
            return Ok(false);
        };
        let Some(form) = root.get_dict("AcroForm")? else {
            return Ok(false);
        };
        Ok(form.get_dict("XFA")?.is_some())
    }

    pub fn form_fields(&self, number: usize) -> Result<Vec<Field>> {
        ensure!(number < self.page_count(), "page is out of range");
        let page = self.document.load_pdf_page(number as i32)?;
        let bounds = page.bounds()?;
        let permitted = self.permissions().fill_forms && !self.has_xfa()?;
        page.widgets()
            .map(|widget| read_field(&widget, bounds, permitted))
            .collect()
    }

    pub fn set_form_value(&mut self, number: usize, xref: i32, value: Value) -> Result<()> {
        ensure!(
            self.permissions().fill_forms,
            "This PDF does not allow form filling"
        );
        ensure!(!self.has_xfa()?, "XFA forms are not supported");
        ensure!(number < self.page_count(), "page is out of range");
        self.edit(|document| {
            let mut page = document.load_pdf_page(number as i32)?;
            let mut widget = page
                .load_widget(xref)?
                .context("Form field no longer exists")?;
            let field = read_field(&widget, page.bounds()?, true)?;
            ensure!(field.editable, "Form field is read-only or unsupported");
            match value {
                Value::Text(value) => {
                    ensure!(
                        field.kind == Kind::Text
                            || (field.kind == Kind::Combo
                                && field.flags.contains(FieldFlags::EDIT)),
                        "Field does not accept free text"
                    );
                    ensure!(!value.contains('\0'), "Field text contains NUL");
                    ensure!(
                        field
                            .max_len
                            .is_none_or(|limit| value.chars().count() <= limit),
                        "Field text exceeds its maximum length"
                    );
                    ensure!(
                        widget.set_value(document, &value, true)?,
                        "Field rejected its value"
                    );
                    if field.kind == Kind::Combo {
                        field_head(widget.annotation().object())?.dict_delete("I")?;
                    } else if field.flags.contains(FieldFlags::RICH_TEXT) {
                        // This editor writes plain text, not rich-text markup.
                        field_head(widget.annotation().object())?.dict_delete("RV")?;
                    }
                }
                Value::Toggle(checked) => {
                    ensure!(
                        matches!(field.kind, Kind::Checkbox | Kind::Radio),
                        "Field is not a button"
                    );
                    // Radio selection never deselects the current member.
                    ensure!(
                        checked || field.kind == Kind::Checkbox,
                        "Radio groups require a selected member"
                    );
                    let value = if checked {
                        field.on_value.as_deref().context("Missing on state")?
                    } else {
                        "Off"
                    };
                    ensure!(
                        widget.set_value(document, value, true)?,
                        "Field rejected its value"
                    );
                    // The generic widget setter only switches this widget's
                    // appearance. Explicitly synchronize the entire group,
                    // including members on pages not currently loaded.
                    let selected = (field.kind == Kind::Radio
                        && !field.flags.contains(FieldFlags::RADIOS_IN_UNISON))
                    .then_some(xref);
                    set_button_states(
                        field_head(widget.annotation().object())?,
                        value,
                        selected,
                        0,
                    )?;
                }
                Value::Choice(mut indices) => {
                    ensure!(
                        matches!(field.kind, Kind::Combo | Kind::List),
                        "Field is not a choice"
                    );
                    indices.sort_unstable();
                    indices.dedup();
                    ensure!(
                        indices.iter().all(|i| *i < field.choices.len()),
                        "Choice is out of range"
                    );
                    ensure!(
                        indices.len() <= 1
                            || (field.kind == Kind::List
                                && field.flags.contains(FieldFlags::MULTI_SELECT)),
                        "Field allows only one choice"
                    );
                    let values: Vec<_> = indices
                        .iter()
                        .map(|i| field.choices[*i].export.as_str())
                        .collect();
                    // The safe MuPDF setter marks the entire field group dirty.
                    // Then store /V and /I at that same group head. /I preserves
                    // exact selections even when display/export values repeat.
                    ensure!(
                        widget.set_value(document, values.first().copied().unwrap_or(""), true)?,
                        "Field rejected its value"
                    );
                    let mut head = field_head(widget.annotation().object())?;
                    let mut array = document.new_array()?;
                    for value in &values {
                        array.array_push(PdfObject::new_string(value)?)?;
                    }
                    head.dict_put(
                        "V",
                        if values.len() == 1 {
                            PdfObject::new_string(values[0])?
                        } else {
                            array
                        },
                    )?;
                    let mut array = document.new_array()?;
                    for index in indices {
                        array.array_push(PdfObject::new_int(index as i32)?)?;
                    }
                    head.dict_put("I", array)?;
                }
            }
            widget.update()?;
            page.update()?;
            Ok(())
        })
    }
}

#[derive(Default)]
pub struct Forms {
    pub open: bool,
    page: Option<usize>,
    fields: Vec<Field>,
    focus: Option<i32>,
}

impl Forms {
    pub fn invalidate(&mut self) {
        self.page = None;
    }

    fn refresh(&mut self, document: &PdfDocument) -> Result<()> {
        if self.page != Some(document.current_page()) {
            self.fields = document.form_fields(document.current_page())?;
            self.page = Some(document.current_page());
        }
        Ok(())
    }

    pub fn focus_ids(&self) -> impl Iterator<Item = egui::Id> + '_ {
        self.fields
            .iter()
            .filter(|f| {
                self.open
                    && f.editable
                    && (f.kind == Kind::Text
                        || (f.kind == Kind::Combo && f.flags.contains(FieldFlags::EDIT)))
            })
            .map(|f| egui::Id::new(("form", f.xref)))
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
        let ctx = root.ctx().clone();
        let restricted_copy = !document.permissions().copy
            && self
                .focus_ids()
                .any(|id| ctx.memory(|memory| memory.focused() == Some(id)));
        if restricted_copy {
            ctx.input_mut(|input| {
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Copy | egui::Event::Cut))
            });
        }
        let mut change = None;
        egui::Panel::right("forms").default_size(280.0).size_range(240.0..=400.0).show_inside(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Forms");
                if ui.button("Close").clicked() { self.open = false; }
            });
            ui.label(format!("Page {} · values saved in the PDF", document.current_page() + 1));
            if document.has_xfa().unwrap_or(true) { ui.label("XFA forms are not supported."); }
            else if !document.permissions().fill_forms { ui.label("This PDF does not allow form filling."); }
            ui.label("Tab / Shift+Tab moves focus. Space selects buttons and list choices.");
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.fields.is_empty() { ui.label("No form fields on this page."); }
                for field in &mut self.fields {
                    ui.push_id(field.xref, |ui| {
                        let label = ui.label(format!("{}{}{}", field.label, if field.flags.contains(FieldFlags::REQUIRED) { " · required" } else { "" }, if field.editable { "" } else { " · read-only" }));
                        ui.add_enabled_ui(field.editable, |ui| {
                            let id = egui::Id::new(("form", field.xref));
                            let response = match field.kind {
                                Kind::Text => {
                                    let edit = if field.flags.contains(FieldFlags::MULTILINE) { egui::TextEdit::multiline(&mut field.value).desired_rows(3) } else { egui::TextEdit::singleline(&mut field.value) };
                                    let response = ui.add(edit.id(id).desired_width(f32::INFINITY).password(field.flags.contains(FieldFlags::PASSWORD)).char_limit(field.max_len.unwrap_or(usize::MAX))).labelled_by(label.id);
                                    if response.changed() { change = Some((field.xref, Value::Text(field.value.clone()))); }
                                    response
                                }
                                Kind::Checkbox | Kind::Radio => {
                                    // Stable scope keeps keyboard focus across source replacements.
                                    let response = ui.push_id(id, |ui| if field.kind == Kind::Checkbox {
                                        ui.checkbox(&mut field.checked, &field.label)
                                    } else { ui.radio(field.checked, format!("{}: {}", field.label, field.on_value.as_deref().unwrap_or("Unavailable"))) }).inner;
                                    if response.clicked() && (field.kind == Kind::Checkbox || !field.checked) { change = Some((field.xref, Value::Toggle(field.kind == Kind::Radio || field.checked))); }
                                    response
                                }
                                Kind::Combo if field.flags.contains(FieldFlags::EDIT) => {
                                    let response = ui.add(egui::TextEdit::singleline(&mut field.value).id(id).desired_width(f32::INFINITY)).labelled_by(label.id);
                                    if response.changed() { change = Some((field.xref, Value::Text(field.value.clone()))); }
                                    egui::ComboBox::from_label(format!("{} choices", field.label)).selected_text("Choices").show_ui(ui, |ui| {
                                        for (index, choice) in field.choices.iter().enumerate() {
                                            if ui.selectable_label(false, &choice.label).clicked() { change = Some((field.xref, Value::Choice(vec![index]))); ui.close(); }
                                        }
                                    });
                                    response
                                }
                                Kind::Combo => {
                                    egui::ComboBox::from_id_salt(id).selected_text(field.selected.first().map_or("(none)", |i| field.choices[*i].label.as_str())).show_ui(ui, |ui| {
                                        for (index, choice) in field.choices.iter().enumerate() {
                                            if ui.selectable_label(field.selected.contains(&index), &choice.label).clicked() { change = Some((field.xref, Value::Choice(vec![index]))); ui.close(); }
                                        }
                                    }).response.labelled_by(label.id)
                                }
                                Kind::List => {
                                    let mut first = None;
                                    ui.vertical(|ui| {
                                        for (index, choice) in field.choices.iter().enumerate() {
                                            let selected = field.selected.contains(&index);
                                            let response = ui.selectable_label(selected, &choice.label);
                                            if response.clicked() {
                                                let mut indices = if field.flags.contains(FieldFlags::MULTI_SELECT) { field.selected.clone() } else { Vec::new() };
                                                if selected { indices.retain(|i| *i != index); } else { indices.push(index); }
                                                change = Some((field.xref, Value::Choice(indices)));
                                            }
                                            if response.has_focus() { response.scroll_to_me(None); }
                                            first.get_or_insert(response);
                                        }
                                    });
                                    first.unwrap_or_else(|| ui.label("No choices"))
                                }
                                Kind::Unsupported => ui.label("Unsupported field; buttons and signatures are not activated."),
                            };
                            if self.focus == Some(field.xref) && field.editable {
                                response.request_focus();
                                ui.scroll_to_rect(response.rect, None);
                                self.focus = None;
                            }
                            if response.has_focus() {
                                response.scroll_to_me(None);
                            }
                        });
                        ui.separator();
                    });
                }
            });
        });
        if restricted_copy {
            ctx.output_mut(|output| {
                output
                    .commands
                    .retain(|command| !matches!(command, egui::OutputCommand::CopyText(_)))
            });
        }
        if let Some((xref, value)) = change {
            before_edit();
            let result = document.set_form_value(document.current_page(), xref, value);
            self.invalidate();
            result?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn page_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &mut PdfDocument,
        pages: &[(usize, crate::layout::PageTransform)],
    ) -> Result<bool> {
        self.refresh(document)?;
        if !ui.is_enabled() {
            return Ok(false);
        }
        let mut activated = None;
        for &(number, transform) in pages {
            let other_fields;
            let fields = if self.page == Some(number) {
                &self.fields
            } else {
                other_fields = document.form_fields(number)?;
                &other_fields
            };
            for field in fields {
                let [x0, y0, x1, y1] = field.bounds;
                let rect = transform.bounds(egui::Rect::from_min_max(
                    egui::pos2(x0, y0),
                    egui::pos2(x1, y1),
                ));
                let response = ui
                    .interact(
                        rect,
                        ui.id().with(("form_hit", number, field.xref)),
                        egui::Sense::click(),
                    )
                    .on_hover_text(&field.label);
                if response.clicked() {
                    activated = Some((number, field.xref));
                }
                if self.open {
                    ui.painter().rect_stroke(
                        rect,
                        0.0,
                        egui::Stroke::new(
                            1.0_f32,
                            if field.editable {
                                egui::Color32::from_rgb(45, 130, 240)
                            } else {
                                egui::Color32::GRAY
                            },
                        ),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
        if let Some((number, xref)) = activated {
            document.go_to_page(number);
            self.open = true;
            self.focus = Some(xref);
            self.refresh(document)?;
            ui.ctx().request_repaint();
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mupdf::{
        Document,
        pdf::{Encryption, PdfWriteOptions, Permission},
    };

    pub fn fixture() -> Vec<u8> {
        let objects = [
            r#"<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R /OpenAction << /S /JavaScript /JS (this.getField\("Name"\).value="EXECUTED";) >> >>"#,
            "<< /Type /Pages /Kids [3 0 R 20 0 R] /Count 2 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 600] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R /Annots [10 0 R 11 0 R 13 0 R 15 0 R 16 0 R 18 0 R 19 0 R 22 0 R 23 0 R 24 0 R 25 0 R] >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Length 0 >>\nstream\n\nendstream",
            "<< /Fields [10 0 R 11 0 R 12 0 R 15 0 R 16 0 R 17 0 R 19 0 R 22 0 R 23 0 R 24 0 R 25 0 R 26 0 R] /DA (/F1 12 Tf 0 g) /DR << /Font << /F1 4 0 R >> >> >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 23 >>\nstream\n0 0 1 RG 1 1 18 18 re S\nendstream",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 23 >>\nstream\n0 0 1 rg 1 1 18 18 re f\nendstream",
            "null",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /TU (Full name) /MaxLen 12 /V (Original) /Rect [40 525 250 550] /F 4 /P 3 0 R /AA << /K << /S /JavaScript /JS (event.value=\"EXECUTED\";) >> >> >>",
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Consent) /V /Off /AS /Off /Rect [40 480 60 500] /F 4 /P 3 0 R /AP << /N << /Off 7 0 R /Accepted 8 0 R >> >> >>",
            "<< /FT /Btn /T (Plan) /Ff 49152 /V /Beta /Kids [13 0 R 14 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /Parent 12 0 R /AS /Off /Rect [40 440 60 460] /F 4 /P 3 0 R /AP << /N << /Off 7 0 R /Alpha 8 0 R >> >> >>",
            "<< /Type /Annot /Subtype /Widget /Parent 12 0 R /AS /Beta /Rect [70 440 90 460] /F 4 /P 20 0 R /AP << /N << /Off 7 0 R /Beta 8 0 R >> >> >>",
            "<< /Type /Annot /Subtype /Widget /FT /Ch /T (Country) /Ff 131072 /V (CA) /Opt [[(CA) (Canada)] [(CM) (Cameroon)] [<FEFF65E5672C> <FEFF65E5672C>]] /Rect [40 385 250 410] /F 4 /P 3 0 R >>",
            "<< /Type /Annot /Subtype /Widget /FT /Ch /T (Colours) /Ff 2097152 /V [(Amber)] /Opt [(Amber) (Blue) [(Blue) (Duplicate blue)]] /Rect [40 290 250 365] /F 4 /P 3 0 R >>",
            "<< /FT /Tx /T (ReadOnly) /TU (Read-only value) /Ff 1 /V (Protected) /Kids [18 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /Parent 17 0 R /Rect [40 240 250 265] /F 4 /P 3 0 R >>",
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Submit) /Ff 65536 /Rect [280 525 350 550] /F 4 /P 3 0 R /A << /S /SubmitForm /F (https://example.invalid/never-submit) >> >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 600] /Resources << /Font << /F1 4 0 R >> >> /Contents 21 0 R /Annots [14 0 R 26 0 R] >>",
            "<< /Length 0 >>\nstream\n\nendstream",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Message) /Ff 4096 /V (Two lines) /Rect [40 150 250 220] /F 4 /P 3 0 R >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Password) /Ff 8192 /V () /Rect [40 110 250 135] /F 4 /P 3 0 R >>",
            "<< /Type /Annot /Subtype /Widget /FT /Ch /T (Custom choice) /Ff 393216 /V (Free) /Opt [(Suggested) (Other)] /Rect [40 65 250 90] /F 4 /P 3 0 R >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Locked) /V (Locked value) /Rect [40 20 250 45] /F 132 /P 3 0 R >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Second page) /TU (Page two note) /V (Page two original) /Rect [155 320 345 355] /F 4 /P 20 0 R >>",
        ];
        let mut pdf = "%PDF-1.7\n".to_owned();
        let mut offsets = vec![0];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
        }
        let xref = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()));
        for offset in &offsets[1..] {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
            offsets.len()
        ));
        pdf.into_bytes()
    }

    pub fn document() -> PdfDocument {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("forms.pdf");
        std::fs::write(&path, fixture()).unwrap();
        PdfDocument::open(path).unwrap()
    }

    fn field(document: &PdfDocument, page: usize, xref: i32) -> Field {
        document
            .form_fields(page)
            .unwrap()
            .into_iter()
            .find(|f| f.xref == xref)
            .unwrap()
    }

    #[test]
    fn field_hits_and_outlines_follow_each_visible_page_transform() {
        use crate::layout::{PageTransform, Rotation};
        for (rotation, bounds) in [
            (
                Rotation::None,
                [0.3875, 245.0 / 600.0, 0.8625, 280.0 / 600.0],
            ),
            (
                Rotation::Clockwise,
                [320.0 / 600.0, 0.3875, 355.0 / 600.0, 0.8625],
            ),
            (
                Rotation::Half,
                [0.1375, 320.0 / 600.0, 0.6125, 355.0 / 600.0],
            ),
            (
                Rotation::Counterclockwise,
                [245.0 / 600.0, 0.1375, 280.0 / 600.0, 0.6125],
            ),
        ] {
            let mut document = document();
            let mut forms = Forms::default();
            let ctx = egui::Context::default();
            let pages = [
                (
                    0,
                    PageTransform {
                        rect: egui::Rect::from_min_size(
                            egui::pos2(20.0, 80.0),
                            egui::vec2(280.0, 420.0),
                        ),
                        rotation,
                    },
                ),
                (
                    1,
                    PageTransform {
                        rect: egui::Rect::from_min_size(
                            egui::pos2(430.0, 80.0),
                            egui::vec2(280.0, 420.0),
                        ),
                        rotation,
                    },
                ),
            ];
            // Derive expected bounds directly from the fixture's PDF rectangle,
            // not from PageTransform or the form hit implementation.
            let [x0, y0, x1, y1] = bounds;
            let expected = egui::Rect::from_min_max(
                egui::pos2(430.0 + x0 * 280.0, 80.0 + y0 * 420.0),
                egui::pos2(430.0 + x1 * 280.0, 80.0 + y1 * 420.0),
            );
            let point = expected.center();
            let mut activated = false;
            let mut run = |events| {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960.0, 720.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        activated = forms.page_ui(ui, &mut document, &pages).unwrap();
                    },
                )
            };
            run(Vec::new());
            for pressed in [true, false] {
                run(vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
            }
            assert!(activated, "{rotation:?}");
            assert_eq!(document.current_page(), 1);
            assert!(forms.open);
            assert_eq!(forms.focus, Some(26));
            assert!(!document.is_dirty());
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                forms.page_ui(ui, &mut document, &pages).unwrap();
            });
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect)
                if rect.rect.min.distance(expected.min) < 0.01 && rect.rect.max.distance(expected.max) < 0.01)), "Missing transformed outline for {rotation:?}");
        }
    }

    #[test]
    fn values_exports_groups_unicode_and_undo_round_trip_without_actions() {
        let mut document = document();
        assert_eq!(field(&document, 0, 10).value, "Original");
        assert_eq!(field(&document, 0, 10).label, "Full name");
        assert_eq!(field(&document, 0, 15).choices[2].label, "日本");
        assert!(!field(&document, 0, 13).checked);
        assert!(field(&document, 1, 14).checked);
        let revision = document.text_revision();
        let before = document.render_at_scale(0, 1.0).unwrap().rgba;
        document
            .set_form_value(0, 10, Value::Text("Ada résumé".into()))
            .unwrap();
        assert_eq!(
            field(&document, 0, 10).value,
            "Ada résumé",
            "JavaScript must not replace the supplied value"
        );
        assert!(document.text_revision() > revision);
        assert_ne!(before, document.render_at_scale(0, 1.0).unwrap().rgba);
        assert_eq!(
            document
                .worker_source()
                .open()
                .unwrap()
                .form_fields(0)
                .unwrap()[0]
                .value,
            "Ada résumé"
        );
        document.set_form_value(0, 11, Value::Toggle(true)).unwrap();
        assert!(field(&document, 0, 11).checked);
        assert_eq!(
            document
                .document
                .load_pdf_page(0)
                .unwrap()
                .load_widget(11)
                .unwrap()
                .unwrap()
                .value()
                .unwrap()
                .as_deref(),
            Some("Accepted")
        );
        document.set_form_value(0, 13, Value::Toggle(true)).unwrap();
        assert!(field(&document, 0, 13).checked);
        assert!(!field(&document, 1, 14).checked);
        document.set_form_value(1, 14, Value::Toggle(true)).unwrap();
        assert!(!field(&document, 0, 13).checked);
        assert!(field(&document, 1, 14).checked);
        document
            .set_form_value(0, 15, Value::Choice(vec![2]))
            .unwrap();
        assert_eq!(field(&document, 0, 15).value, "日本");
        document
            .set_form_value(0, 16, Value::Choice(vec![2, 0, 2]))
            .unwrap();
        assert_eq!(field(&document, 0, 16).selected, [0, 2]);
        document.undo().unwrap();
        assert_eq!(field(&document, 0, 16).selected, [0]);
        document.redo().unwrap();
        assert_eq!(field(&document, 0, 16).selected, [0, 2]);
        document
            .set_form_value(0, 22, Value::Text("Line one\nLine two".into()))
            .unwrap();
        document
            .set_form_value(0, 24, Value::Choice(vec![1]))
            .unwrap();
        document
            .set_form_value(0, 24, Value::Text("Custom free value".into()))
            .unwrap();
        assert!(
            field(&document, 0, 24).selected.is_empty(),
            "Free text must clear the preceding choice index"
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("saved.pdf");
        document.save(&path).unwrap();
        let reopened = PdfDocument::open(&path).unwrap();
        assert_eq!(field(&reopened, 0, 10).value, "Ada résumé");
        assert_eq!(field(&reopened, 0, 15).value, "日本");
        assert_eq!(field(&reopened, 0, 16).selected, [0, 2]);
        assert_eq!(field(&reopened, 0, 22).value, "Line one\nLine two");
        assert_eq!(field(&reopened, 0, 24).value, "Custom free value");
        assert_eq!(field(&reopened, 0, 18).value, "Protected");
        assert!(field(&reopened, 1, 14).checked);
        assert!(!reopened.is_dirty());
        document
            .set_form_value(0, 16, Value::Choice(Vec::new()))
            .unwrap();
        assert!(field(&document, 0, 16).selected.is_empty());
    }

    #[test]
    fn read_only_limits_invalid_choices_and_unsupported_fields_preserve_source() {
        let mut document = document();
        for (xref, value) in [
            (18, Value::Text("Changed".into())),
            (23, Value::Text("Secret".into())),
            (25, Value::Text("Changed".into())),
            (19, Value::Toggle(true)),
            (10, Value::Text("ThirteenChars".into())),
            (10, Value::Text("nul\0text".into())),
            (15, Value::Text("Arbitrary".into())),
            (15, Value::Choice(vec![0, 1])),
            (15, Value::Choice(vec![3])),
            (13, Value::Toggle(false)),
        ] {
            assert!(document.set_form_value(0, xref, value).is_err());
            assert!(!document.is_dirty());
            assert_eq!(document.text_revision(), 0);
        }
        document
            .set_form_value(0, 10, Value::Text("TwelveChars!".into()))
            .unwrap();
        assert_eq!(field(&document, 0, 10).value, "TwelveChars!");
    }

    fn modified_document(change: impl FnOnce(&mut mupdf::pdf::PdfDocument)) -> PdfDocument {
        let pdf = Document::from_bytes(&fixture(), "application/pdf").unwrap();
        let mut pdf = mupdf::pdf::PdfDocument::try_from(pdf).unwrap();
        pdf.disable_js().unwrap();
        change(&mut pdf);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("modified.pdf");
        pdf.save(path.to_str().unwrap()).unwrap();
        PdfDocument::open(path).unwrap()
    }

    #[test]
    fn radio_updates_cannot_change_a_locked_or_read_only_member_on_another_page() {
        for locked in [true, false] {
            let mut document = modified_document(|pdf| {
                let page = pdf.load_pdf_page(1).unwrap();
                let mut object = page.load_widget(14).unwrap().unwrap().annotation().object();
                object
                    .dict_put(
                        if locked { "F" } else { "Ff" },
                        PdfObject::new_int(if locked { 132 } else { 49153 }).unwrap(),
                    )
                    .unwrap();
            });
            assert!(document.set_form_value(0, 13, Value::Toggle(true)).is_err());
            assert!(!document.is_dirty());
            assert_eq!(document.text_revision(), 0);
            assert!(!field(&document, 0, 13).checked);
            assert!(field(&document, 1, 14).checked);
        }
    }

    #[test]
    fn duplicate_radio_exports_select_one_member_or_all_in_unison() {
        for unison in [true, false] {
            let mut document = modified_document(|pdf| {
                let page = pdf.load_pdf_page(1).unwrap();
                let mut object = page.load_widget(14).unwrap().unwrap().annotation().object();
                let mut normal = object
                    .get_dict("AP")
                    .unwrap()
                    .unwrap()
                    .get_dict("N")
                    .unwrap()
                    .unwrap();
                let appearance = normal.get_dict("Beta").unwrap().unwrap();
                normal.dict_delete("Beta").unwrap();
                normal.dict_put("Alpha", appearance).unwrap();
                object
                    .dict_put("AS", PdfObject::new_name("Alpha").unwrap())
                    .unwrap();
                let mut head = field_head(object).unwrap();
                head.dict_put("V", PdfObject::new_name("Alpha").unwrap())
                    .unwrap();
                head.dict_put(
                    "Ff",
                    PdfObject::new_int(
                        49152
                            | if unison {
                                FieldFlags::RADIOS_IN_UNISON.bits()
                            } else {
                                0
                            },
                    )
                    .unwrap(),
                )
                .unwrap();
            });
            document.set_form_value(0, 13, Value::Toggle(true)).unwrap();
            assert!(field(&document, 0, 13).checked);
            assert_eq!(field(&document, 1, 14).checked, unison);
        }
    }

    #[test]
    fn xfa_is_read_only_without_attempting_to_execute_or_rewrite_it() {
        let mut document = modified_document(|pdf| {
            let mut form = pdf
                .trailer()
                .unwrap()
                .get_dict("Root")
                .unwrap()
                .unwrap()
                .get_dict("AcroForm")
                .unwrap()
                .unwrap();
            form.dict_put("XFA", PdfObject::new_string("unsupported XFA").unwrap())
                .unwrap();
        });
        assert!(document.has_xfa().unwrap());
        assert!(document.form_fields(0).unwrap().iter().all(|f| !f.editable));
        assert!(
            document
                .set_form_value(0, 10, Value::Text("Changed".into()))
                .is_err()
        );
        assert!(!document.is_dirty());
        assert_eq!(field(&document, 0, 10).value, "Original");
    }

    fn encrypt(path: &std::path::Path, permissions: Permission) {
        let pdf = Document::from_bytes(&fixture(), "application/pdf").unwrap();
        let pdf = mupdf::pdf::PdfDocument::try_from(pdf).unwrap();
        let mut options = PdfWriteOptions::default();
        options
            .set_encryption(Encryption::Aes256)
            .set_user_password("")
            .set_owner_password("owner-secret")
            .set_permissions(permissions);
        pdf.save_with_options(path.to_str().unwrap(), options)
            .unwrap();
    }

    #[test]
    fn fill_permission_is_independent_of_copy_and_annotation_and_survives_encrypted_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("encrypted.pdf");
        for permissions in [Permission::FORM, Permission::ANNOTATE, Permission::COPY] {
            let allowed = permissions.intersects(Permission::FORM | Permission::ANNOTATE);
            encrypt(&path, permissions);
            let mut document = PdfDocument::open(&path).unwrap();
            assert_eq!(document.permissions().fill_forms, allowed);
            assert_eq!(field(&document, 0, 10).editable, allowed);
            assert_eq!(
                document
                    .set_form_value(0, 10, Value::Text("Allowed".into()))
                    .is_ok(),
                allowed
            );
            if allowed {
                document.save(&path).unwrap();
                let saved = PdfDocument::open(&path).unwrap();
                assert_eq!(saved.permissions(), document.permissions());
                assert!(!saved.permissions().copy);
                assert_eq!(field(&saved, 0, 10).value, "Allowed");
                assert!(
                    saved
                        .document
                        .trailer()
                        .unwrap()
                        .get_dict("Encrypt")
                        .unwrap()
                        .is_some()
                );
            } else {
                let mut owner = PdfDocument::open_with_password(&path, Some("owner-secret"))
                    .unwrap()
                    .unwrap();
                owner
                    .set_form_value(0, 10, Value::Text("Owner".into()))
                    .unwrap();
            }
        }
    }

    #[test]
    #[ignore = "exports synthetic AcroForm PDFs for native tests"]
    fn export_forms_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        std::fs::write(directory.join("forms.pdf"), fixture()).unwrap();
        encrypt(&directory.join("forms-only.pdf"), Permission::FORM);
        encrypt(&directory.join("forms-denied.pdf"), Permission::COPY);
    }

    #[test]
    #[ignore = "reopens output from the native forms check"]
    fn verify_native_forms_output() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        for name in ["forms", "forms-only"] {
            let document = PdfDocument::open(directory.join(format!("{name}.pdf"))).unwrap();
            assert_eq!(field(&document, 0, 10).value, "Nora native");
            assert!(field(&document, 0, 11).checked);
            assert_eq!(field(&document, 0, 13).checked, name == "forms-only");
            assert_eq!(field(&document, 1, 14).checked, name == "forms");
            assert_eq!(
                field(&document, 1, 26).value,
                if name == "forms" {
                    "Rotated second page"
                } else {
                    "Page two original"
                }
            );
            assert_eq!(
                field(&document, 0, 15).value,
                "CM",
                "Store the export, not the display label Cameroon"
            );
            assert_eq!(field(&document, 0, 16).selected, [0, 2]);
            assert_eq!(field(&document, 0, 18).value, "Protected");
            assert_eq!(field(&document, 0, 22).value, "Native first\nNative second");
            assert_eq!(field(&document, 0, 24).value, "Native custom");
            assert_eq!(field(&document, 0, 25).value, "Locked value");
            if name == "forms-only" {
                assert!(document.permissions().fill_forms);
                assert!(!document.permissions().copy);
                assert!(!document.permissions().annotate);
                assert!(
                    document
                        .document
                        .trailer()
                        .unwrap()
                        .get_dict("Encrypt")
                        .unwrap()
                        .is_some()
                );
            }
        }
        let denied = PdfDocument::open(directory.join("forms-denied.pdf")).unwrap();
        assert_eq!(field(&denied, 0, 10).value, "Original");
    }
}
