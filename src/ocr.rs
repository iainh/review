//! Optional local Tesseract CLI integration. Only owned pixels/text cross
//! threads; the PDF is opened and authenticated on the recognition worker.
use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    document::{PdfDocument, WorkerSource},
    structured_text::{PageText, TextChar, TextLine},
};

const MAX_PIXELS: f32 = 8_000_000.0;
const MAX_SIDE: f32 = 8192.0;
const MISSING_ENGINE: &str = "Local OCR needs Tesseract. Install Tesseract and the languages you need, then make tesseract available on PATH and click Retry. Review never downloads language data or uploads PDFs.";

enum Outcome {
    Languages(Vec<String>),
    Recognized {
        page: usize,
        revision: u64,
        text: PageText,
    },
}

enum Event {
    Progress(String),
    Done(Result<Outcome, String>),
}

struct Job {
    cancel: Arc<AtomicBool>,
    events: mpsc::Receiver<Event>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Ocr {
    pub open: bool,
    languages: Option<Vec<String>>,
    language: String,
    job: Option<Job>,
    status: String,
}

impl Ocr {
    pub fn cancel(&mut self) {
        self.job = None;
        self.status = "OCR cancelled. Existing session text is unchanged.".into();
    }

    fn launch(&mut self, ctx: &egui::Context, source: Option<(WorkerSource, usize, u64)>) {
        let (send, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            cancel: cancel.clone(),
            events,
        });
        self.status = if source.is_some() {
            "Preparing page…"
        } else {
            "Checking installed OCR languages…"
        }
        .into();
        let ctx = ctx.clone();
        let language = self.language.clone();
        std::thread::spawn(move || {
            let progress = |status: String| {
                let _ = send.send(Event::Progress(status));
                ctx.request_repaint();
            };
            let result = match source {
                Some((source, page, revision)) => recognize(
                    &source,
                    page,
                    &language,
                    Path::new("tesseract"),
                    &cancel,
                    progress,
                )
                .map(|text| Outcome::Recognized {
                    page,
                    revision,
                    text,
                }),
                None => {
                    installed_languages(Path::new("tesseract"), &cancel).map(Outcome::Languages)
                }
            };
            let _ = send.send(Event::Done(result.map_err(|error| format!("{error:#}"))));
            ctx.request_repaint();
        });
    }

    /// Apply completed text on the UI thread. Cancellation drops the receiver,
    /// so even a completion racing with Cancel cannot change the document.
    pub fn poll(&mut self, document: &PdfDocument) {
        while let Some(job) = &self.job {
            match job.events.try_recv() {
                Ok(Event::Progress(status)) => self.status = status,
                Ok(Event::Done(result)) => {
                    self.job = None;
                    match result {
                        Ok(Outcome::Languages(languages)) => {
                            if self.language.is_empty() {
                                self.language = languages
                                    .iter()
                                    .find(|l| *l == "eng")
                                    .unwrap_or(&languages[0])
                                    .clone();
                            }
                            self.languages = Some(languages);
                            self.status = "Ready. Recognize only the page you choose.".into();
                        }
                        Ok(Outcome::Recognized { revision, .. })
                            if revision != document.text_revision() =>
                        {
                            self.status = "Page text changed during OCR; the stale result was discarded. Recognize the page again.".into();
                        }
                        Ok(Outcome::Recognized { page, text, .. }) => {
                            let empty = text.chars.is_empty();
                            self.status = match document.set_recognized_text(page, text) {
                                Ok(()) if empty => format!(
                                    "Page {}: no text recognized. Try another installed language or a clearer scan.",
                                    page + 1
                                ),
                                Ok(()) => format!(
                                    "Page {} recognized. Text is available for search and assistive reading; copying requires PDF permission.",
                                    page + 1
                                ),
                                Err(error) => format!("OCR failed: {error:#}"),
                            };
                        }
                        Err(error) => self.status = error,
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.status = "OCR worker stopped unexpectedly. Retry recognition.".into();
                }
            }
        }
    }

    pub fn ui(&mut self, root: &mut egui::Ui, document: &PdfDocument) {
        if !self.open {
            return;
        }
        let ctx = root.ctx().clone();
        if self.languages.is_none() && self.job.is_none() && self.status.is_empty() {
            self.launch(&ctx, None);
        }
        egui::Panel::top("ocr_panel").show_inside(root, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Local OCR");
                ui.add_enabled_ui(self.job.is_none(), |ui| {
                    if let Some(languages) = &self.languages {
                        let language_label = ui.label("Language");
                        egui::ComboBox::from_id_salt("ocr_language")
                            .selected_text(&self.language)
                            .show_ui(ui, |ui| {
                                for language in languages {
                                    ui.selectable_value(
                                        &mut self.language,
                                        language.clone(),
                                        language,
                                    );
                                }
                            })
                            .response
                            .labelled_by(language_label.id);
                        if ui
                            .button(format!("Recognize page {}", document.current_page() + 1))
                            .clicked()
                        {
                            self.launch(
                                &ctx,
                                Some((
                                    document.worker_source(),
                                    document.current_page(),
                                    document.text_revision(),
                                )),
                            );
                        }
                    } else if ui.button("Retry").clicked() {
                        self.launch(&ctx, None);
                    }
                });
                if self.job.is_some() {
                    ui.spinner();
                    if ui.button("Cancel").clicked() {
                        self.cancel();
                    }
                }
                if ui.button("Close OCR").clicked() {
                    if self.job.is_some() {
                        self.cancel();
                    }
                    self.open = false;
                }
            });
            ui.label(&self.status);
            ui.small(
                "Scanned pages only · installed languages · session only · original PDF unchanged",
            );
        });
    }
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "OCR cancelled");
    Ok(())
}

fn private_directory() -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("review-ocr-");
    // TempDir defaults to 0777 minus umask. Apply 0700 at creation, not after
    // exposing an image in a traversable directory. Windows inherits TEMP ACLs.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir()
        .context("Could not create a private OCR directory")
}

fn read_bounded(path: &Path, limit: u64) -> Result<String> {
    let mut data = String::new();
    File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut data)?;
    ensure!(
        data.len() as u64 <= limit,
        "OCR output exceeds the text memory limit"
    );
    Ok(data)
}

/// Redirect output into the private directory rather than pipes: a verbose
/// engine cannot deadlock cancellation by filling stdout or stderr.
fn run_engine(
    command: &mut Command,
    directory: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String> {
    let output = directory.join("stdout");
    let errors = directory.join("stderr");
    command
        .stdin(Stdio::null())
        .stdout(File::create(&output)?)
        .stderr(File::create(&errors)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cancelled(cancel)?;
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(MISSING_ENGINE)
        } else {
            anyhow::anyhow!("Could not start local Tesseract: {error}")
        }
    })?;
    let start = Instant::now();
    let result = loop {
        if let Err(error) = cancelled(cancel) {
            break Err(error);
        }
        if start.elapsed() > timeout {
            break Err(anyhow::anyhow!(
                "Local OCR timed out; try a smaller page or another language"
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break read_bounded(&output, 64 * 1024),
            Ok(Some(_)) => {
                break Err(anyhow::anyhow!(
                    "Tesseract failed: {}",
                    read_bounded(&errors, 64 * 1024)
                        .unwrap_or_else(|_| "could not read engine diagnostics".into())
                ));
            }
            Err(error) => break Err(error.into()),
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
        }
    };
    // Kill and reap on cancellation, timeout or I/O failure, before deleting the
    // temporary image. For an already exited child kill is harmless.
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn valid_language(language: &str) -> bool {
    language.split('/').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    })
}

fn installed_languages(engine: &Path, cancel: &AtomicBool) -> Result<Vec<String>> {
    let directory = private_directory()?;
    let output = run_engine(
        Command::new(engine).arg("--list-langs"),
        directory.path(),
        cancel,
        Duration::from_secs(15),
    )?;
    let mut languages: Vec<_> = output
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|language| *language != "osd" && valid_language(language))
        .map(str::to_owned)
        .collect();
    languages.sort();
    languages.dedup();
    ensure!(
        !languages.is_empty(),
        "Tesseract has no installed recognition languages. Install language data yourself, then click Retry. Review does not download it."
    );
    Ok(languages)
}

fn render_scale(size: (f32, f32)) -> Result<f32> {
    ensure!(
        [size.0, size.1].iter().all(|v| v.is_finite() && *v > 0.0),
        "Invalid OCR page dimensions"
    );
    let mut scale = (300.0_f32 / 72.0)
        .min((MAX_SIDE - 2.0) / size.0.max(size.1))
        .min((MAX_PIXELS / (size.0 * size.1)).sqrt());
    ensure!(
        scale.is_finite() && scale > 0.0,
        "OCR page dimensions exceed the rendering limit"
    );
    // Reserve rounding at both edges, including nonzero MediaBox origins.
    while ((size.0 * scale).ceil() + 2.0) * ((size.1 * scale).ceil() + 2.0) > MAX_PIXELS {
        scale *= 0.999;
    }
    Ok(scale)
}

fn recognize(
    source: &WorkerSource,
    page: usize,
    language: &str,
    engine: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(String),
) -> Result<PageText> {
    ensure!(valid_language(language), "Choose an installed OCR language");
    cancelled(cancel)?;
    let document = source.open()?;
    ensure!(
        document.native_text(page)?.chars.is_empty(),
        "Page {} already has native text; OCR will not replace it",
        page + 1
    );
    let size = document.page_size(page)?;
    let directory = private_directory()?;
    progress(format!(
        "Page {}: rendering a bounded local image…",
        page + 1
    ));
    // MuPDF's in-flight raster cannot be interrupted. It is bounded and stays
    // off the UI thread; cancellation is checked before starting the engine.
    let image = document.render_at_scale(page, render_scale(size)?)?;
    cancelled(cancel)?;
    let dimensions = [image.width, image.height];
    let path = directory.path().join("page.ppm");
    let mut file = BufWriter::new(File::create(&path)?);
    write!(file, "P6\n{} {}\n255\n", image.width, image.height)?;
    for row in image.rgba.chunks(image.width as usize * 4) {
        cancelled(cancel)?;
        let rgb: Vec<u8> = row
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| pixel[..3].iter().copied())
            .collect();
        file.write_all(&rgb)?;
    }
    file.flush()?;
    drop(file);
    drop(image);
    progress(format!(
        "Page {}: recognizing with {language} locally…",
        page + 1
    ));
    run_engine(
        Command::new(engine)
            .arg(&path)
            .arg(directory.path().join("text"))
            .arg("-l")
            .arg(language)
            .arg("--dpi")
            .arg("300")
            .arg("tsv"),
        directory.path(),
        cancel,
        Duration::from_secs(300),
    )?;
    cancelled(cancel)?;
    let tsv = read_bounded(&directory.path().join("text.tsv"), 16 * 1024 * 1024)?;
    let text = parse_tsv(&tsv, dimensions, [size.0, size.1])?;
    cancelled(cancel)?;
    Ok(text)
}

/// Tesseract gives word boxes, not glyph outlines. Subdivide each word box by
/// grapheme (RTL words right-to-left); combining marks share their base quad.
/// Line/paragraph order remains Tesseract's layout heuristic.
fn parse_tsv(tsv: &str, image: [u32; 2], size: [f32; 2]) -> Result<PageText> {
    ensure!(image.iter().all(|n| *n > 0), "Invalid OCR image dimensions");
    let mut page = PageText {
        size,
        ..Default::default()
    };
    let mut previous = None;
    let mut line_start = 0;
    let mut paragraph_start = 0;
    for row in tsv.lines().skip(1) {
        let fields: Vec<_> = row.splitn(12, '\t').collect();
        ensure!(fields.len() == 12, "Invalid Tesseract TSV row");
        if fields[0] != "5" || fields[11].trim().is_empty() {
            continue;
        }
        let key: Vec<u32> = fields[1..5]
            .iter()
            .map(|n| n.parse())
            .collect::<Result<_, _>>()?;
        let key: [u32; 4] = key.try_into().unwrap();
        if previous.is_some_and(|previous| previous != key) {
            page.lines.push(TextLine {
                chars: line_start..page.chars.len(),
                direction: [1.0, 0.0],
            });
            page.separator();
            if previous.is_some_and(|previous: [u32; 4]| previous[..3] != key[..3]) {
                page.paragraphs.push(paragraph_start..page.chars.len() - 1);
                page.separator();
                paragraph_start = page.chars.len();
            }
            line_start = page.chars.len();
        } else if previous.is_some() {
            page.chars.push(TextChar {
                ch: ' ',
                quad: None,
                bidi: 0,
            });
        }
        previous = Some(key);
        let bbox: Vec<f32> = fields[6..10]
            .iter()
            .map(|n| n.parse())
            .collect::<Result<_, _>>()?;
        let [x, y, width, height]: [f32; 4] = bbox.try_into().unwrap();
        ensure!(
            [x, y, width, height].iter().all(|n| n.is_finite())
                && x >= 0.0
                && y >= 0.0
                && width > 0.0
                && height > 0.0
                && x + width <= image[0] as f32
                && y + height <= image[1] as f32,
            "Invalid OCR word geometry"
        );
        let word = fields[11];
        let bidi = unicode_bidi::BidiInfo::new(word, None);
        let graphemes: Vec<_> = word.grapheme_indices(true).collect();
        let levels: Vec<_> = graphemes
            .iter()
            .map(|(byte, _)| bidi.levels[*byte])
            .collect();
        let mut positions = vec![0; graphemes.len()];
        for (visual, logical) in unicode_bidi::BidiInfo::reorder_visual(&levels)
            .into_iter()
            .enumerate()
        {
            positions[logical] = visual;
        }
        for (index, (byte, grapheme)) in graphemes.iter().enumerate() {
            let level = bidi.levels[*byte].number() as u16;
            let index = positions[index];
            let left = (x + width * index as f32 / graphemes.len() as f32) / image[0] as f32;
            let right = (x + width * (index + 1) as f32 / graphemes.len() as f32) / image[0] as f32;
            let top = y / image[1] as f32;
            let bottom = (y + height) / image[1] as f32;
            for ch in grapheme.chars() {
                page.chars.push(TextChar {
                    ch,
                    quad: Some([[left, top], [right, top], [right, bottom], [left, bottom]]),
                    bidi: level,
                });
            }
        }
    }
    if previous.is_some() {
        page.lines.push(TextLine {
            chars: line_start..page.chars.len(),
            direction: [1.0, 0.0],
        });
        page.paragraphs.push(paragraph_start..page.chars.len());
    }
    Ok(page)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::document::tests::sample_pdf;

    const HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n";

    pub fn word_text(word: &str) -> PageText {
        parse_tsv(
            &format!("{HEADER}5\t1\t1\t1\t1\t1\t20\t40\t100\t20\t95\t{word}\n"),
            [300, 400],
            [300.0, 400.0],
        )
        .unwrap()
    }

    pub fn scanned_fixture(path: &Path, blank: bool) {
        let content = if blank {
            ""
        } else {
            "BT /F1 20 Tf 40 350 Td (Scanned amber fox) Tj 0 -36 Td (Local violet river) Tj ET"
        };
        let original =
            mupdf::Document::from_bytes(&sample_pdf(content, false), "application/pdf").unwrap();
        let mut writer =
            mupdf::DocumentWriter::new(path.to_str().unwrap(), "pdf", "compress").unwrap();
        for number in 0..2 {
            let page = original.load_page(number).unwrap();
            let bounds = page.bounds().unwrap();
            let pixmap = page
                .to_pixmap(
                    &mupdf::Matrix::new_scale(4.0, 4.0),
                    &mupdf::Colorspace::device_rgb(),
                    false,
                    true,
                )
                .unwrap();
            let image = mupdf::Image::from_pixmap(&pixmap).unwrap();
            let device = writer
                .begin_page(mupdf::Rect::new(0.0, 0.0, bounds.width(), bounds.height()))
                .unwrap();
            device
                .fill_image(
                    &image,
                    &mupdf::Matrix::new_scale(bounds.width(), bounds.height()),
                    1.0,
                    mupdf::ColorParams::default(),
                )
                .unwrap();
            writer.end_page(device).unwrap();
        }
    }

    #[test]
    fn tsv_retains_unicode_paragraphs_asymmetric_geometry_and_rtl_carets() {
        let text = format!(
            "{HEADER}\
5\t1\t1\t1\t1\t1\t20\t40\t90\t20\t95\té中\n\
5\t1\t1\t1\t1\t2\t150\t40\t60\t20\t90\tאב\n\
5\t1\t1\t1\t2\t1\t20\t90\t60\t20\t85\tİx\n\
5\t1\t2\t1\t1\t1\t180\t190\t60\t30\t85\tfin\n"
        );
        let page = parse_tsv(&text, [300, 400], [600.0, 800.0]).unwrap();
        assert_eq!(page.plain_text(), "é中 אב\nİx\n\nfin");
        assert_eq!(page.lines.len(), 3);
        assert_eq!(page.paragraph(2), 0..9);
        assert_eq!(page.paragraph(12), 11..14);
        assert_eq!(page.chars[0].quad, page.chars[1].quad);
        assert_eq!(page.chars[0].quad.unwrap()[0], [20.0 / 300.0, 0.1]);
        assert_eq!(page.chars[2].quad.unwrap()[0], [65.0 / 300.0, 0.1]);
        assert_eq!(page.chars[4].bidi, 1);
        assert_eq!(page.chars[4].quad.unwrap()[0][0], 0.6);
        assert_eq!(page.hit([0.69, 0.12]).unwrap().caret, 4);
        assert_eq!(page.hit([0.61, 0.12]).unwrap().caret, 5);
        assert_eq!(page.search("אב İX").len(), 1);
        assert_eq!(page.search("i\u{307}x")[0].len(), 2);
        assert!(page.search("absent").is_empty());
        for bbox in [
            "NaN\t40\t90\t20",
            "20\t40\t0\t20",
            "290\t40\t90\t20",
            "-1\t40\t90\t20",
        ] {
            assert!(
                parse_tsv(
                    &format!("{HEADER}5\t1\t1\t1\t1\t1\t{bbox}\t95\tbad\n"),
                    [300, 400],
                    [300.0, 400.0]
                )
                .is_err()
            );
        }
    }

    #[test]
    fn image_scale_bounds_area_and_long_thin_pages() {
        assert_eq!(render_scale((300.0, 400.0)).unwrap(), 300.0 / 72.0);
        for size in [(2400.0, 1800.0), (100_000.0, 60.0), (60.0, 100_000.0)] {
            let scale = render_scale(size).unwrap();
            let width = (size.0 * scale).ceil() + 2.0;
            let height = (size.1 * scale).ceil() + 2.0;
            assert!(width * height <= MAX_PIXELS);
            assert!(width.max(height) <= MAX_SIDE);
        }
        for size in [(0.0, 20.0), (f32::NAN, 30.0), (f32::INFINITY, 30.0)] {
            assert!(render_scale(size).is_err());
        }
    }

    #[test]
    fn missing_engine_is_actionable_and_native_text_is_never_replaced() {
        for language in ["eng", "srp_latn", "script/Arabic"] {
            assert!(valid_language(language));
        }
        for language in ["", "../eng", "/eng", "eng/", "eng+fra", "eng;command"] {
            assert!(!valid_language(language));
        }
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-tesseract");
        let error = installed_languages(&missing, &AtomicBool::new(false))
            .unwrap_err()
            .to_string();
        assert_eq!(error, MISSING_ENGINE);
        let path = directory.path().join("native.pdf");
        std::fs::write(
            &path,
            sample_pdf("BT /F1 20 Tf 40 350 Td (native original) Tj ET", false),
        )
        .unwrap();
        let document = PdfDocument::open(&path).unwrap();
        let error = recognize(
            &document.worker_source(),
            0,
            "eng",
            &missing,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("already has native text"));
        assert!(
            document
                .set_recognized_text(0, word_text("replacement"))
                .is_err()
        );
        assert_eq!(document.text_revision(), 0);
        assert_eq!(
            document.structured_text(0).unwrap().plain_text(),
            "native original"
        );
    }

    #[test]
    fn layer_changes_clear_ocr_and_worker_native_text_remains_authoritative() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layers.pdf");
        let bytes = crate::inspection::tests::fixture();
        std::fs::write(&path, &bytes).unwrap();
        let mut document = PdfDocument::open(&path).unwrap();
        let original_source = document.worker_source();
        let mut layers = crate::inspection::Inspection::read(document.pdf())
            .layers
            .unwrap();
        layers[0].enabled = false;
        document.set_layer_visibility(&layers).unwrap();
        assert_eq!(document.text_revision(), 1);
        assert!(document.native_text(0).unwrap().chars.is_empty());
        document
            .set_recognized_text(0, word_text("visible scan"))
            .unwrap();
        let hidden_source = document.worker_source();
        std::thread::spawn(move || {
            let original = original_source.open().unwrap();
            assert_eq!(original.text_revision(), 2);
            assert_eq!(original.page_text(0).unwrap(), "Draft text");
            let hidden = hidden_source.open().unwrap();
            assert_eq!(hidden.page_text(0).unwrap(), "visible scan");
            // Reconstructing a worker's layer view must not invalidate shared OCR.
            assert_eq!(hidden.text_revision(), 2);
        })
        .join()
        .unwrap();
        layers[0].enabled = true;
        document.set_layer_visibility(&layers).unwrap();
        assert_eq!(document.text_revision(), 3);
        assert_eq!(document.page_text(0).unwrap(), "Draft text");
        layers[0].enabled = false;
        document.set_layer_visibility(&layers).unwrap();
        assert_eq!(document.text_revision(), 4);
        assert!(document.page_text(0).unwrap().is_empty());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn cancelled_and_stale_completions_do_not_install_text() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("blank.pdf");
        std::fs::write(&path, sample_pdf("", false)).unwrap();
        let document = PdfDocument::open(&path).unwrap();
        let complete = || {
            let (send, events) = mpsc::channel();
            send.send(Event::Done(Ok(Outcome::Recognized {
                page: 0,
                revision: 0,
                text: word_text("late text"),
            })))
            .unwrap();
            Ocr {
                job: Some(Job {
                    cancel: Arc::new(AtomicBool::new(false)),
                    events,
                }),
                ..Default::default()
            }
        };
        let mut ocr = complete();
        ocr.cancel();
        ocr.poll(&document);
        assert_eq!(document.text_revision(), 0);
        let mut ocr = complete();
        document.invalidate_text();
        ocr.poll(&document);
        assert_eq!(document.text_revision(), 1);
        assert!(document.structured_text(0).unwrap().chars.is_empty());
        assert!(ocr.status.contains("stale result"));
    }

    #[cfg(unix)]
    #[test]
    fn running_engine_can_be_cancelled_and_reaped() {
        let directory = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let error = run_engine(
            Command::new("sh").arg("-c").arg("exec sleep 10"),
            directory.path(),
            &cancel,
            Duration::from_secs(15),
        )
        .unwrap_err();
        trigger.join().unwrap();
        assert!(error.to_string().contains("cancelled"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    #[ignore = "requires locally installed Tesseract with eng language; runs actual OCR"]
    fn actual_local_tesseract_scanned_pdf_session_search_and_permissions() {
        use mupdf::pdf::{Encryption, PdfWriteOptions, Permission};
        let directory = tempfile::tempdir().unwrap();
        let scan = directory.path().join("scan.pdf");
        scanned_fixture(&scan, false);
        let restricted = directory.path().join("restricted.pdf");
        let pdf = mupdf::pdf::PdfDocument::try_from(
            mupdf::Document::from_bytes(&std::fs::read(&scan).unwrap(), "application/pdf").unwrap(),
        )
        .unwrap();
        let mut options = PdfWriteOptions::default();
        options
            .set_encryption(Encryption::Aes256)
            .set_user_password("scan-secret")
            .set_owner_password("owner-secret")
            .set_permissions(Permission::ACCESSIBILITY);
        pdf.save_with_options(restricted.to_str().unwrap(), options)
            .unwrap();
        assert!(
            installed_languages(Path::new("tesseract"), &AtomicBool::new(false))
                .unwrap()
                .contains(&"eng".to_owned())
        );
        for (path, password, copy) in [
            (&scan, None, true),
            (&restricted, Some("scan-secret"), false),
        ] {
            let before = std::fs::read(path).unwrap();
            let document = PdfDocument::open_with_password(path, password)
                .unwrap()
                .unwrap();
            assert_eq!(document.permissions().copy, copy);
            assert!(document.structured_text(0).unwrap().chars.is_empty());
            let source = document.worker_source();
            let text = std::thread::spawn(move || {
                recognize(
                    &source,
                    0,
                    "eng",
                    Path::new("tesseract"),
                    &AtomicBool::new(false),
                    |_| {},
                )
                .unwrap()
            })
            .join()
            .unwrap();
            assert_eq!(
                text.plain_text().split_whitespace().collect::<Vec<_>>(),
                ["Scanned", "amber", "fox", "Local", "violet", "river"]
            );
            assert_eq!(text.lines.len(), 2);
            let quad = text.chars[0].quad.unwrap();
            assert!(
                (quad[0][0] - 0.1).abs() < 0.015,
                "OCR aligns with the scanned image, not an invented origin"
            );
            document.set_recognized_text(0, text).unwrap();
            assert_eq!(document.text_revision(), 1);
            assert_eq!(document.search_page(0, "AMBER FOX local").unwrap().len(), 1);
            assert!(
                document.structured_text(1).unwrap().chars.is_empty(),
                "OCR is per-page, not document-wide"
            );
            let source = document.worker_source();
            std::thread::spawn(move || {
                let worker = source.open().unwrap();
                assert_eq!(worker.text_revision(), 1);
                assert_eq!(
                    worker
                        .structured_text(0)
                        .unwrap()
                        .plain_text()
                        .split_whitespace()
                        .collect::<Vec<_>>(),
                    ["Scanned", "amber", "fox", "Local", "violet", "river"]
                );
                worker.invalidate_text();
            })
            .join()
            .unwrap();
            assert_eq!(document.text_revision(), 2);
            assert!(document.structured_text(0).unwrap().chars.is_empty());
            assert_eq!(
                std::fs::read(path).unwrap(),
                before,
                "OCR never writes the original PDF"
            );
            assert!(
                PdfDocument::open_with_password(path, password)
                    .unwrap()
                    .unwrap()
                    .structured_text(0)
                    .unwrap()
                    .chars
                    .is_empty()
            );
        }
    }

    #[test]
    #[ignore = "exports synthetic scanned PDFs to REVIEW_FIXTURE_DIR for native tests"]
    fn export_ocr_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        let scan = directory.join("scan.pdf");
        scanned_fixture(&scan, false);
        scanned_fixture(&directory.join("blank.pdf"), true);
        std::fs::write(
            directory.join("native.pdf"),
            sample_pdf("BT /F1 20 Tf 40 350 Td (Native original text) Tj ET", false),
        )
        .unwrap();
        let pdf = mupdf::pdf::PdfDocument::try_from(
            mupdf::Document::from_bytes(&std::fs::read(&scan).unwrap(), "application/pdf").unwrap(),
        )
        .unwrap();
        let mut options = mupdf::pdf::PdfWriteOptions::default();
        options
            .set_encryption(mupdf::pdf::Encryption::Aes256)
            .set_user_password("")
            .set_owner_password("owner-secret")
            .set_permissions(mupdf::pdf::Permission::ACCESSIBILITY);
        pdf.save_with_options(
            directory.join("restricted-scan.pdf").to_str().unwrap(),
            options,
        )
        .unwrap();
    }
}
