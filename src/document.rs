use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use mupdf::{Colorspace, Document, Matrix, TextPageFlags, text_page::SearchHitResponse};
use zeroize::Zeroizing;

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
    document: mupdf::pdf::PdfDocument,
    path: PathBuf,
    page_count: usize,
    current_page: usize,
    session_password: Option<Arc<Zeroizing<String>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdfPermissions {
    pub print: bool,
    pub print_high_quality: bool,
    pub copy: bool,
}

/// Only owned Rust data crosses threads. Open this source on the worker so
/// MuPDF's document and pages stay on the thread that created them.
/// Deliberately not Debug: the session password must never reach logs.
pub struct WorkerSource {
    path: PathBuf,
    password: Option<Arc<Zeroizing<String>>>,
}

impl WorkerSource {
    pub fn open(&self) -> Result<PdfDocument> {
        PdfDocument::open_with_password(&self.path, self.password.as_deref().map(|p| p.as_str()))?
            .context("PDF password changed; reopen the document")
    }
}

impl PdfDocument {
    #[allow(dead_code)] // Also used by document/sidebar fixture tests.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_password(path, None)?.context("PDF requires a password")
    }

    /// None means authentication is required or the supplied password was
    /// incorrect. Other failures are errors. No viewer state changes here.
    pub fn open_with_password(
        path: impl AsRef<Path>,
        password: Option<&str>,
    ) -> Result<Option<Self>> {
        let path = path.as_ref().to_path_buf();
        #[cfg(windows)]
        let mupdf_path = path.to_str().context("PDF path is not valid UTF-8")?;
        #[cfg(not(windows))]
        let mupdf_path = path.as_path();
        let mut document = Document::open(mupdf_path)
            .with_context(|| format!("failed to open PDF at {}", path.display()))?;
        ensure!(
            document.is_pdf(),
            "{} is not a PDF document",
            path.display()
        );
        // This also authenticates PDFs with an empty user password. Do not
        // repeat it after authentication: it can reset owner access.
        let needs_password = document
            .needs_password()
            .context("failed to read PDF encryption")?;
        if let Some(password) = password {
            if !document
                .authenticate(password)
                .context("failed to authenticate PDF")?
            {
                return Ok(None);
            }
        } else if needs_password {
            return Ok(None);
        }
        let document = mupdf::pdf::PdfDocument::try_from(document)
            .context("failed to read PDF permissions")?;
        let page_count = usize::try_from(
            document
                .page_count()
                .context("failed to read PDF page count")?,
        )
        .context("PDF reported a negative page count")?;
        ensure!(page_count > 0, "PDF contains no pages");

        Ok(Some(Self {
            document,
            path,
            page_count,
            current_page: 0,
            session_password: password.map(|p| Arc::new(Zeroizing::new(p.to_owned()))),
        }))
    }

    pub fn permissions(&self) -> PdfPermissions {
        use mupdf::pdf::Permission;
        let permissions = self.document.permissions();
        PdfPermissions {
            print: permissions.contains(Permission::PRINT),
            print_high_quality: permissions.contains(Permission::PRINT)
                && permissions.contains(Permission::PRINT_HQ),
            copy: permissions.contains(Permission::COPY),
        }
    }

    pub fn worker_source(&self) -> WorkerSource {
        WorkerSource {
            path: self.path.clone(),
            password: self.session_password.clone(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
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

    pub fn page_size(&self, page_number: usize) -> Result<(f32, f32)> {
        ensure!(page_number < self.page_count, "page is out of range");
        let bounds = self.document.load_page(page_number as i32)?.bounds()?;
        let size = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
        ensure!(size.0 > 0.0 && size.1 > 0.0, "page has invalid bounds");
        Ok(size)
    }

    #[allow(dead_code)] // Compatibility helper for document/worker fixture tests.
    pub fn render_page(
        &self,
        page_number: usize,
        viewport: (u32, u32),
        zoom: f32,
    ) -> Result<PageImage> {
        let size = self.page_size(page_number)?;
        self.render_at_scale(page_number, fit_scale(viewport, size, zoom))
    }

    pub fn render_at_scale(&self, page_number: usize, scale: f32) -> Result<PageImage> {
        ensure!(page_number < self.page_count, "page is out of range");
        ensure!(scale.is_finite() && scale > 0.0, "invalid rendering scale");
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

        ensure!(
            (page_width * scale).ceil() * (page_height * scale).ceil() <= 64_000_000.0,
            "This zoom exceeds the page rendering memory limit; reduce the zoom"
        );
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
    use mupdf::pdf::{Encryption, PdfWriteOptions, Permission};
    use std::sync::Arc;

    pub fn encrypted_fixture(
        path: &std::path::Path,
        user_password: &str,
        permissions: Permission,
        encryption: Encryption,
    ) {
        let bytes = sample_pdf("BT /F1 16 Tf 40 350 Td (Chapter one) Tj ET", true);
        let pdf = mupdf::pdf::PdfDocument::try_from(
            mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap(),
        )
        .unwrap();
        let mut options = PdfWriteOptions::default();
        options
            .set_encryption(encryption)
            .set_owner_password("owner-secret")
            .set_user_password(user_password)
            .set_permissions(permissions);
        pdf.save_with_options(path.to_str().unwrap(), options)
            .unwrap();
    }

    #[test]
    fn encrypted_pdf_authentication_and_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked résumé.pdf");
        for encryption in [Encryption::Aes256, Encryption::Aes128, Encryption::Rc4_128] {
            encrypted_fixture(&path, "open-secret", Permission::ACCESSIBILITY, encryption);
            for password in [None, Some(""), Some("wrong"), Some("open-secreT")] {
                assert!(
                    PdfDocument::open_with_password(&path, password)
                        .unwrap()
                        .is_none()
                );
            }
            let document = PdfDocument::open_with_password(&path, Some("open-secret"))
                .unwrap()
                .unwrap();
            assert_eq!(
                document.permissions(),
                super::PdfPermissions {
                    print: false,
                    print_high_quality: false,
                    copy: false
                }
            );
            assert_eq!(document.page_count(), 2);
            assert_eq!(document.search_page(0, "Chapter").unwrap().len(), 1);
            assert_eq!(document.outlines().unwrap()[0].down.len(), 1);
            assert!(document.render_page(1, (244, 244), 1.0).is_ok());
            let owner = PdfDocument::open_with_password(&path, Some("owner-secret"))
                .unwrap()
                .unwrap();
            assert_eq!(
                owner.permissions(),
                super::PdfPermissions {
                    print: true,
                    print_high_quality: true,
                    copy: true
                }
            );
        }
    }

    #[test]
    fn empty_password_and_low_quality_print_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restricted.pdf");
        encrypted_fixture(
            &path,
            "",
            Permission::PRINT | Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
        let document = PdfDocument::open(&path).unwrap();
        assert_eq!(
            document.permissions(),
            super::PdfPermissions {
                print: true,
                print_high_quality: false,
                copy: false
            }
        );
        assert!(document.session_password.is_none());
        drop(document);
        encrypted_fixture(
            &path,
            "",
            Permission::COPY | Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
        assert_eq!(
            PdfDocument::open(&path).unwrap().permissions(),
            super::PdfPermissions {
                print: false,
                print_high_quality: false,
                copy: true
            }
        );
        assert_eq!(
            sample_document().permissions(),
            super::PdfPermissions {
                print: true,
                print_high_quality: true,
                copy: true
            }
        );
    }

    #[test]
    fn worker_source_reauthenticates_on_its_own_thread() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked.pdf");
        encrypted_fixture(
            &path,
            "open-secret",
            Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
        for password in ["open-secret", "owner-secret"] {
            let document = PdfDocument::open_with_password(&path, Some(password))
                .unwrap()
                .unwrap();
            let permissions = document.permissions();
            let source = document.worker_source();
            let secret = Arc::downgrade(document.session_password.as_ref().unwrap());
            drop(document);
            assert!(secret.upgrade().is_some());
            std::thread::spawn(move || {
                let document = source.open().unwrap();
                assert_eq!(document.permissions(), permissions);
                assert_eq!(document.search_page(0, "Chapter").unwrap().len(), 1);
                assert!(document.render_page(1, (244, 244), 1.0).is_ok());
            })
            .join()
            .unwrap();
            assert!(secret.upgrade().is_none());
        }
    }

    #[test]
    fn opens_pdf_with_spaces_and_unicode_in_filename() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("résumé 日本語 document.pdf");
        std::fs::write(&path, sample_pdf("", false)).unwrap();
        let document = PdfDocument::open(&path).unwrap();
        assert_eq!(document.path(), path);
        assert_eq!(document.name(), "résumé 日本語 document.pdf");
        assert_eq!(document.page_count(), 2);
        assert_eq!(document.current_page(), 0);
        assert!(document.render_page(1, (244, 244), 1.0).is_ok());
    }

    // APFS rejects invalid UTF-8 filenames before Review can open them.
    #[cfg(target_os = "linux")]
    #[test]
    fn opens_pdf_with_non_utf8_filename() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(OsStr::from_bytes(b"review-\xff.pdf"));
        std::fs::write(&path, sample_pdf("", false)).unwrap();
        let document = PdfDocument::open(&path).unwrap();
        assert_eq!(document.path(), path);
        assert_eq!(document.page_count(), 2);
    }

    pub fn sample_document() -> PdfDocument {
        let text =
            "BT /F1 16 Tf 40 350 Td (Alpha alpha) Tj 0 -24 Td (Needle) Tj 0 -24 Td (phrase) Tj ET";
        sample_with_text(text)
    }

    fn sample_with_text(text: &str) -> PdfDocument {
        PdfDocument {
            document: mupdf::pdf::PdfDocument::try_from(
                mupdf::Document::from_bytes(&sample_pdf(text, false), "application/pdf").unwrap(),
            )
            .unwrap(),
            path: "sample.pdf".into(),
            page_count: 2,
            current_page: 0,
            session_password: None,
        }
    }

    pub(crate) fn sample_pdf(text: &str, outline: bool) -> Vec<u8> {
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
        encrypted_fixture(
            &directory.join("locked.pdf"),
            "open-secret",
            Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
        encrypted_fixture(
            &directory.join("restricted.pdf"),
            "",
            Permission::PRINT | Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
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
    fn explicit_scale_respects_page_bounds_and_rejects_invalid_scales() {
        let document = sample_document();
        assert_eq!(document.page_size(0).unwrap(), (300.0, 400.0));
        let image = document.render_at_scale(0, 2.0).unwrap();
        assert_eq!((image.width, image.height), (600, 800));
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(document.render_at_scale(0, scale).is_err());
        }
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
