use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use mupdf::{Colorspace, Document, Matrix};

const PAGE_MARGIN: u32 = 64;

pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
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
        let page_count = usize::try_from(document.page_count()?)
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

    pub fn render_current(&self, viewport: (u32, u32), zoom: f32) -> Result<PageImage> {
        let page = self.document.load_page(self.current_page as i32)?;
        let bounds = page.bounds()?;
        let page_width = bounds.x1 - bounds.x0;
        let page_height = bounds.y1 - bounds.y0;
        ensure!(
            page_width > 0.0 && page_height > 0.0,
            "page has invalid bounds"
        );

        let scale = fit_scale(viewport, (page_width, page_height), zoom);
        let pixmap = page.to_pixmap(
            &Matrix::new_scale(scale, scale),
            &Colorspace::device_rgb(),
            false,
            true,
        )?;
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
        let last = "BT /F1 16 Tf 30 120 Td (Last alpha) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [10 20 310 420] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len()),
            format!("<< /Length {} >>\nstream\n{last}\nendstream", last.len()),
        ];
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
        PdfDocument {
            document: mupdf::Document::from_bytes(pdf.as_bytes(), "application/pdf").unwrap(),
            path: "sample.pdf".into(),
            page_count: 2,
            current_page: 0,
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
