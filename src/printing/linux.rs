//! GTK owns ranges, copies, orientation, duplex, spooling and print-to-file.
//! Load it lazily: viewing PDFs does not require GTK or its development files.
use std::{
    ffi::{CStr, CString, c_char, c_double, c_int, c_ulong, c_void},
    ptr,
    sync::OnceLock,
};

use anyhow::{Context, Result, ensure};
use libloading::Library;

use super::{Sizing, raster_page};
use crate::document::PdfDocument;

type Pointer = *mut c_void;

// These signatures follow GTK3, GObject, GLib and Cairo's public C headers.
// Keep the library loaded for the process lifetime: GTK owns global objects.
macro_rules! api {
    ($($name:ident: $signature:ty),* $(,)?) => {
        struct Gtk { $($name: $signature,)* _library: Library }
        impl Gtk {
            unsafe fn load() -> Result<Self> {
                // SAFETY: GTK3 has the C ABI below; loading runs on the GUI thread.
                let library = unsafe { Library::new("libgtk-3.so.0") }
                    .context("Linux printing requires the GTK 3 runtime (libgtk-3.so.0)")?;
                Ok(Self {
                    // SAFETY: All symbols use their documented C signatures.
                    $($name: unsafe { *library.get(concat!(stringify!($name), "\0").as_bytes())? },)*
                    _library: library,
                })
            }
        }
    }
}

api! {
    gtk_init_check: unsafe extern "C" fn(Pointer, Pointer) -> c_int,
    gtk_print_operation_new: unsafe extern "C" fn() -> Pointer,
    gtk_print_operation_set_n_pages: unsafe extern "C" fn(Pointer, c_int),
    gtk_print_operation_set_current_page: unsafe extern "C" fn(Pointer, c_int),
    gtk_print_operation_set_unit: unsafe extern "C" fn(Pointer, c_int),
    gtk_print_operation_set_embed_page_setup: unsafe extern "C" fn(Pointer, c_int),
    gtk_print_operation_set_job_name: unsafe extern "C" fn(Pointer, *const c_char),
    gtk_print_operation_run: unsafe extern "C" fn(Pointer, c_int, Pointer, *mut *mut GError) -> c_int,
    gtk_print_operation_cancel: unsafe extern "C" fn(Pointer),
    gtk_print_operation_get_error: unsafe extern "C" fn(Pointer, *mut *mut GError),
    gtk_print_context_get_width: unsafe extern "C" fn(Pointer) -> c_double,
    gtk_print_context_get_height: unsafe extern "C" fn(Pointer) -> c_double,
    gtk_print_context_get_cairo_context: unsafe extern "C" fn(Pointer) -> Pointer,
    g_signal_connect_data: unsafe extern "C" fn(Pointer, *const c_char, unsafe extern "C" fn(), Pointer, Pointer, c_int) -> c_ulong,
    g_object_unref: unsafe extern "C" fn(Pointer),
    g_error_free: unsafe extern "C" fn(*mut GError),
    cairo_image_surface_create_for_data: unsafe extern "C" fn(*mut u8, c_int, c_int, c_int, c_int) -> Pointer,
    cairo_surface_status: unsafe extern "C" fn(Pointer) -> c_int,
    cairo_surface_destroy: unsafe extern "C" fn(Pointer),
    cairo_save: unsafe extern "C" fn(Pointer),
    cairo_restore: unsafe extern "C" fn(Pointer),
    cairo_translate: unsafe extern "C" fn(Pointer, c_double, c_double),
    cairo_scale: unsafe extern "C" fn(Pointer, c_double, c_double),
    cairo_set_source_surface: unsafe extern "C" fn(Pointer, Pointer, c_double, c_double),
    cairo_paint: unsafe extern "C" fn(Pointer),
    cairo_status: unsafe extern "C" fn(Pointer) -> c_int,
}

#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: *mut c_char,
}

fn gtk() -> Result<&'static Gtk> {
    static GTK: OnceLock<Result<Gtk, String>> = OnceLock::new();
    GTK.get_or_init(|| {
        // SAFETY: Loading only resolves symbols; GTK initialization happens below.
        unsafe { Gtk::load() }.map_err(|error| format!("{error:#}"))
    })
    .as_ref()
    .map_err(|error| anyhow::anyhow!(error.clone()))
}

struct Drawing<'a> {
    gtk: &'static Gtk,
    document: &'a PdfDocument,
    sizing: Sizing,
    error: Option<String>,
    #[cfg(test)]
    pages: Vec<usize>,
}

impl Drawing<'_> {
    fn draw(&mut self, context: Pointer, number: c_int) -> Result<()> {
        let gtk = self.gtk;
        // SAFETY: GTK lends this context to draw-page on the GUI thread.
        unsafe {
            let mut image = raster_page(
                self.document,
                number as usize,
                (
                    (gtk.gtk_print_context_get_width)(context),
                    (gtk.gtk_print_context_get_height)(context),
                ),
                self.sizing,
            )?;
            let cr = (gtk.gtk_print_context_get_cairo_context)(context);
            // CAIRO_FORMAT_ARGB32 uses native-endian premultiplied pixels.
            #[cfg(target_endian = "big")]
            for p in image.bgra.chunks_exact_mut(4) {
                p.reverse();
            }
            let surface = (gtk.cairo_image_surface_create_for_data)(
                image.bgra.as_mut_ptr(),
                0,
                image.width,
                image.height,
                image.width * 4,
            );
            if (gtk.cairo_surface_status)(surface) != 0 {
                (gtk.cairo_surface_destroy)(surface);
                anyhow::bail!("Failed to create print image surface");
            }
            (gtk.cairo_save)(cr);
            (gtk.cairo_translate)(cr, image.placement.x, image.placement.y);
            (gtk.cairo_scale)(
                cr,
                image.placement.width / f64::from(image.width),
                image.placement.height / f64::from(image.height),
            );
            (gtk.cairo_set_source_surface)(cr, surface, 0.0, 0.0);
            (gtk.cairo_paint)(cr);
            // Restore releases the context's reference to our source surface
            // before its borrowed pixel storage goes out of scope.
            (gtk.cairo_restore)(cr);
            (gtk.cairo_surface_destroy)(surface);
            ensure!((gtk.cairo_status)(cr) == 0, "Failed to draw print page");
        }
        #[cfg(test)]
        self.pages.push(number as usize);
        Ok(())
    }
}

unsafe extern "C" fn draw_page(operation: Pointer, context: Pointer, number: c_int, data: Pointer) {
    // SAFETY: Synchronous run retains this stack value until all signals return.
    let drawing = unsafe { &mut *data.cast::<Drawing<'_>>() };
    // Do not unwind through GTK's C callback boundary.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drawing.draw(context, number)
    }));
    let error = match result {
        Ok(Ok(())) => return,
        Ok(Err(error)) => format!("{error:#}"),
        Err(_) => "Print renderer panicked".into(),
    };
    drawing.error = Some(error);
    // SAFETY: The signal supplies the live print operation.
    unsafe { (drawing.gtk.gtk_print_operation_cancel)(operation) };
}

pub(super) fn print(document: &PdfDocument, sizing: Sizing) -> Result<()> {
    run(document, sizing, 0, |_, _| {})?; // GTK_PRINT_OPERATION_ACTION_PRINT_DIALOG
    Ok(())
}

fn run(
    document: &PdfDocument,
    sizing: Sizing,
    action: c_int,
    configure: impl FnOnce(&Gtk, Pointer),
) -> Result<Drawing<'_>> {
    let gtk = gtk()?;
    let mut drawing = Drawing {
        gtk,
        document,
        sizing,
        error: None,
        #[cfg(test)]
        pages: vec![],
    };
    let title = CString::new(document.name()).context("Invalid print job name")?;
    // SAFETY: All GTK calls run synchronously on the same GUI thread. The
    // operation owns no Rust data; its draw-page signal borrows drawing only
    // until run returns. NULL parent is required: winit is not a GtkWindow.
    unsafe {
        ensure!(
            (gtk.gtk_init_check)(ptr::null_mut(), ptr::null_mut()) != 0,
            "GTK could not connect to the display"
        );
        let operation = (gtk.gtk_print_operation_new)();
        (gtk.gtk_print_operation_set_n_pages)(operation, document.page_count() as c_int);
        (gtk.gtk_print_operation_set_current_page)(operation, document.current_page() as c_int);
        (gtk.gtk_print_operation_set_unit)(operation, 1); // GTK_UNIT_POINTS
        (gtk.gtk_print_operation_set_embed_page_setup)(operation, 1);
        (gtk.gtk_print_operation_set_job_name)(operation, title.as_ptr());
        (gtk.g_signal_connect_data)(
            operation,
            c"draw-page".as_ptr(),
            std::mem::transmute::<
                unsafe extern "C" fn(Pointer, Pointer, c_int, Pointer),
                unsafe extern "C" fn(),
            >(draw_page),
            (&mut drawing as *mut Drawing<'_>).cast(),
            ptr::null_mut(),
            0,
        );
        configure(gtk, operation);
        let mut error = ptr::null_mut();
        let result = (gtk.gtk_print_operation_run)(operation, action, ptr::null_mut(), &mut error);
        if result == 0 && error.is_null() {
            (gtk.gtk_print_operation_get_error)(operation, &mut error);
        }
        (gtk.g_object_unref)(operation);
        if !error.is_null() {
            let message = CStr::from_ptr((*error).message)
                .to_string_lossy()
                .into_owned();
            (gtk.g_error_free)(error);
            anyhow::bail!(message);
        }
        if let Some(error) = &drawing.error {
            anyhow::bail!(error.clone());
        }
        ensure!(result != 0, "The system print operation failed");
    }
    Ok(drawing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a disposable Wayland display; uses EXPORT, never a printer"]
    fn gtk_exports_print_pages_without_submitting_a_job() {
        let document = crate::document::tests::sample_document();
        let directory = tempfile::tempdir().unwrap();
        // GTK's EXPORT action always exports all pages; native-dialog range
        // selection is exercised by wayland-print.sh without submitting a job.
        for (sizing, landscape) in [(Sizing::Fit, false), (Sizing::Actual, true)] {
            let output = directory.path().join("printed.pdf");
            let filename = CString::new(output.to_str().unwrap()).unwrap();
            // SAFETY: Test-only GTK setters from the same loaded library.
            let drawing = run(&document, sizing, 3, |gtk, operation| unsafe {
                let library = &gtk._library;
                let export = library
                    .get::<unsafe extern "C" fn(Pointer, *const c_char)>(
                        b"gtk_print_operation_set_export_filename\0",
                    )
                    .unwrap();
                export(operation, filename.as_ptr());
                if landscape {
                    let new_setup = library
                        .get::<unsafe extern "C" fn() -> Pointer>(b"gtk_page_setup_new\0")
                        .unwrap();
                    let setup = new_setup();
                    let orientation = library
                        .get::<unsafe extern "C" fn(Pointer, c_int)>(
                            b"gtk_page_setup_set_orientation\0",
                        )
                        .unwrap();
                    orientation(setup, 1); // GTK_PAGE_ORIENTATION_LANDSCAPE
                    let apply = library
                        .get::<unsafe extern "C" fn(Pointer, Pointer)>(
                            b"gtk_print_operation_set_default_page_setup\0",
                        )
                        .unwrap();
                    apply(operation, setup);
                    (gtk.g_object_unref)(setup);
                }
            })
            .unwrap();
            assert_eq!(drawing.pages, [0, 1]);
            let copy = mupdf::Document::open(output.as_path()).unwrap();
            assert_eq!(copy.page_count().unwrap(), 2);
            let bounds = copy.load_page(0).unwrap().bounds().unwrap();
            assert_eq!(bounds.width() > bounds.height(), landscape);
            let image = copy
                .load_page(0)
                .unwrap()
                .to_pixmap(
                    &mupdf::Matrix::IDENTITY,
                    &mupdf::Colorspace::device_rgb(),
                    false,
                    true,
                )
                .unwrap();
            assert!(image.samples().iter().any(|v| *v < 128));
        }
    }
}
