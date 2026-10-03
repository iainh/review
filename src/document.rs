use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, ensure};
use mupdf::{Colorspace, Document, Matrix, TextPageFlags};
use zeroize::Zeroizing;

const PAGE_MARGIN: u32 = 64;

pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct PdfDocument {
    pub(crate) document: mupdf::pdf::PdfDocument,
    // A fresh, unsaved copy for layer visibility. Inspection always uses source.
    layer_document: Option<mupdf::pdf::PdfDocument>,
    layer_settings: Vec<(i32, bool)>,
    path: PathBuf,
    page_count: usize,
    current_page: usize,
    session_password: Option<Arc<Zeroizing<String>>>,
    recognized: Arc<Mutex<RecognizedText>>,
    history: Vec<Snapshot>,
    history_position: usize,
    saved_revision: u64,
    next_revision: u64,
}

struct Snapshot {
    bytes: Arc<Vec<u8>>,
    revision: u64,
}

#[derive(Default)]
struct RecognizedText {
    pages: HashMap<usize, crate::structured_text::PageText>,
    revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdfPermissions {
    pub print: bool,
    pub print_high_quality: bool,
    pub copy: bool,
    pub annotate: bool,
    pub fill_forms: bool,
}

/// Only owned Rust data crosses threads. Open this source on the worker so
/// MuPDF's document and pages stay on the thread that created them.
/// Deliberately not Debug: the session password must never reach logs.
pub struct WorkerSource {
    path: PathBuf,
    password: Option<Arc<Zeroizing<String>>>,
    layers: Vec<(i32, bool)>,
    recognized: Arc<Mutex<RecognizedText>>,
    bytes: Arc<Vec<u8>>,
}

impl WorkerSource {
    pub fn open(&self) -> Result<PdfDocument> {
        let mut document = PdfDocument::from_bytes(
            &self.path,
            self.bytes.clone(),
            self.password.as_deref().map(|p| p.as_str()),
        )?
        .context("PDF password changed; reopen the document")?;
        if !self.layers.is_empty() {
            document.apply_layer_visibility(&self.layers)?;
        }
        document.recognized = self.recognized.clone();
        Ok(document)
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
        let path = path.as_ref();
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to open PDF at {}", path.display()))?;
        Self::from_bytes(path, Arc::new(bytes), password)
    }

    fn from_bytes(
        path: &Path,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<Option<Self>> {
        // Neither the live document nor workers retain an original-file reader.
        // This also permits replacement on Windows after all UI readers drop.
        let mut document = Document::from_bytes(&bytes, "application/pdf")
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
        let mut document = mupdf::pdf::PdfDocument::try_from(document)
            .context("failed to read PDF permissions")?;
        document.disable_js()?;
        let page_count = usize::try_from(
            document
                .page_count()
                .context("failed to read PDF page count")?,
        )
        .context("PDF reported a negative page count")?;
        ensure!(page_count > 0, "PDF contains no pages");

        Ok(Some(Self {
            document,
            layer_document: None,
            layer_settings: Vec::new(),
            path: path.to_path_buf(),
            page_count,
            current_page: 0,
            session_password: password.map(|p| Arc::new(Zeroizing::new(p.to_owned()))),
            recognized: Arc::default(),
            history: vec![Snapshot { bytes, revision: 0 }],
            history_position: 0,
            saved_revision: 0,
            next_revision: 1,
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
            annotate: permissions.contains(Permission::ANNOTATE),
            fill_forms: permissions.intersects(Permission::FORM | Permission::ANNOTATE),
        }
    }

    pub fn worker_source(&self) -> WorkerSource {
        WorkerSource {
            path: self.path.clone(),
            password: self.session_password.clone(),
            layers: self.layer_settings.clone(),
            recognized: self.recognized.clone(),
            bytes: self.history[self.history_position].bytes.clone(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.history[self.history_position].revision != self.saved_revision
    }

    pub fn can_undo(&self) -> bool {
        self.history_position > 0
    }

    pub fn can_redo(&self) -> bool {
        self.history_position + 1 < self.history.len()
    }

    pub fn undo(&mut self) -> Result<()> {
        ensure!(self.can_undo(), "Nothing to undo");
        self.restore(self.history_position - 1)
    }

    pub fn redo(&mut self) -> Result<()> {
        ensure!(self.can_redo(), "Nothing to redo");
        self.restore(self.history_position + 1)
    }

    fn restore(&mut self, position: usize) -> Result<()> {
        let mut restored = Self::from_bytes(
            &self.path,
            self.history[position].bytes.clone(),
            self.session_password.as_deref().map(|p| p.as_str()),
        )?
        .context("Undo PDF could not be authenticated")?;
        if !self.layer_settings.is_empty() {
            restored.apply_layer_visibility(&self.layer_settings)?;
        }
        self.layer_document = restored.layer_document;
        self.document = restored.document;
        self.history_position = position;
        self.invalidate_text();
        Ok(())
    }

    /// Apply an atomic edit on a separate, authenticated, thread-local document.
    /// Snapshot failure leaves the live source and its undo history untouched.
    pub(crate) fn edit(
        &mut self,
        change: impl FnOnce(&mut mupdf::pdf::PdfDocument) -> Result<()>,
    ) -> Result<()> {
        // Do not use the render worker's open path: inspection layer settings
        // are view-only and must never become part of an edited/saved source.
        let mut candidate = Self::from_bytes(
            &self.path,
            self.history[self.history_position].bytes.clone(),
            self.session_password.as_deref().map(|p| p.as_str()),
        )?
        .context("Edit PDF could not be authenticated")?;
        change(&mut candidate.document)?;
        let mut options = mupdf::pdf::PdfWriteOptions::default();
        options.set_encryption(mupdf::pdf::Encryption::Keep);
        let mut bytes = Vec::new();
        candidate
            .document
            .write_to_with_options(&mut bytes, options)?;
        let bytes = Arc::new(bytes);
        let mut reopened = Self::from_bytes(
            &self.path,
            bytes.clone(),
            self.session_password.as_deref().map(|p| p.as_str()),
        )?
        .context("Edited PDF could not be authenticated")?;
        ensure!(
            reopened.page_count == self.page_count && reopened.permissions() == self.permissions(),
            "Edited PDF changed document permissions or page count"
        );
        if !self.layer_settings.is_empty() {
            reopened.apply_layer_visibility(&self.layer_settings)?;
        }
        self.layer_document = reopened.layer_document;
        self.document = reopened.document;
        self.history.truncate(self.history_position + 1);
        self.history.push(Snapshot {
            bytes,
            revision: self.next_revision,
        });
        self.next_revision += 1;
        // Bound history by count and bytes, retaining at least the current and
        // previous versions. No temporary decrypted files are used for undo.
        while self.history.len() > 32
            || (self.history.len() > 2
                && self.history.iter().map(|s| s.bytes.len()).sum::<usize>() > 256 * 1024 * 1024)
        {
            self.history.remove(0);
        }
        self.history_position = self.history.len() - 1;
        self.invalidate_text();
        Ok(())
    }

    pub fn save(&mut self, destination: &Path) -> Result<()> {
        use std::io::Write;
        let source = self.worker_source();
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .context("Could not create a temporary PDF beside the destination")?;
        temporary.write_all(&source.bytes)?;
        temporary.as_file().sync_all()?;
        // Reopen the actual file, authenticate it and load every page before
        // replacing anything. Keep encryption/passwords and all unknown objects.
        let reopened = Self::open_with_password(
            temporary.path(),
            self.session_password.as_deref().map(|p| p.as_str()),
        )?
        .context("Saved PDF could not be authenticated")?;
        ensure!(
            reopened.page_count == self.page_count && reopened.permissions() == self.permissions(),
            "Saved PDF verification failed"
        );
        for page in 0..self.page_count {
            reopened.document.load_page(page as i32)?.bounds()?;
        }
        drop(reopened);
        if let Ok(metadata) = std::fs::metadata(destination) {
            ensure!(
                !metadata.permissions().readonly(),
                "Destination is read-only"
            );
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
        }
        temporary
            .persist(destination)
            .context("Could not replace the destination PDF")?;
        self.path = destination.to_path_buf();
        self.saved_revision = self.history[self.history_position].revision;
        Ok(())
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

    /// Print from the loaded document, independently of display zoom and caches.
    pub fn print_page(&self, page: usize) -> Result<mupdf::Page> {
        ensure!(self.permissions().print, "This PDF does not allow printing");
        ensure!(page < self.page_count, "print page is out of range");
        self.rendering_document()
            .load_page(page as i32)
            .context("failed to load print page")
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn print_pdf(&self) -> Result<Vec<u8>> {
        ensure!(self.permissions().print, "This PDF does not allow printing");
        // DocumentWriter avoids convert_to_pdf's unsafe error cleanup in
        // mupdf 0.8. The private temporary directory is deleted on every exit.
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("print.pdf");
        {
            let mut writer = mupdf::DocumentWriter::new(
                path.to_str().context("Invalid temporary PDF path")?,
                "pdf",
                "compress",
            )?;
            for number in 0..self.page_count {
                let page = self.print_page(number)?;
                let bounds = page.bounds()?;
                ensure!(
                    [bounds.width(), bounds.height()]
                        .iter()
                        .all(|n| n.is_finite() && *n > 0.0),
                    "Invalid print page dimensions"
                );
                let device = writer.begin_page(mupdf::Rect::new(
                    0.0,
                    0.0,
                    bounds.width(),
                    bounds.height(),
                ))?;
                if self.permissions().print_high_quality {
                    page.run(&device, &Matrix::new_translate(-bounds.x0, -bounds.y0))?;
                } else {
                    // Do not pass vector content to PDFKit when the source
                    // permits only degraded printing. Keep at most 150 dpi.
                    let size = (bounds.width(), bounds.height());
                    let scale = (150.0_f32 / 72.0)
                        .min(16_000.0 / size.0.max(size.1))
                        .min((32_000_000.0 / (size.0 * size.1)).sqrt());
                    let pixmap = page.to_pixmap(
                        &Matrix::new(
                            scale,
                            0.0,
                            0.0,
                            scale,
                            -bounds.x0 * scale,
                            -bounds.y0 * scale,
                        ),
                        &Colorspace::device_rgb(),
                        false,
                        true,
                    )?;
                    let image = mupdf::Image::from_pixmap(&pixmap)?;
                    device.fill_image(
                        &image,
                        &Matrix::new_scale(size.0, size.1),
                        1.0,
                        mupdf::ColorParams::default(),
                    )?;
                }
                writer.end_page(device)?;
            }
        } // Finalize the PDF before reading it for PDFKit.
        std::fs::read(&path).context("Failed to read print PDF")
    }

    pub fn pdf(&self) -> &mupdf::pdf::PdfDocument {
        &self.document
    }

    fn rendering_document(&self) -> &Document {
        self.layer_document.as_deref().unwrap_or(&self.document)
    }

    pub fn page_label(&self, page: usize) -> Result<String> {
        ensure!(page < self.page_count, "page is out of range");
        Ok(self.document.page_label(page)?)
    }

    pub fn page_description(&self, page: usize) -> String {
        let physical = (page + 1).to_string();
        match self.page_label(page) {
            Ok(label) if !label.is_empty() && label != physical => {
                format!("{label} (page {physical})")
            }
            _ => format!("Page {physical}"),
        }
    }

    pub fn resolve_page(&self, input: &str) -> Result<usize> {
        let input = input.trim();
        // Numeric input always means physical page, even for numeric PDF labels.
        if !input.is_empty() && input.bytes().all(|c| c.is_ascii_digit()) {
            let number = input
                .parse::<usize>()
                .context("Invalid physical page number")?;
            ensure!(
                (1..=self.page_count).contains(&number),
                "Enter a physical page from 1 to {}",
                self.page_count
            );
            return Ok(number - 1);
        }
        ensure!(
            !input.is_empty(),
            "Enter a physical page number or PDF label"
        );
        let pdf = &self.document;
        let mut destination = None;
        for page in 0..self.page_count {
            if pdf.page_label(page)? == input {
                ensure!(
                    destination.is_none(),
                    "Label {input} is ambiguous; use a physical page number"
                );
                destination = Some(page);
            }
        }
        destination.with_context(|| {
            format!("No page label {input}; enter a physical page number or exact PDF label")
        })
    }

    pub fn set_layer_visibility(&mut self, layers: &[crate::inspection::Layer]) -> Result<()> {
        let settings: Vec<_> = layers
            .iter()
            .filter(|layer| layer.controllable)
            .map(|layer| (layer.reference.xref(), layer.enabled))
            .collect();
        self.apply_layer_visibility(&settings)?;
        self.invalidate_text();
        Ok(())
    }

    pub fn layer_visibility(&self, xref: i32) -> Option<bool> {
        self.layer_settings
            .iter()
            .find_map(|&(id, enabled)| (id == xref).then_some(enabled))
    }

    fn apply_layer_visibility(&mut self, settings: &[(i32, bool)]) -> Result<()> {
        // mupdf-rs's setter edits /D/ON and /D/OFF, but does not reset MuPDF's
        // runtime OCG cache. Recreate a rendering-only document before loading
        // any pages. Never write this copy back to the original file.
        let mut bytes = Zeroizing::new(Vec::new());
        let mut options = mupdf::pdf::PdfWriteOptions::default();
        options.set_encryption(mupdf::pdf::Encryption::None);
        self.document.write_to_with_options(&mut *bytes, options)?;
        let mut copy = mupdf::pdf::PdfDocument::from_bytes(&bytes)?;
        copy.disable_js()?;
        for &(xref, enabled) in settings {
            copy.set_optional_content_enabled(mupdf::pdf::OptionalContentRef::new(xref)?, enabled)?;
        }
        self.layer_document = Some(copy);
        self.layer_settings = settings.to_vec();
        Ok(())
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

    /// Extraction itself is permission-neutral. Copy and accessibility consumers
    /// must independently enforce their respective document permissions.
    pub fn structured_text(&self, page_number: usize) -> Result<crate::structured_text::PageText> {
        let native = self.native_text(page_number)?;
        if !native.chars.is_empty() {
            return Ok(native);
        }
        Ok(self
            .recognized
            .lock()
            .unwrap()
            .pages
            .get(&page_number)
            .cloned()
            .unwrap_or(native))
    }

    /// Session text changes invalidate consumer caches, including accessibility
    /// nodes, selection and any in-flight document search.
    pub fn text_revision(&self) -> u64 {
        self.recognized.lock().unwrap().revision
    }

    /// Call after a successful edit or undo/redo. Preserve the recognized Arc
    /// when replacing the live source so old workers observe this revision too.
    /// Layer visibility changes also call this after updating the active view.
    pub fn invalidate_text(&self) {
        let mut recognized = self.recognized.lock().unwrap();
        recognized.pages.clear();
        recognized.revision += 1;
    }

    pub fn set_recognized_text(
        &self,
        page_number: usize,
        text: crate::structured_text::PageText,
    ) -> Result<()> {
        ensure!(
            self.native_text(page_number)?.chars.is_empty(),
            "This page already has native text; OCR will not replace it"
        );
        let mut recognized = self.recognized.lock().unwrap();
        recognized.pages.insert(page_number, text);
        recognized.revision += 1;
        Ok(())
    }

    pub fn native_text(&self, page_number: usize) -> Result<crate::structured_text::PageText> {
        ensure!(page_number < self.page_count, "page is out of range");
        let page = self.rendering_document().load_page(page_number as i32)?;
        let bounds = page.bounds()?;
        let text = page.to_text_page(TextPageFlags::SEGMENT | TextPageFlags::PARAGRAPH_BREAK)?;
        crate::structured_text::PageText::from_xml(&text.to_xml(page_number as i32)?, bounds)
            .with_context(|| format!("failed to extract text from page {}", page_number + 1))
    }

    pub fn page_text(&self, page_number: usize) -> Result<String> {
        Ok(self.structured_text(page_number)?.plain_text())
    }

    pub fn outlines(&self) -> Result<Vec<mupdf::Outline>> {
        self.document
            .outlines()
            .context("failed to read PDF outline")
    }

    pub fn links(&self, page_number: usize) -> Result<Vec<crate::links::PageLink>> {
        ensure!(page_number < self.page_count, "page is out of range");
        crate::links::extract(&self.document, page_number)
    }

    pub fn page_bounds(&self, page_number: usize) -> Result<mupdf::Rect> {
        ensure!(page_number < self.page_count, "page is out of range");
        Ok(self.document.load_page(page_number as i32)?.bounds()?)
    }

    pub fn page_size(&self, page_number: usize) -> Result<(f32, f32)> {
        ensure!(page_number < self.page_count, "page is out of range");
        let bounds = self
            .rendering_document()
            .load_page(page_number as i32)?
            .bounds()?;
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
            .rendering_document()
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

    /// Rasterize a bounded rectangle in scaled, page-local pixels. Allocate
    /// only this region, never a full-page pixmap (even for enormous pages).
    pub fn render_region(
        &self,
        page_number: usize,
        scale: f32,
        region: [u32; 4],
    ) -> Result<PageImage> {
        ensure!(page_number < self.page_count, "page is out of range");
        ensure!(scale.is_finite() && scale > 0.0, "invalid rendering scale");
        let [x0, y0, x1, y1] = region;
        ensure!(
            x0 < x1 && y0 < y1 && x1 <= i32::MAX as u32 && y1 <= i32::MAX as u32,
            "invalid rendering region"
        );
        ensure!(
            u64::from(x1 - x0) * u64::from(y1 - y0) <= 64_000_000,
            "rendering region exceeds the memory limit"
        );
        let page = self.rendering_document().load_page(page_number as i32)?;
        let bounds = page.bounds()?;
        let width = (f64::from(bounds.x1 - bounds.x0) * f64::from(scale)).ceil();
        let height = (f64::from(bounds.y1 - bounds.y0) * f64::from(scale)).ceil();
        ensure!(
            x1 as f64 <= width && y1 as f64 <= height,
            "rendering region is outside the page"
        );
        // MuPDF's scan conversion can change its last clipped pixel. Render
        // two guard pixels beyond the returned overlap, then discard them.
        let left = x0.saturating_sub(2);
        let top = y0.saturating_sub(2);
        let right = x1.saturating_add(2).min(width as u32);
        let bottom = y1.saturating_add(2).min(height as u32);
        ensure!(
            u64::from(right - left) * u64::from(bottom - top) <= 64_000_000,
            "rendering region exceeds the memory limit"
        );
        let mut pixmap = mupdf::Pixmap::new(
            &Colorspace::device_rgb(),
            left as i32,
            top as i32,
            (right - left) as i32,
            (bottom - top) as i32,
            false,
        )?;
        pixmap.clear_with(255)?;
        let device = mupdf::Device::from_pixmap(&pixmap)?;
        // The transform is identical for every tile, including fractional
        // scales. Nonzero native bounds are normalized exactly once here.
        page.run(&device, &page_raster_matrix(bounds, scale))?;
        drop(device);
        ensure!(pixmap.n() == 3, "MuPDF returned an unexpected pixel format");
        let samples = pixmap.samples();
        let mut rgba = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize * 4);
        for y in y0..y1 {
            let start = ((y - top) as usize * (right - left) as usize + (x0 - left) as usize) * 3;
            for pixel in samples[start..start + (x1 - x0) as usize * 3]
                .as_chunks::<3>()
                .0
            {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        Ok(PageImage {
            width: x1 - x0,
            height: y1 - y0,
            rgba,
        })
    }
}

fn page_raster_matrix(bounds: mupdf::Rect, scale: f32) -> Matrix {
    Matrix::new(
        scale,
        0.0,
        0.0,
        scale,
        -bounds.x0 * scale,
        -bounds.y0 * scale,
    )
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
                    copy: false,
                    annotate: false,
                    fill_forms: false,
                }
            );
            assert_eq!(document.page_count(), 2);
            assert_eq!(
                document.structured_text(0).unwrap().plain_text(),
                "Chapter one"
            );
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
                    copy: true,
                    annotate: true,
                    fill_forms: true,
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
                copy: false,
                annotate: false,
                fill_forms: false,
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
                copy: true,
                annotate: false,
                fill_forms: false,
            }
        );
        assert_eq!(
            sample_document().permissions(),
            super::PdfPermissions {
                print: true,
                print_high_quality: true,
                copy: true,
                annotate: true,
                fill_forms: true,
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
                assert_eq!(
                    document.structured_text(0).unwrap().plain_text(),
                    "Chapter one"
                );
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
        PdfDocument::from_bytes(
            std::path::Path::new("sample.pdf"),
            Arc::new(sample_pdf(text, false)),
            None,
        )
        .unwrap()
        .unwrap()
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
        pdf_objects(&objects)
    }

    pub(crate) fn huge_pdf() -> Vec<u8> {
        let content = "q 1 0 0 -1 10 12020 cm \
            0.15 0.6 0.8 rg 40 200 1600 300 re f \
            0 0 0 RG 3 w 40 210 m 1600 480 l S \
            0 0 0 rg BT /F1 24 Tf 1 0 0 -1 80 120 Tm (Jump farther) Tj ET \
            BT /F1 32 Tf 1 0 0 -1 620 280 Tm (Tile crossing) Tj ET \
            0.7 0.2 0.3 rg 3900 2850 900 500 re f \
            0 0 0 rg BT /F1 24 Tf 1 0 0 -1 4000 3000 Tm (Distant needle) Tj ET Q";
        pdf_objects(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [10 20 20010 12020] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R /Annots [6 0 R] >>".into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Annot /Subtype /Link /Rect [90 11888 290 11925] /Border [0 0 0] /Dest [3 0 R /XYZ 3910 9130 1] >>".into(),
        ])
    }

    pub(crate) fn pdf_objects(objects: &[String]) -> Vec<u8> {
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
    fn text_extraction_preserves_lines_and_does_not_change_pages() {
        let document = sample_document();
        assert_eq!(
            document.page_text(0).unwrap().trim(),
            "Alpha alpha\nNeedle\nphrase"
        );
        assert_eq!(document.page_text(1).unwrap().trim(), "Last alpha");
        assert_eq!(document.current_page(), 0);
        assert!(document.page_text(2).is_err());
        assert!(sample_with_text("").page_text(0).unwrap().trim().is_empty());
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

    pub fn geometry_fixture(path: &std::path::Path) {
        let bytes = sample_pdf(
            "q 1 0 0 rg 50 80 40 80 re f /Blend gs 0 0 1 rg 70 100 50 60 re f Q \
             BT /F1 12 Tf 150 180 Td (Cropped rotation) Tj ET",
            false,
        );
        let pdf = mupdf::pdf::PdfDocument::try_from(
            mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap(),
        )
        .unwrap();
        let mut page = pdf.find_page(0).unwrap();
        for (key, value) in [
            ("CropBox", "[30 60 270 210]"),
            ("Rotate", "90"),
            (
                "Resources",
                "<< /Font << /F1 5 0 R >> /ExtGState << /Blend << /Type /ExtGState /ca 0.5 /CA 0.5 >> >> >>",
            ),
        ] {
            page.dict_put(key, pdf.new_object_from_str(value).unwrap())
                .unwrap();
        }
        drop(page);
        pdf.save(path.to_str().unwrap()).unwrap();
    }

    #[test]
    fn cropped_rotated_transparency_preserves_pixels_text_and_worker_geometry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("geometry.pdf");
        geometry_fixture(&path);
        let document = PdfDocument::open(&path).unwrap();
        // CropBox is 240×150 points; intrinsic clockwise rotation swaps axes.
        assert_eq!(document.page_size(0).unwrap(), (150.0, 240.0));
        assert_eq!(document.page_size(1).unwrap(), (400.0, 200.0));
        let image = document.render_at_scale(0, 1.0).unwrap();
        assert_eq!((image.width, image.height), (150, 240));
        let pixel = |x: usize, y: usize| {
            let offset = (y * image.width as usize + x) * 4;
            &image.rgba[offset..offset + 4]
        };
        assert_eq!(pixel(0, 0), [255, 255, 255, 255]);
        // PDF (x,y) maps to (y−60,x−30) after crop and intrinsic rotation.
        assert_eq!(pixel(25, 25), [255, 0, 0, 255]);
        // Allow alpha quantization and channel rounding in the 8-bit compositor.
        for (point, expected) in [((50, 50), [128, 0, 128]), ((80, 70), [128, 128, 255])] {
            for (actual, expected) in pixel(point.0, point.1)[..3].iter().zip(expected) {
                assert!(
                    actual.abs_diff(expected) <= 2,
                    "incorrect alpha blending at {point:?}: {:?}",
                    pixel(point.0, point.1)
                );
            }
        }
        let text = document.structured_text(0).unwrap();
        assert_eq!(text.plain_text(), "Cropped rotation");
        assert!(text.chars.iter().filter_map(|ch| ch.quad).all(|quad| {
            quad.iter()
                .flatten()
                .all(|coordinate| (0.0..=1.0).contains(coordinate))
        }));
        let source = document.worker_source();
        let worker_image = std::thread::spawn(move || {
            let document = source.open().unwrap();
            assert_eq!(document.page_size(0).unwrap(), (150.0, 240.0));
            document.render_at_scale(0, 1.0).unwrap()
        })
        .join()
        .unwrap();
        assert_eq!(worker_image.rgba, image.rgba);
        assert_eq!(document.current_page(), 0);
    }

    #[test]
    fn broken_startxref_recovers_content_and_page_order() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("repaired.pdf");
        let original = sample_pdf("BT /F1 12 Tf 40 350 Td (Recover this text) Tj ET", false);
        let mut repaired = String::from_utf8(original).unwrap();
        repaired.truncate(repaired.rfind("startxref").unwrap());
        repaired.push_str("startxref\n0\n%%EOF");
        std::fs::write(&path, repaired).unwrap();
        let document = PdfDocument::open(path).unwrap();
        assert_eq!(document.page_count(), 2);
        assert_eq!(document.page_text(0).unwrap(), "Recover this text");
        assert_eq!(document.page_text(1).unwrap(), "Last alpha");
        let image = document.render_at_scale(1, 1.0).unwrap();
        assert_eq!((image.width, image.height), (400, 200));
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] < 128)
        );
    }

    #[test]
    fn raster_matrix_normalizes_asymmetric_nonzero_native_bounds() {
        let matrix = super::page_raster_matrix(mupdf::Rect::new(-37.25, 81.5, 262.75, 481.5), 1.5);
        assert_eq!(
            mupdf::Point::new(-37.25, 81.5).transform(&matrix),
            mupdf::Point::new(0.0, 0.0)
        );
        assert_eq!(
            mupdf::Point::new(262.75, 481.5).transform(&matrix),
            mupdf::Point::new(450.0, 600.0)
        );
        assert_eq!(
            mupdf::Point::new(12.75, 201.5).transform(&matrix),
            mupdf::Point::new(75.0, 180.0)
        );
    }

    #[test]
    fn huge_region_is_bounded_and_keeps_native_text_and_links() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("huge.pdf");
        std::fs::write(&path, huge_pdf()).unwrap();
        let document = PdfDocument::open(path).unwrap();
        assert_eq!(document.page_size(0).unwrap(), (20_000.0, 12_000.0));
        assert!(document.render_at_scale(0, 1.0).is_err());
        let image = document
            .render_region(0, 16.0, [63_900, 47_500, 64_400, 48_300])
            .unwrap();
        assert_eq!((image.width, image.height), (500, 800));
        assert_eq!(image.rgba.len(), 500 * 800 * 4);
        assert!(image.rgba.as_chunks::<4>().0.iter().any(|p| p[0] < 128));
        assert_eq!(document.search_page(0, "Distant needle").unwrap().len(), 1);
        assert_eq!(document.links(0).unwrap().len(), 1);
        for region in [[4, 5, 4, 10], [0, 0, 20_001, 2], [0, 0, u32::MAX, 5]] {
            assert!(document.render_region(0, 1.0, region).is_err());
        }
    }

    #[test]
    #[ignore = "exports a huge page to REVIEW_FIXTURE_DIR for native tile tests"]
    fn export_tiles_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        std::fs::write(directory.join("huge.pdf"), huge_pdf()).unwrap();
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
