//! Read-only PDF inspection. No actions, JavaScript, or signature verification.
use std::{collections::HashSet, path::Path};

use anyhow::{Context, Result, ensure};
use mupdf::{
    MetadataName,
    pdf::{OptionalContentRef, PdfDocument, PdfObject},
};

pub const MAX_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;

pub struct Inspection {
    pub metadata: Vec<(&'static str, Result<String, String>)>,
    pub attachments: Result<Vec<Attachment>, String>,
    pub layers: Result<Vec<Layer>, String>,
    pub signatures: Result<Vec<Signature>, String>,
}

pub struct Attachment {
    pub name: String,
    pub filename: String,
    pub description: String,
    pub mime_type: String,
    pub size: Option<usize>,
    stream: PdfObject,
}

impl Attachment {
    pub fn suggested_name(&self) -> String {
        // PDF filenames are untrusted and may use either platform's separators.
        let name: String = self
            .filename
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control() && !"<>:\"|?*".contains(*c))
            .collect();
        let name = name.trim_matches([' ', '.']);
        if name.is_empty() {
            "attachment.bin".into()
        } else {
            name.into()
        }
    }

    pub fn save(&self, path: &Path, document_path: &Path) -> Result<()> {
        if path.exists() {
            ensure!(
                path.canonicalize()? != document_path.canonicalize()?,
                "An attachment cannot replace the open PDF"
            );
        }
        ensure!(
            self.size.is_none_or(|size| size <= MAX_ATTACHMENT_BYTES),
            "Attachment exceeds the 64 MiB saving limit"
        );
        // MuPDF's safe stream API decodes to a Vec; it has no bounded reader.
        // Check declared size first, then actual decoded size before writing.
        let bytes = self
            .stream
            .read_stream()
            .context("failed to decode attachment")?;
        ensure!(
            bytes.len() <= MAX_ATTACHMENT_BYTES,
            "Attachment exceeds the 64 MiB saving limit"
        );
        std::fs::write(path, bytes).context("failed to save attachment")
    }
}

pub struct Layer {
    pub reference: OptionalContentRef,
    pub name: String,
    pub enabled: bool,
    pub controllable: bool,
}

pub struct Signature {
    pub name: String,
    pub present: bool,
    pub details: Vec<(&'static str, String)>,
}

impl Signature {
    pub fn status(&self) -> &'static str {
        if self.present {
            "Signature present — not verified"
        } else {
            "Unsigned signature field"
        }
    }
}

impl Inspection {
    pub fn read(document: &PdfDocument) -> Self {
        use MetadataName::*;
        let metadata = [
            ("Format", Format),
            ("Encryption", Encryption),
            ("Title", Title),
            ("Author", Author),
            ("Subject", Subject),
            ("Keywords", Keywords),
            ("Creator", Creator),
            ("Producer", Producer),
            ("Created (PDF date)", CreationDate),
            ("Modified (PDF date)", ModDate),
        ]
        .into_iter()
        .map(|(label, key)| (label, document.metadata(key).map_err(|e| e.to_string())))
        .collect();
        Self {
            metadata,
            attachments: attachments(document).map_err(|e| format!("{e:#}")),
            layers: layers(document).map_err(|e| format!("{e:#}")),
            signatures: signatures(document).map_err(|e| format!("{e:#}")),
        }
    }
}

fn text(object: &PdfObject, key: &str) -> Result<String> {
    Ok(object
        .get_dict(key)?
        .filter(|value| value.is_string().unwrap_or(false))
        .map(|value| value.as_string())
        .transpose()?
        .unwrap_or_default())
}

fn attachment(name: String, spec: PdfObject) -> Result<Option<Attachment>> {
    let Some(ef) = spec.get_dict("EF")? else {
        return Ok(None);
    };
    let Some(stream) = ef.get_dict("UF")?.or(ef.get_dict("F")?) else {
        return Ok(None);
    };
    ensure!(
        stream.is_stream()?,
        "Attachment {name} has no embedded stream"
    );
    let filename = text(&spec, "UF")?;
    let filename = if filename.is_empty() {
        text(&spec, "F")?
    } else {
        filename
    };
    let size = stream
        .get_dict("Params")?
        .map(|params| params.get_dict("Size"))
        .transpose()?
        .flatten()
        .map(|size| size.as_int())
        .transpose()?
        .and_then(|size| usize::try_from(size).ok());
    let mime_type = stream
        .get_dict("Subtype")?
        .map(|value| value.as_name())
        .transpose()?
        .map(|name| String::from_utf8_lossy(&name).into_owned())
        .unwrap_or_default();
    Ok(Some(Attachment {
        filename: if filename.is_empty() {
            name.clone()
        } else {
            filename
        },
        description: text(&spec, "Desc")?,
        name,
        mime_type,
        size,
        stream,
    }))
}

fn attachments(document: &PdfDocument) -> Result<Vec<Attachment>> {
    // Use MuPDF's name-tree loader, not embedded_files(), which only reads
    // a flat /Names array in mupdf-rs 0.8. This also supports /Kids trees.
    let tree = document.load_name_tree(PdfObject::new_name("EmbeddedFiles")?)?;
    let mut files = Vec::new();
    for index in 0..tree.dict_len()? {
        let key = tree
            .get_dict_key(index as i32)?
            .context("attachment key missing")?;
        let name = String::from_utf8_lossy(&key.as_name()?).into_owned();
        let spec = tree
            .get_dict_val(index as i32)?
            .context("attachment filespec missing")?;
        if let Some(file) = attachment(name, spec)? {
            files.push(file);
        }
    }
    Ok(files)
}

fn layers(document: &PdfDocument) -> Result<Vec<Layer>> {
    let groups = document.optional_content_groups()?;
    let properties = document.catalog()?.get_dict("OCProperties")?;
    let config = properties.map(|p| p.get_dict("D")).transpose()?.flatten();
    // The safe binding does not implement runtime layer configurations. Keep
    // unsupported UI policies read-only rather than violating radio/lock rules.
    let complex = config
        .as_ref()
        .map(|c| -> Result<bool> {
            Ok(c.get_dict("RBGroups")?.is_some() || c.get_dict("AS")?.is_some())
        })
        .transpose()?
        .unwrap_or(false);
    let base_off = config
        .as_ref()
        .map(|c| c.get_dict("BaseState"))
        .transpose()?
        .flatten()
        .map(|state| state.as_name())
        .transpose()?
        .is_some_and(|state| state == b"OFF");
    let on = config
        .as_ref()
        .map(|c| c.get_dict("ON"))
        .transpose()?
        .flatten();
    let locked = config
        .as_ref()
        .map(|c| c.get_dict("Locked"))
        .transpose()?
        .flatten();
    groups
        .into_iter()
        .map(|group| {
            let xref = group.reference.xref();
            let usage = document
                .xref_object(xref)?
                .map(|object| object.get_dict("Usage"))
                .transpose()?
                .flatten()
                .is_some();
            Ok(Layer {
                reference: group.reference,
                name: group.name.unwrap_or_else(|| format!("Layer {xref}")),
                enabled: group.enabled && (!base_off || contains_xref(on.as_ref(), xref)?),
                controllable: !complex && !usage && !contains_xref(locked.as_ref(), xref)?,
            })
        })
        .collect()
}

fn contains_xref(array: Option<&PdfObject>, xref: i32) -> Result<bool> {
    if let Some(array) = array {
        for index in 0..array.len()? {
            if array
                .get_array(index as i32)?
                .is_some_and(|item| item.as_indirect().ok() == Some(xref))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn signatures(document: &PdfDocument) -> Result<Vec<Signature>> {
    let mut signatures = Vec::new();
    if let Some(form) = document.catalog()?.get_dict("AcroForm")?
        && let Some(fields) = form.get_dict("Fields")?
    {
        signature_fields(&fields, "", 0, &mut HashSet::new(), &mut signatures)?;
    }
    Ok(signatures)
}

fn signature_fields(
    fields: &PdfObject,
    parent: &str,
    depth: usize,
    seen: &mut HashSet<i32>,
    output: &mut Vec<Signature>,
) -> Result<()> {
    ensure!(depth < 64, "Signature field tree is too deep");
    for index in 0..fields.len()? {
        let field = fields
            .get_array(index as i32)?
            .context("signature field missing")?;
        if field.is_indirect()? {
            ensure!(
                seen.insert(field.as_indirect()?),
                "Repeated or cyclic signature field"
            );
        }
        let part = text(&field, "T")?;
        let name = match (parent.is_empty(), part.is_empty()) {
            (true, _) => part,
            (_, true) => parent.into(),
            _ => format!("{parent}.{part}"),
        };
        if let Some(kids) = field.get_dict("Kids")? {
            let mut has_child_fields = false;
            for child in 0..kids.len()? {
                let child = kids
                    .get_array(child as i32)?
                    .context("signature child missing")?;
                has_child_fields |=
                    child.get_dict("T")?.is_some() || child.get_dict("Kids")?.is_some();
            }
            if has_child_fields {
                signature_fields(&kids, &name, depth + 1, seen, output)?;
                continue;
            }
        }
        // Read fields, not just page widgets: invisible signature fields count.
        // /V presence is evidence of a signature value, never of validity.
        if field
            .get_dict_inheritable("FT")?
            .map(|kind| kind.as_name())
            .transpose()?
            .is_some_and(|kind| kind == b"Sig")
        {
            let value = field.get_dict_inheritable("V")?;
            let present = value.as_ref().is_some_and(|v| !v.is_null().unwrap_or(true));
            let mut details = Vec::new();
            if let Some(value) = value.filter(|_| present) {
                for (label, key) in [
                    ("Claimed signer", "Name"),
                    ("Signing time (PDF date)", "M"),
                    ("Reason", "Reason"),
                    ("Location", "Location"),
                ] {
                    let value = text(&value, key)?;
                    if !value.is_empty() {
                        details.push((label, value));
                    }
                }
                for (label, key) in [("Filter", "Filter"), ("Subfilter", "SubFilter")] {
                    if let Some(value) = value.get_dict(key)? {
                        details.push((
                            label,
                            String::from_utf8_lossy(&value.as_name()?).into_owned(),
                        ));
                    }
                }
            }
            output.push(Signature {
                name: if name.is_empty() {
                    "Unnamed signature field".into()
                } else {
                    name
                },
                present,
                details,
            });
            // A signature field's /Kids are widget instances of the same value.
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn fixture() -> Vec<u8> {
        let content = "0 0 1 rg 20 20 30 30 re f /OC /Draft BDC 1 0 0 rg 100 100 70 30 re f 0 g BT /F1 12 Tf 40 350 Td (Draft text) Tj ET EMC";
        let payload = "embedded payload\n";
        let objects = [
            r"<< /Type /Catalog /Pages 2 0 R /PageLabels << /Nums [0 << /S /r /St 4 >> 1 << /P (A-) /S /D >>] >> /Names << /EmbeddedFiles << /Kids [12 0 R] >> >> /OCProperties << /OCGs [8 0 R 9 0 R 10 0 R] /D << /BaseState /ON /OFF [9 0 R] /Locked [10 0 R] >> >> /AcroForm << /Fields [15 0 R 16 0 R 17 0 R] >> /OpenAction << /S /JavaScript /JS (app.alert\(never run\)) >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /Properties << /Draft 8 0 R >> >> /Contents 6 0 R >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Resources << >> /Contents 7 0 R >>".into(),
            "null".into(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Length 0 >>\nstream\n\nendstream".into(),
            "<< /Type /OCG /Name (Draft marks) >>".into(),
            "<< /Type /OCG /Name (Notes) >>".into(),
            "<< /Type /OCG /Name (Locked artwork) >>".into(),
            "<< /Title <FEFF005200E900730075006D00E9> /Author (Review fixture) /Subject (Inspection) /CreationDate (D:20261003123000-04'00') >>".into(),
            "<< /Limits [(payload) (payload)] /Names [(payload) 13 0 R] >>".into(),
            "<< /Type /Filespec /F (../../payload.txt) /Desc (Synthetic attachment) /EF << /F 14 0 R >> >>".into(),
            format!("<< /Type /EmbeddedFile /Subtype /text#2Fplain /Params << /Size {} >> /Length {} >>\nstream\n{payload}endstream", payload.len(), payload.len()),
            "<< /FT /Sig /T (Pending approval) >>".into(),
            "<< /FT /Sig /T (Empty value) /V null >>".into(),
            "<< /T (Approvals) /Kids [18 0 R] >>".into(),
            "<< /FT /Sig /T (Signature) /Parent 17 0 R /V 19 0 R >>".into(),
            "<< /Type /Sig /Name (Mallory) /Reason (Unverified fixture claim) /M (D:20261003120000Z) /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 1 2 3] /Contents <0001> >>".into(),
        ];
        let mut pdf = "%PDF-1.7\n".to_string();
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
            "trailer\n<< /Size {} /Root 1 0 R /Info 11 0 R >>\nstartxref\n{xref}\n%%EOF",
            offsets.len()
        ));
        pdf.into_bytes()
    }

    #[test]
    fn metadata_nested_attachments_and_forged_signatures_are_inspected_without_trust() {
        let pdf = PdfDocument::from_bytes(&fixture()).unwrap();
        let inspection = Inspection::read(&pdf);
        let value = |key| {
            inspection
                .metadata
                .iter()
                .find(|(name, _)| *name == key)
                .unwrap()
                .1
                .as_ref()
                .unwrap()
        };
        assert_eq!(value("Title"), "Résumé");
        assert_eq!(value("Author"), "Review fixture");
        assert_eq!(value("Created (PDF date)"), "D:20261003123000-04'00'");
        assert_eq!(value("Keywords"), "");
        let files = inspection.attachments.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "payload");
        assert_eq!(files[0].filename, "../../payload.txt");
        assert_eq!(files[0].suggested_name(), "payload.txt");
        assert_eq!(files[0].mime_type, "text/plain");
        assert_eq!(files[0].size, Some(17));
        let signatures = inspection.signatures.unwrap();
        assert_eq!(signatures.len(), 3);
        assert!(!signatures[0].present);
        assert!(!signatures[1].present);
        assert_eq!(signatures[2].name, "Approvals.Signature");
        assert_eq!(signatures[2].status(), "Signature present — not verified");
        assert!(
            signatures[2]
                .details
                .contains(&("Claimed signer", "Mallory".into()))
        );
        assert!(!pdf.is_js_supported().unwrap());
        assert!(!pdf.has_unsaved_changes());
    }

    #[test]
    fn attachments_save_only_to_selected_destination_and_cannot_replace_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.pdf");
        let bytes = fixture();
        std::fs::write(&source, &bytes).unwrap();
        let pdf = PdfDocument::from_bytes(&bytes).unwrap();
        let mut files = Inspection::read(&pdf).attachments.unwrap();
        let destination = directory.path().join("chosen.txt");
        assert!(!destination.exists());
        files[0].save(&destination, &source).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"embedded payload\n");
        assert!(files[0].save(&source, &source).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        files[0].filename = "..\\..\\日本語.txt".into();
        assert_eq!(files[0].suggested_name(), "日本語.txt");
        files[0].filename = "../..".into();
        assert_eq!(files[0].suggested_name(), "attachment.bin");
        files[0].size = Some(MAX_ATTACHMENT_BYTES);
        files[0].save(&destination, &source).unwrap();
        files[0].size = Some(MAX_ATTACHMENT_BYTES + 1);
        let rejected = directory.path().join("rejected.txt");
        assert!(files[0].save(&rejected, &source).is_err());
        assert!(!rejected.exists());
    }

    #[test]
    fn layer_toggle_changes_pixels_after_source_was_rendered_without_modifying_pdf() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("inspection.pdf");
        let bytes = fixture();
        std::fs::write(&source, &bytes).unwrap();
        let mut document = crate::document::PdfDocument::open(&source).unwrap();
        let red = |image: crate::document::PageImage| {
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[0] > 200 && p[1] < 20 && p[2] < 20)
                .count()
        };
        assert_eq!(red(document.render_at_scale(0, 1.0).unwrap()), 2100);
        assert_eq!(
            document.structured_text(0).unwrap().plain_text(),
            "Draft text"
        );
        let mut layers = Inspection::read(document.pdf()).layers.unwrap();
        assert_eq!(
            layers.iter().map(|layer| layer.enabled).collect::<Vec<_>>(),
            [true, false, true]
        );
        assert!(!layers[2].controllable);
        layers[0].enabled = false;
        document.set_layer_visibility(&layers).unwrap();
        assert_eq!(red(document.render_at_scale(0, 1.0).unwrap()), 0);
        assert!(document.structured_text(0).unwrap().plain_text().is_empty());
        assert!(
            document
                .print_page(0)
                .unwrap()
                .to_text_page(mupdf::TextPageFlags::empty())
                .unwrap()
                .to_text()
                .unwrap()
                .trim()
                .is_empty()
        );
        let worker_source = document.worker_source();
        let worker_red = std::thread::spawn(move || {
            red(worker_source
                .open()
                .unwrap()
                .render_at_scale(0, 1.0)
                .unwrap())
        })
        .join()
        .unwrap();
        assert_eq!(worker_red, 0);
        layers[0].enabled = true;
        document.set_layer_visibility(&layers).unwrap();
        assert_eq!(red(document.render_at_scale(0, 1.0).unwrap()), 2100);
        assert_eq!(document.page_text(0).unwrap(), "Draft text");
        assert!(
            document
                .print_page(0)
                .unwrap()
                .to_text_page(mupdf::TextPageFlags::empty())
                .unwrap()
                .to_text()
                .unwrap()
                .contains("Draft text")
        );
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        assert!(!document.pdf().has_unsaved_changes());
    }

    #[test]
    fn labels_navigate_with_physical_numbers_and_duplicates_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("inspection.pdf");
        std::fs::write(&source, fixture()).unwrap();
        let document = crate::document::PdfDocument::open(&source).unwrap();
        assert_eq!(document.page_label(0).unwrap(), "iv");
        assert_eq!(document.page_label(1).unwrap(), "A-1");
        assert_eq!(document.page_description(1), "A-1 (page 2)");
        assert_eq!(document.resolve_page(" iv ").unwrap(), 0);
        assert_eq!(document.resolve_page("A-1").unwrap(), 1);
        assert_eq!(document.resolve_page("2").unwrap(), 1);
        for input in ["0", "3", "IV", "a-1", "-1", "", "NaN"] {
            assert!(document.resolve_page(input).is_err(), "{input}");
        }
        let mut pdf = PdfDocument::from_bytes(&fixture()).unwrap();
        use mupdf::pdf::{PageLabelRule, PageLabelStyle};
        let mut rule = PageLabelRule::new(0, PageLabelStyle::None);
        rule.prefix = "Duplicate".into();
        pdf.set_page_label_rule(rule).unwrap();
        let mut rule = PageLabelRule::new(1, PageLabelStyle::None);
        rule.prefix = "Duplicate".into();
        pdf.set_page_label_rule(rule).unwrap();
        let mut bytes = Vec::new();
        pdf.write_to(&mut bytes).unwrap();
        std::fs::write(&source, bytes).unwrap();
        let document = crate::document::PdfDocument::open(&source).unwrap();
        assert!(
            document
                .resolve_page("Duplicate")
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert_eq!(document.resolve_page("1").unwrap(), 0);
    }

    #[test]
    fn inherited_signature_type_and_widget_instances_are_counted_once() {
        let pdf = PdfDocument::from_bytes(&fixture()).unwrap();
        pdf.xref_object(17)
            .unwrap()
            .unwrap()
            .dict_put("FT", PdfObject::new_name("Sig").unwrap())
            .unwrap();
        pdf.xref_object(18)
            .unwrap()
            .unwrap()
            .dict_delete("FT")
            .unwrap();
        assert_eq!(signatures(&pdf).unwrap()[2].name, "Approvals.Signature");
        pdf.xref_object(18)
            .unwrap()
            .unwrap()
            .dict_put(
                "Kids",
                pdf.new_object_from_str("[<< /Subtype /Widget >> << /Subtype /Widget >>]")
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(signatures(&pdf).unwrap().len(), 3);
        pdf.xref_object(17)
            .unwrap()
            .unwrap()
            .dict_put("Kids", pdf.new_object_from_str("[17 0 R]").unwrap())
            .unwrap();
        let data = Inspection::read(&pdf);
        assert!(data.signatures.err().unwrap().contains("cyclic"));
        assert_eq!(data.attachments.unwrap().len(), 1);
        assert_eq!(data.layers.unwrap().len(), 3);
    }

    #[test]
    fn base_off_locked_and_usage_controlled_layers_do_not_offer_misleading_controls() {
        let pdf = PdfDocument::from_bytes(&fixture()).unwrap();
        let mut config = pdf
            .catalog()
            .unwrap()
            .get_dict("OCProperties")
            .unwrap()
            .unwrap()
            .get_dict("D")
            .unwrap()
            .unwrap();
        config
            .dict_put("BaseState", PdfObject::new_name("OFF").unwrap())
            .unwrap();
        config
            .dict_put("ON", pdf.new_object_from_str("[8 0 R]").unwrap())
            .unwrap();
        assert_eq!(
            layers(&pdf)
                .unwrap()
                .iter()
                .map(|layer| layer.enabled)
                .collect::<Vec<_>>(),
            [true, false, false]
        );
        assert!(!layers(&pdf).unwrap()[2].controllable);
        pdf.xref_object(8)
            .unwrap()
            .unwrap()
            .dict_put(
                "Usage",
                pdf.new_object_from_str("<< /View << /ViewState /OFF >> >>")
                    .unwrap(),
            )
            .unwrap();
        assert!(!layers(&pdf).unwrap()[0].controllable);
        config
            .dict_put(
                "RBGroups",
                pdf.new_object_from_str("[[8 0 R 9 0 R]]").unwrap(),
            )
            .unwrap();
        assert!(
            layers(&pdf)
                .unwrap()
                .iter()
                .all(|layer| !layer.controllable)
        );
    }

    #[test]
    fn encrypted_worker_source_preserves_authentication_permissions_and_layer_state() {
        use mupdf::pdf::{Encryption, PdfWriteOptions, Permission};
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("encrypted.pdf");
        let pdf = PdfDocument::from_bytes(&fixture()).unwrap();
        let mut options = PdfWriteOptions::default();
        options
            .set_encryption(Encryption::Aes256)
            .set_user_password("fixture-password")
            .set_owner_password("fixture-owner")
            .set_permissions(Permission::ACCESSIBILITY);
        pdf.save_with_options(source.to_str().unwrap(), options)
            .unwrap();
        let bytes = std::fs::read(&source).unwrap();
        let mut document =
            crate::document::PdfDocument::open_with_password(&source, Some("fixture-password"))
                .unwrap()
                .unwrap();
        let mut layers = Inspection::read(document.pdf()).layers.unwrap();
        layers[0].enabled = false;
        document.set_layer_visibility(&layers).unwrap();
        let worker_source = document.worker_source();
        drop(document);
        std::thread::spawn(move || {
            let worker = worker_source.open().unwrap();
            assert!(!worker.permissions().copy);
            assert!(!worker.permissions().print);
            let image = worker.render_at_scale(0, 1.0).unwrap();
            assert!(
                image
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| !(p[0] > 200 && p[1] < 20 && p[2] < 20))
            );
        })
        .join()
        .unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
    }

    #[test]
    #[ignore = "exports synthetic inspection PDF to REVIEW_FIXTURE_DIR for native tests"]
    fn export_inspection_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        std::fs::write(directory.join("inspection.pdf"), fixture()).unwrap();
    }
}
