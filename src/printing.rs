//! Printing owns physical page sizing; display zoom never enters this path.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use crate::document::PdfDocument;

#[derive(Clone, Copy, Default, PartialEq)]
pub enum Sizing {
    #[default]
    Fit,
    Actual,
}

#[derive(Default)]
pub struct PrintDialog {
    pub open: bool,
    sizing: Sizing,
    requested: bool,
    error: Option<String>,
}

impl PrintDialog {
    pub fn ui(&mut self, ctx: &egui::Context, document: &PdfDocument) {
        if !self.open {
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.open = false;
            self.error = None;
            return;
        }
        egui::Modal::new(egui::Id::new("print_options")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Print PDF");
            ui.horizontal(|ui| {
                ui.label("Page sizing");
                ui.radio_value(&mut self.sizing, Sizing::Fit, "Fit");
                ui.radio_value(&mut self.sizing, Sizing::Actual, "Actual size");
            });
            ui.label(match self.sizing {
                Sizing::Fit => "Scale each PDF page to the printable area.",
                Sizing::Actual => "Print at 100%. Pages larger than the paper may be clipped.",
            });
            ui.label("Choose pages, paper, orientation and available two-sided options in the system print dialog.");
            if !document.permissions().print_high_quality {
                ui.label("This PDF permits only low-resolution printing (up to 150 dpi).");
            }
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Continue…").clicked() {
                    self.requested = true;
                    self.open = false;
                    self.error = None;
                }
                if ui.button("Cancel").clicked() {
                    self.open = false;
                    self.error = None;
                }
            });
        });
    }

    /// Called on the event-loop thread after presenting the egui frame.
    pub fn run_requested(&mut self, document: &PdfDocument, window: &winit::window::Window) {
        if !std::mem::take(&mut self.requested) {
            return;
        }
        let result = if !document.permissions().print {
            Err(anyhow::anyhow!("This PDF does not allow printing"))
        } else {
            #[cfg(target_os = "linux")]
            {
                linux::print(document, self.sizing)
            }
            #[cfg(target_os = "macos")]
            {
                macos::print(document, self.sizing)
            }
            #[cfg(target_os = "windows")]
            {
                windows::print(document, self.sizing, window)
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            {
                Err(anyhow::anyhow!(
                    "Printing is not supported on this platform"
                ))
            }
        };
        if let Err(error) = result {
            self.error = Some(format!("Cannot print: {error:#}"));
            self.open = true;
        }
        window.request_redraw();
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", test))]
struct Placement {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
}

#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn placement(page: (f64, f64), area: (f64, f64), sizing: Sizing) -> anyhow::Result<Placement> {
    anyhow::ensure!(
        [page.0, page.1, area.0, area.1]
            .iter()
            .all(|n| n.is_finite() && *n > 0.0),
        "Invalid print page or paper dimensions"
    );
    let scale = match sizing {
        Sizing::Fit => (area.0 / page.0).min(area.1 / page.1),
        Sizing::Actual => 1.0,
    };
    let width = page.0 * scale;
    let height = page.1 * scale;
    Ok(Placement {
        x: (area.0 - width) / 2.0,
        y: (area.1 - height) / 2.0,
        width,
        height,
        scale,
    })
}

#[cfg(any(target_os = "linux", target_os = "windows", test))]
struct RasterPage {
    width: i32,
    height: i32,
    bgra: Vec<u8>,
    placement: Placement,
}

#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn raster_page(
    document: &PdfDocument,
    page: usize,
    area: (f64, f64),
    sizing: Sizing,
) -> anyhow::Result<RasterPage> {
    let page = document.print_page(page)?;
    let bounds = page.bounds()?;
    let size = (f64::from(bounds.width()), f64::from(bounds.height()));
    let placement = placement(size, area, sizing)?;
    // Low-quality permission caps the source resolution, even when Fit would
    // enlarge the page. Never turn a restricted PDF into high-resolution output.
    let requested_scale = if document.permissions().print_high_quality {
        placement.scale * 300.0 / 72.0
    } else {
        150.0 / 72.0
    };
    // Rasterize one page at a time, at up to 300 dpi. Bound extreme media boxes
    // to 32 million pixels / 16k per edge without changing their physical size.
    let scale = requested_scale
        .min(16_000.0 / size.0.max(size.1))
        .min((32_000_000.0 / (size.0 * size.1)).sqrt()) as f32;
    let pixmap = page.to_pixmap(
        &mupdf::Matrix::new(
            scale,
            0.0,
            0.0,
            scale,
            -bounds.x0 * scale,
            -bounds.y0 * scale,
        ),
        &mupdf::Colorspace::device_rgb(),
        false,
        true,
    )?;
    anyhow::ensure!(pixmap.n() == 3, "Unexpected print pixel format");
    let bgra = pixmap
        .samples()
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], 255])
        .collect();
    Ok(RasterPage {
        width: pixmap.width() as i32,
        height: pixmap.height() as i32,
        bgra,
        placement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_and_actual_size_use_points_and_centre_without_distortion() {
        let fit = placement((400.0, 200.0), (300.0, 500.0), Sizing::Fit).unwrap();
        assert_eq!(
            (fit.x, fit.y, fit.width, fit.height, fit.scale),
            (0.0, 175.0, 300.0, 150.0, 0.75)
        );
        let landscape = placement((400.0, 200.0), (500.0, 300.0), Sizing::Fit).unwrap();
        assert_eq!(
            (landscape.x, landscape.y, landscape.width, landscape.height),
            (0.0, 25.0, 500.0, 250.0)
        );
        let actual = placement((400.0, 200.0), (300.0, 500.0), Sizing::Actual).unwrap();
        assert_eq!(
            (
                actual.x,
                actual.y,
                actual.width,
                actual.height,
                actual.scale
            ),
            (-50.0, 150.0, 400.0, 200.0, 1.0)
        );
        for invalid in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert!(placement((invalid, 100.0), (300.0, 500.0), Sizing::Fit).is_err());
            assert!(placement((100.0, 200.0), (invalid, 500.0), Sizing::Actual).is_err());
        }
    }

    #[test]
    fn print_raster_is_independent_of_navigation_and_normalizes_media_box_origin() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("colours.pdf");
        std::fs::write(
            &path,
            crate::document::tests::sample_pdf(
                "q 1 0 0 rg 20 30 40 60 re f Q q 0 0 1 rg 250 350 50 50 re f Q",
                false,
            ),
        )
        .unwrap();
        let document = PdfDocument::open(&path).unwrap();
        let image = raster_page(&document, 0, (300.0, 400.0), Sizing::Actual).unwrap();
        assert_eq!((image.width, image.height), (1250, 1667));
        assert_eq!(
            (image.placement.width, image.placement.height),
            (300.0, 400.0)
        );
        assert_eq!(
            image.bgra.len(),
            image.width as usize * image.height as usize * 4
        );
        // The fixture's MediaBox starts at (10,20). PDF coordinates run upward;
        // the top-down native bitmap must keep red below left and blue above right.
        let pixel = |x: usize, y: usize| &image.bgra[(y * image.width as usize + x) * 4..][..4];
        assert_eq!(pixel(125, 1458), [0, 0, 255, 255]);
        assert_eq!(pixel(1083, 166), [255, 0, 0, 255]);
        assert!(image.bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        assert_eq!(document.current_page(), 0);
        assert!(raster_page(&document, 2, (300.0, 400.0), Sizing::Fit).is_err());
    }

    #[test]
    fn pdfkit_snapshot_preserves_page_sizes_text_and_order() {
        let document = crate::document::tests::sample_document();
        let bytes = document.print_pdf().unwrap();
        let copy = mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap();
        assert_eq!(copy.page_count().unwrap(), 2);
        for (number, size) in [(0, (300.0, 400.0)), (1, (400.0, 200.0))] {
            let page = copy.load_page(number).unwrap();
            let bounds = page.bounds().unwrap();
            assert_eq!((bounds.width(), bounds.height()), size);
            assert!(!page.search("alpha", 10).unwrap().is_empty());
        }
    }

    #[test]
    fn permissions_block_printing_and_cap_restricted_sources_even_when_enlarged() {
        use mupdf::pdf::{Encryption, Permission};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restricted.pdf");
        crate::document::tests::encrypted_fixture(
            &path,
            "user",
            Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
        let denied = PdfDocument::open_with_password(&path, Some("user"))
            .unwrap()
            .unwrap();
        assert!(denied.print_page(0).is_err());
        assert!(denied.print_pdf().is_err());
        assert!(raster_page(&denied, 0, (900.0, 1200.0), Sizing::Fit).is_err());
        drop(denied);

        crate::document::tests::encrypted_fixture(
            &path,
            "user",
            Permission::PRINT | Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
        let restricted = PdfDocument::open_with_password(&path, Some("user"))
            .unwrap()
            .unwrap();
        let raster = raster_page(&restricted, 0, (900.0, 1200.0), Sizing::Fit).unwrap();
        assert_eq!((raster.width, raster.height), (625, 834));
        assert_eq!(
            (raster.placement.width, raster.placement.height),
            (900.0, 1200.0)
        );
        let bytes = restricted.print_pdf().unwrap();
        let copy = mupdf::pdf::PdfDocument::try_from(
            mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap(),
        )
        .unwrap();
        assert_eq!(copy.page_count().unwrap(), 2);
        let page = copy.load_page(0).unwrap();
        assert!(page.search("Chapter one", 10).unwrap().is_empty());
        let resources = copy
            .find_page(0)
            .unwrap()
            .get_dict("Resources")
            .unwrap()
            .unwrap();
        let images = resources.get_dict("XObject").unwrap().unwrap();
        assert_eq!(images.dict_len().unwrap(), 1);
        let image = images.get_dict_val(0).unwrap().unwrap();
        assert_eq!(
            image.get_dict("Width").unwrap().unwrap().as_int().unwrap(),
            625
        );
        assert_eq!(
            image.get_dict("Height").unwrap().unwrap().as_int().unwrap(),
            834
        );
        let pixmap = page
            .to_pixmap(
                &mupdf::Matrix::IDENTITY,
                &mupdf::Colorspace::device_rgb(),
                false,
                true,
            )
            .unwrap();
        assert!(pixmap.samples().iter().any(|value| *value < 128));

        let owner = PdfDocument::open_with_password(&path, Some("owner-secret"))
            .unwrap()
            .unwrap();
        let raster = raster_page(&owner, 0, (300.0, 400.0), Sizing::Actual).unwrap();
        assert_eq!((raster.width, raster.height), (1250, 1667));
        let bytes = owner.print_pdf().unwrap();
        let copy = mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap();
        assert!(
            !copy
                .load_page(0)
                .unwrap()
                .search("Chapter one", 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    #[ignore = "exports restricted synthetic PDFs for printer-free Wayland checks"]
    fn export_print_permission_fixtures() {
        use mupdf::pdf::{Encryption, Permission};
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        for (name, permissions) in [
            ("no-print.pdf", Permission::ACCESSIBILITY),
            (
                "low-quality.pdf",
                Permission::PRINT | Permission::ACCESSIBILITY,
            ),
        ] {
            crate::document::tests::encrypted_fixture(
                &directory.join(name),
                "",
                permissions,
                Encryption::Aes256,
            );
        }
    }
}
