use anyhow::Result;
use egui::{Color32, Rect};
use mupdf::{
    Document,
    document::Location,
    link::LinkDestination,
    pdf::{PdfAction, PdfDestination, PdfDocument, PdfPage},
};

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Internal(LinkDestination),
    External(SafeUrl),
}

/// Constructed only after validation. No shell, file path or arbitrary scheme
/// ever reaches the platform URL opener.
#[derive(Clone, Debug, PartialEq)]
pub struct SafeUrl(String);

impl SafeUrl {
    pub fn parse(input: &str) -> Option<Self> {
        if input.chars().any(|c| c.is_control() || c.is_whitespace()) || input.contains('\\') {
            return None;
        }
        // Reject escaped controls too (especially mailto header injection).
        let bytes = input.as_bytes();
        for escape in bytes.windows(3).filter(|v| v[0] == b'%') {
            let value = std::str::from_utf8(&escape[1..])
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok());
            if value.is_some_and(|v| v < 32 || v == 127) {
                return None;
            }
        }
        let url = url::Url::parse(input).ok()?;
        match url.scheme() {
            "http" | "https"
                if input.split_once(':')?.1.starts_with("//")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none() => {}
            "mailto" if !url.path().is_empty() && !url.path().starts_with('/') => {}
            _ => return None,
        }
        Some(Self(url.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct PageLink {
    /// Page-relative fractions, matching the displayed image at every zoom/DPI.
    pub bounds: Rect,
    pub target: LinkTarget,
}

pub fn extract(document: &Document, number: usize) -> Result<Vec<PageLink>> {
    let page = PdfPage::try_from(document.load_page(number as i32)?)?;
    let pdf = PdfDocument::try_from(document.clone())?;
    let bounds = page.bounds()?;
    let ctm = page.ctm()?;
    let mut links = Vec::new();
    for annotation in page.link_annotations()? {
        // A broken/unsupported annotation must not disable valid links nearby.
        let Ok(annotation) = annotation else { continue };
        let parsed = (|| -> Result<Option<PageLink>> {
            if annotation.get_dict("AA")?.is_some() {
                return Ok(None);
            }
            if let Some(action) = annotation.get_dict("A")? {
                if action.get_dict("Next")?.is_some() {
                    return Ok(None);
                }
                let Some(kind) = action.get_dict("S")? else {
                    return Ok(None);
                };
                if !matches!(kind.as_name()?.as_slice(), b"GoTo" | b"URI") {
                    return Ok(None);
                }
            }
            let Some(action) = annotation.action(&pdf, Some(number as i32))? else {
                return Ok(None);
            };
            let target = match action.into_pdf_action() {
                PdfAction::GoTo(PdfDestination::Page { page, kind }) => {
                    if page >= document.page_count()? as u32 {
                        return Ok(None);
                    }
                    LinkTarget::Internal(LinkDestination {
                        loc: Location {
                            chapter: 0,
                            page_in_chapter: page,
                            page_number: page,
                        },
                        kind,
                    })
                }
                PdfAction::GoTo(destination @ PdfDestination::Named(_)) => {
                    let Some(dest) = document.resolve_link(&format!("#{destination}"))? else {
                        return Ok(None);
                    };
                    if dest.loc.page_number >= document.page_count()? as u32 {
                        return Ok(None);
                    }
                    LinkTarget::Internal(dest)
                }
                PdfAction::Uri(uri) => {
                    let Some(url) = SafeUrl::parse(&uri) else {
                        return Ok(None);
                    };
                    LinkTarget::External(url)
                }
                _ => return Ok(None),
            };
            let rect = annotation.rect(Some(&ctm))?;
            let values = [rect.x0, rect.y0, rect.x1, rect.y1];
            if !values.iter().all(|v| v.is_finite()) || rect.x1 <= rect.x0 || rect.y1 <= rect.y0 {
                return Ok(None);
            }
            let normalized = Rect::from_min_max(
                egui::pos2(
                    (rect.x0 - bounds.x0) / (bounds.x1 - bounds.x0),
                    (rect.y0 - bounds.y0) / (bounds.y1 - bounds.y0),
                ),
                egui::pos2(
                    (rect.x1 - bounds.x0) / (bounds.x1 - bounds.x0),
                    (rect.y1 - bounds.y0) / (bounds.y1 - bounds.y0),
                ),
            );
            Ok(Some(PageLink {
                bounds: normalized,
                target,
            }))
        })();
        if let Ok(Some(link)) = parsed {
            links.push(link);
        }
    }
    Ok(links)
}

impl PageLink {
    pub fn screen_bounds(&self, page: Rect) -> Rect {
        Rect::from_min_max(
            page.min + self.bounds.min.to_vec2() * page.size(),
            page.min + self.bounds.max.to_vec2() * page.size(),
        )
        .intersect(page)
    }
}

/// Keep hit testing and feedback independent of page rendering/text selection.
/// Register after Selection::ui: links claim clicks/hover, selection owns drags.
pub fn ui(ui: &mut egui::Ui, links: &[PageLink], page: Rect) -> Option<LinkTarget> {
    let mut activated = None;
    for (index, link) in links.iter().enumerate() {
        let rect = link.screen_bounds(page);
        if !rect.is_positive() {
            continue;
        }
        let response = ui.interact(
            rect,
            ui.id().with(("pdf_link", index)),
            egui::Sense::click(),
        );
        let label = match &link.target {
            LinkTarget::Internal(dest) => format!("Go to page {}", dest.loc.page_number + 1),
            LinkTarget::External(url) => url.as_str().to_owned(),
        };
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(60, 140, 255, 45));
        }
        let response = response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(label);
        if let LinkTarget::External(url) = &link.target {
            response.context_menu(|ui| {
                if ui.button("Copy Link").clicked() {
                    ui.ctx().copy_text(url.as_str().to_owned());
                    ui.close();
                }
            });
        }
        if response.clicked() {
            activated = Some(link.target.clone());
        }
    }
    activated
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use egui::Vec2;

    pub fn fixture() -> Vec<u8> {
        let actions = [
            "/Dest [4 0 R /XYZ 100 650 2.25]",
            "/Dest (chapter two)",
            "/A << /S /URI /URI (https://example.org/paper.pdf#page=3) >>",
            "/A << /S /URI /URI (http://example.org/reader) >>",
            "/A << /S /URI /URI (mailto:reader@example.org?subject=Review) >>",
            "/A << /S /GoTo /D [4 0 R /FitH 650] >>",
            "/Dest [4 0 R /FitR 100 450 300 650]",
            "/Dest [4 0 R /XYZ null 650 null]",
            "/A << /S /Launch /F << /FS /URL /F (https://example.org/launch) >> >>",
            "/A << /S /GoToR /F << /FS /URL /F (https://example.org/remote.pdf) >> /D [0 /Fit] >>",
            r#"/A << /S /JavaScript /JS (app.launchURL\("https://example.org"\)) >>"#,
            "/A << /S /URI /URI (file:///tmp/launch.pdf) >>",
            r"/A << /S /URI /URI (javascript:alert\(1\)) >>",
            "/A << /S /Named /N /NextPage >>",
            "/A << /S /URI /URI (https://example.org/chained) /Next << /S /Launch /F (/tmp/program) >> >>",
            "/AA << /U << /S /URI /URI (https://example.org/automatic) >> >>",
            "/Dest (missing chapter)",
            "/A << /S /URI /URI (https://example.org) >> /Rect [1 2 1 2]",
        ];
        let labels = [
            "XYZ: page 2 at 225%",
            "Named chapter two",
            "HTTPS: example.org",
            "HTTP: example.org",
            "Email reader",
            "Fit width at destination",
            "Fit destination rectangle",
            "Keep zoom and left position",
            "Blocked actions below",
        ];
        let annots = actions
            .iter()
            .enumerate()
            .map(|(i, action)| {
                let top = 375 - i as i32 * 30;
                format!(
                    "<< /Type /Annot /Subtype /Link /Rect [40 {} 250 {top}] {action} >>",
                    top - 25
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let text = format!(
            "BT /F1 12 Tf 40 355 Td {} ET",
            labels
                .iter()
                .enumerate()
                .map(|(i, label)| format!("{} ({label}) Tj", if i == 0 { "" } else { "0 -30 Td" }))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let target = "0.85 0.93 1 rg 100 625 180 25 re f 0 0 0 rg BT /F1 16 Tf 100 630 Td (Destination at x=70 y=300) Tj 0 -100 Td (Second marker below target) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R /Names << /Dests << /Names [(chapter two) [4 0 R /XYZ 100 650 2.25]] >> >> >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_owned(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [10 20 310 420] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R /Annots [{annots}] >>"),
            "<< /Type /Page /Parent 2 0 R /MediaBox [30 50 530 950] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len()),
            format!("<< /Length {} >>\nstream\n{target}\nendstream", target.len()),
            "<< /Type /Outlines /First 9 0 R /Last 10 0 R /Count 2 >>".to_owned(),
            "<< /Title (Named XYZ destination) /Parent 8 0 R /Dest (chapter two) /Next 10 0 R >>".to_owned(),
            "<< /Title (Fit width destination) /Parent 8 0 R /Dest [4 0 R /FitH 650] /Prev 9 0 R >>".to_owned(),
        ];
        let mut bytes = "%PDF-1.4\n".to_owned();
        let mut offsets = vec![0];
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
        }
        let xref = bytes.len();
        bytes.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()));
        for offset in &offsets[1..] {
            bytes.push_str(&format!("{offset:010} 00000 n \n"));
        }
        bytes.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
            offsets.len()
        ));
        bytes.into_bytes()
    }

    #[test]
    #[ignore = "exports synthetic link PDF to REVIEW_FIXTURE_DIR for native tests"]
    fn export_links_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        std::fs::write(directory.join("links.pdf"), fixture()).unwrap();
    }

    #[test]
    fn extraction_preserves_named_explicit_fit_and_null_destinations_and_blocks_actions() {
        let document = Document::from_bytes(&fixture(), "application/pdf").unwrap();
        let links = extract(&document, 0).unwrap();
        assert_eq!(links.len(), 8);
        let xyz = mupdf::DestinationKind::XYZ {
            left: Some(70.0),
            top: Some(300.0),
            zoom: Some(225.0),
        };
        for link in &links[..2] {
            let LinkTarget::Internal(dest) = link.target else {
                panic!("expected internal")
            };
            assert_eq!(dest.loc.page_number, 1);
            assert_eq!(dest.kind, xyz);
        }
        assert_eq!(links[0].bounds.min, egui::pos2(0.1, 0.1125));
        assert_eq!(links[0].bounds.max, egui::pos2(0.8, 0.175));
        let external: Vec<_> = links[2..5]
            .iter()
            .map(|link| {
                let LinkTarget::External(url) = &link.target else {
                    panic!("expected external")
                };
                url.as_str()
            })
            .collect();
        assert_eq!(
            external,
            [
                "https://example.org/paper.pdf#page=3",
                "http://example.org/reader",
                "mailto:reader@example.org?subject=Review"
            ]
        );
        for (index, expected) in [
            (5, mupdf::DestinationKind::FitH { top: Some(300.0) }),
            (
                6,
                mupdf::DestinationKind::FitR {
                    left: 70.0,
                    bottom: 300.0,
                    right: 270.0,
                    top: 500.0,
                },
            ),
            (
                7,
                mupdf::DestinationKind::XYZ {
                    left: None,
                    top: Some(300.0),
                    zoom: None,
                },
            ),
        ] {
            let LinkTarget::Internal(dest) = links[index].target else {
                panic!("expected internal")
            };
            assert_eq!(dest.kind, expected);
        }
        let outlines = document.outlines().unwrap();
        assert_eq!(outlines[0].dest.unwrap().kind, xyz);
        assert_eq!(
            outlines[1].dest.unwrap().kind,
            mupdf::DestinationKind::FitH { top: Some(300.0) }
        );
    }

    #[test]
    fn url_policy_rejects_files_programs_controls_and_deceptive_authorities() {
        for input in [
            "https://example.org/paper.pdf#page=3",
            "HTTP://example.org:8080/a?b=c",
            "mailto:reader@example.org?subject=Review",
        ] {
            assert!(SafeUrl::parse(input).is_some(), "{input}");
        }
        for input in [
            "javascript:alert(1)",
            "file:///tmp/a.pdf",
            "data:text/html,hello",
            "ftp://example.org",
            "review:foo",
            "../a.pdf",
            "C:\\a.pdf",
            "//example.org",
            " https://example.org",
            "https:example.org",
            "https://good.org@evil.org",
            "https://a\\b",
            "https://example.org/\n",
            "mailto:r@example.org?subject=x%0aBcc:evil",
            "mailto:",
            "https://",
        ] {
            assert!(SafeUrl::parse(input).is_none(), "{input}");
        }
    }

    #[test]
    fn hit_rect_uses_both_asymmetric_axes_and_clips_to_page() {
        let link = PageLink {
            bounds: Rect::from_min_max(egui::pos2(0.1, 0.3), egui::pos2(0.6, 0.4)),
            target: LinkTarget::External(SafeUrl::parse("https://example.org").unwrap()),
        };
        let rect = link.screen_bounds(Rect::from_min_size(
            egui::pos2(17.0, 43.0),
            Vec2::new(600.0, 800.0),
        ));
        assert_eq!(
            rect,
            Rect::from_min_max(egui::pos2(77.0, 283.0), egui::pos2(377.0, 363.0))
        );
        assert!(rect.contains(egui::pos2(78.0, 300.0)));
        assert!(!rect.contains(egui::pos2(78.0, 280.0)));
        let outside = PageLink {
            bounds: Rect::from_min_max(egui::pos2(-0.1, 0.2), egui::pos2(1.4, 0.5)),
            target: link.target,
        };
        assert_eq!(
            outside.screen_bounds(Rect::from_min_size(
                egui::pos2(17.0, 43.0),
                Vec2::new(600.0, 800.0)
            )),
            Rect::from_min_max(egui::pos2(17.0, 203.0), egui::pos2(617.0, 443.0)),
        );
    }
}
