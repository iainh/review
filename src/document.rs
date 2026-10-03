use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use mupdf::{Colorspace, Document, Matrix, TextPageFlags, text_page::SearchHitResponse};

const PAGE_MARGIN: u32 = 64;

pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct SearchMatch {
    pub page: usize,
    /// One normalized quad per line of a single occurrence, in perimeter order.
    pub quads: Vec<[[f32; 2]; 4]>,
}

pub struct PdfDocument {
    document: Document,
    path: PathBuf,
    page_count: usize,
    current_page: usize,
}

impl PdfDocument {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let document = Document::open(path.as_path())
            .with_context(|| format!("failed to open PDF at {}", path.display()))?;
        let page_count = usize::try_from(
            document
                .page_count()
                .context("failed to read PDF page count")?,
        )
        .context("PDF reported a negative page count")?;
        ensure!(page_count > 0, "PDF contains no pages");

        Ok(Self {
            document,
            path,
            page_count,
            current_page: 0,
        })
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    pub fn current_page(&self) -> usize {
        self.current_page
    }

    pub fn go_to_page(&mut self, page: usize) -> bool {
        if page >= self.page_count || page == self.current_page {
            return false;
        }
        self.current_page = page;
        true
    }

    pub fn change_page(&mut self, delta: i32) -> bool {
        let next = self.current_page.saturating_add_signed(delta as isize);
        self.go_to_page(next)
    }

    pub fn search_page(&self, page_number: usize, query: &str) -> Result<Vec<SearchMatch>> {
        ensure!(page_number < self.page_count, "page is out of range");
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let page_label = page_number + 1;
        let page = self
            .document
            .load_page(page_number as i32)
            .with_context(|| format!("failed to load page {page_label}"))?;
        let bounds = page
            .bounds()
            .with_context(|| format!("failed to read bounds for page {page_label}"))?;
        let text = page
            .to_text_page(TextPageFlags::empty())
            .with_context(|| format!("failed to read text from page {page_label}"))?;
        let mut matches = Vec::new();
        // The callback groups multiline quads into one occurrence and has no
        // fixed hit limit, unlike Page::search.
        text.search_cb(query, &mut matches, |matches, quads| {
            matches.push(SearchMatch {
                page: page_number,
                quads: quads
                    .iter()
                    .map(|quad| {
                        [quad.ul, quad.ur, quad.lr, quad.ll].map(|point| {
                            [
                                (point.x - bounds.x0) / (bounds.x1 - bounds.x0),
                                (point.y - bounds.y0) / (bounds.y1 - bounds.y0),
                            ]
                        })
                    })
                    .collect(),
            });
            SearchHitResponse::ContinueSearch
        })
        .with_context(|| format!("failed to search page {page_label}"))?;
        Ok(matches)
    }

    pub fn outlines(&self) -> Result<Vec<mupdf::Outline>> {
        self.document
            .outlines()
            .context("failed to read PDF outline")
    }

    pub fn render_page(
        &self,
        page_number: usize,
        viewport: (u32, u32),
        zoom: f32,
    ) -> Result<PageImage> {
        ensure!(page_number < self.page_count, "page is out of range");
        let page_label = page_number + 1;
        let page = self
            .document
            .load_page(page_number as i32)
            .with_context(|| format!("failed to load page {page_label}"))?;
        let bounds = page
            .bounds()
            .with_context(|| format!("failed to read bounds for page {page_label}"))?;
        let page_width = bounds.x1 - bounds.x0;
        let page_height = bounds.y1 - bounds.y0;
        ensure!(
            page_width > 0.0 && page_height > 0.0,
            "page has invalid bounds"
        );

        let scale = fit_scale(viewport, (page_width, page_height), zoom);
        let pixmap = page
            .to_pixmap(
                &Matrix::new_scale(scale, scale),
                &Colorspace::device_rgb(),
                false,
                true,
            )
            .with_context(|| format!("failed to rasterize page {page_label}"))?;
        ensure!(pixmap.n() == 3, "MuPDF returned an unexpected pixel format");

        Ok(PageImage {
            width: pixmap.width(),
            height: pixmap.height(),
            rgba: rgb_to_rgba(pixmap.samples()),
        })
    }
}

fn fit_scale(viewport: (u32, u32), page: (f32, f32), zoom: f32) -> f32 {
    let available_width = viewport.0.saturating_sub(PAGE_MARGIN).max(1) as f32;
    let available_height = viewport.1.saturating_sub(PAGE_MARGIN).max(1) as f32;
    (available_width / page.0)
        .min(available_height / page.1)
        .max(0.01)
        * zoom
}

fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    let (pixels, _) = rgb.as_chunks::<3>();
    for pixel in pixels {
        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
    }
    rgba
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{PdfDocument, fit_scale, rgb_to_rgba};

    pub fn sample_document() -> PdfDocument {
        let text =
            "BT /F1 16 Tf 40 350 Td (Alpha alpha) Tj 0 -24 Td (Needle) Tj 0 -24 Td (phrase) Tj ET";
        sample_with_text(text)
    }

    fn sample_with_text(text: &str) -> PdfDocument {
        PdfDocument {
            document: mupdf::Document::from_bytes(&sample_pdf(text, false), "application/pdf")
                .unwrap(),
            path: "sample.pdf".into(),
            page_count: 2,
            current_page: 0,
        }
    }

    fn sample_pdf(text: &str, outline: bool) -> Vec<u8> {
        let last = "BT /F1 16 Tf 30 120 Td (Last alpha) Tj ET";
        let mut objects = vec![
            format!("<< /Type /Catalog /Pages 2 0 R {} >>", if outline { "/Outlines 8 0 R" } else { "" }),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [10 20 310 420] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len()),
            format!("<< /Length {} >>\nstream\n{last}\nendstream", last.len()),
        ];
        if outline {
            objects.extend([
                "<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 2 >>".to_string(),
                "<< /Title (Chapter one) /Parent 8 0 R /Dest [3 0 R /Fit] /First 10 0 R /Last 10 0 R /Count 1 >>".to_string(),
                "<< /Title (Nested chapter two) /Parent 9 0 R /Dest [4 0 R /Fit] >>".to_string(),
            ]);
        }
        let mut pdf = "%PDF-1.4\n".to_string();
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

    #[test]
    #[ignore = "exports a synthetic outline PDF to REVIEW_FIXTURE_DIR for Wayland tests"]
    fn export_wayland_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        std::fs::write(
            directory.join("outline.pdf"),
            sample_pdf("BT /F1 16 Tf 40 350 Td (Chapter one) Tj ET", true),
        )
        .unwrap();
    }

    #[test]
    fn outline_preserves_hierarchy_and_resolves_page_destinations() {
        assert!(sample_document().outlines().unwrap().is_empty());
        let bytes = sample_pdf("", true);
        let document = mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap();
        let outline = document.outlines().unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].title, "Chapter one");
        assert_eq!(outline[0].dest.unwrap().loc.page_number, 0);
        assert_eq!(outline[0].down.len(), 1);
        assert_eq!(outline[0].down[0].title, "Nested chapter two");
        assert_eq!(outline[0].down[0].dest.unwrap().loc.page_number, 1);
    }

    #[test]
    fn preview_renders_the_requested_page_without_changing_navigation() {
        let document = sample_document();
        let image = document.render_page(1, (244, 244), 1.0).unwrap();
        assert_eq!((image.width, image.height), (180, 90));
        assert_eq!(image.rgba.len(), 180 * 90 * 4);
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] < 128)
        );
        assert_eq!(document.current_page(), 0);
        assert!(document.render_page(2, (244, 244), 1.0).is_err());
    }

    #[test]
    fn navigation_rejects_out_of_range_pages_without_moving() {
        let mut document = sample_document();
        assert!(!document.change_page(-1));
        assert!(document.go_to_page(1));
        assert!(!document.go_to_page(1));
        assert!(!document.go_to_page(2));
        assert!(!document.go_to_page(usize::MAX));
        assert!(!document.change_page(1));
        assert_eq!(document.current_page(), 1);
        assert!(document.change_page(-1));
        assert_eq!(document.current_page(), 0);
    }

    #[test]
    fn search_groups_multiline_matches_and_normalizes_coordinates() {
        let document = sample_document();
        let hits = document.search_page(0, "ALPHA").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].page, 0);
        assert!((hits[0].quads[0][0][0] - 0.1).abs() < 0.001);
        assert!(hits[1].quads[0][0][0] > hits[0].quads[0][1][0]);
        let phrase = document.search_page(0, "needle phrase").unwrap();
        assert_eq!(phrase.len(), 1);
        assert_eq!(phrase[0].quads.len(), 2);
        assert!(phrase[0].quads[1][0][1] > phrase[0].quads[0][0][1]);
        assert!(document.search_page(0, "absent").unwrap().is_empty());
        assert!(document.search_page(0, " ").unwrap().is_empty());
        assert!(document.search_page(2, "alpha").is_err());
    }

    #[test]
    fn search_does_not_truncate_dense_pages() {
        let text = format!(
            "BT /F1 8 Tf 40 380 Td {} ET",
            "(alpha) Tj 0 -10 Td ".repeat(30)
        );
        let document = sample_with_text(&text);
        assert_eq!(document.search_page(0, "alpha").unwrap().len(), 30);
    }

    #[test]
    #[ignore = "requires REVIEW_TEST_PDF pointing to the OpenID Connect handbook"]
    fn handbook_search_matches_known_page_numbers() {
        let document = PdfDocument::open(std::env::var("REVIEW_TEST_PDF").unwrap()).unwrap();
        assert_eq!(document.page_count(), 45);
        let pages: Vec<_> = (0..45)
            .flat_map(|page| document.search_page(page, "Recap").unwrap())
            .map(|hit| hit.page + 1)
            .collect();
        assert_eq!(pages, [2, 2, 2, 7, 17, 17, 44]);
    }

    #[test]
    fn fit_scale_uses_the_constraining_dimension() {
        assert_eq!(fit_scale((1064, 864), (500.0, 800.0), 1.0), 1.0);
        assert_eq!(fit_scale((564, 1064), (500.0, 800.0), 1.5), 1.5);
    }

    #[test]
    fn converts_rgb_pixels_to_opaque_rgba() {
        assert_eq!(
            rgb_to_rgba(&[1, 2, 3, 10, 20, 30]),
            [1, 2, 3, 255, 10, 20, 30, 255]
        );
    }
}
