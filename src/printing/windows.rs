use std::{mem::size_of, ptr};

use anyhow::{Context, Result, ensure};
use windows_sys::Win32::{
    Foundation::{GlobalFree, RPC_E_CHANGED_MODE},
    Graphics::Gdi::*,
    Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage},
    System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
    UI::Controls::Dialogs::*,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{Sizing, raster_page};
use crate::document::PdfDocument;

struct PrintResources(PRINTDLGEXW);

struct Com(bool);

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: Balance this thread's successful CoInitializeEx call.
            unsafe { CoUninitialize() };
        }
    }
}

impl Drop for PrintResources {
    fn drop(&mut self) {
        // SAFETY: PrintDlgEx allocates these resources for the caller. Only
        // release non-null handles, on every return path including cancellation.
        unsafe {
            if !self.0.hDC.is_null() {
                DeleteDC(self.0.hDC);
            }
            if !self.0.hDevMode.is_null() {
                GlobalFree(self.0.hDevMode);
            }
            if !self.0.hDevNames.is_null() {
                GlobalFree(self.0.hDevNames);
            }
        }
    }
}

pub(super) fn print(
    document: &PdfDocument,
    sizing: Sizing,
    window: &winit::window::Window,
) -> Result<()> {
    let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
        anyhow::bail!("No native Windows print-dialog owner");
    };
    // SAFETY: Initialize COM on the calling GUI thread; an existing apartment
    // can be used without changing or uninitializing its owner’s COM state.
    let status = unsafe { CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    ensure!(
        status >= 0 || status == RPC_E_CHANGED_MODE,
        "Could not initialize the print dialog"
    );
    let _com = Com(status >= 0);
    let mut ranges = [PRINTPAGERANGE {
        nFromPage: 1,
        nToPage: document.page_count() as u32,
    }; 32];
    let mut resources = PrintResources(PRINTDLGEXW {
        lStructSize: size_of::<PRINTDLGEXW>() as u32,
        hwndOwner: handle.hwnd.get() as _,
        Flags: PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE | PD_NOSELECTION | PD_NOCURRENTPAGE,
        nMinPage: 1,
        nMaxPage: document.page_count() as u32,
        nMaxPageRanges: ranges.len() as u32,
        lpPageRanges: ranges.as_mut_ptr(),
        nCopies: 1,
        nStartPage: START_PAGE_GENERAL,
        ..Default::default()
    });
    // SAFETY: The dialog and its range buffer live until the synchronous call
    // returns. Windows builds the printer DC from the selected DEVMODE, which
    // includes orientation, paper, driver-supported duplex and copies/collation.
    unsafe {
        let status = PrintDlgExW(&mut resources.0);
        ensure!(
            status >= 0,
            "The Windows print dialog failed (HRESULT {status:#x})"
        );
        if resources.0.dwResultAction != PD_RESULT_PRINT {
            return Ok(());
        }
        let pages = selected_pages(
            document.page_count(),
            resources.0.Flags & PD_PAGENUMS != 0,
            &ranges[..resources.0.nPageRanges as usize],
        )?;
        let dc = resources.0.hDC;
        ensure!(!dc.is_null(), "The printer returned no drawing context");
        let title: Vec<u16> = document.name().encode_utf16().chain([0]).collect();
        let info = DOCINFOW {
            cbSize: size_of::<DOCINFOW>() as i32,
            lpszDocName: title.as_ptr(),
            ..Default::default()
        };
        ensure!(StartDocW(dc, &info) > 0, "Could not start the print job");
        let result = draw_pages(document, sizing, dc, &pages);
        if result.is_err() {
            AbortDoc(dc);
        } else if EndDoc(dc) <= 0 {
            AbortDoc(dc);
            anyhow::bail!("Could not finish the print job");
        }
        result
    }
}

fn selected_pages(count: usize, use_ranges: bool, ranges: &[PRINTPAGERANGE]) -> Result<Vec<usize>> {
    if !use_ranges {
        return Ok((0..count).collect());
    }
    ensure!(!ranges.is_empty(), "Choose at least one page range");
    let mut pages = Vec::new();
    for range in ranges {
        ensure!(
            range.nFromPage > 0
                && range.nFromPage <= range.nToPage
                && range.nToPage as usize <= count,
            "The print page range is outside the document"
        );
        pages.extend(range.nFromPage as usize - 1..range.nToPage as usize);
    }
    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
}

unsafe fn draw_pages(
    document: &PdfDocument,
    sizing: Sizing,
    dc: HDC,
    pages: &[usize],
) -> Result<()> {
    // SAFETY: dc is the live printer context returned by PrintDlgEx. Raster
    // buffers and BITMAPINFO remain valid until each synchronous GDI call ends.
    unsafe {
        let dpi = (
            f64::from(GetDeviceCaps(dc, LOGPIXELSX as i32)),
            f64::from(GetDeviceCaps(dc, LOGPIXELSY as i32)),
        );
        ensure!(dpi.0 > 0.0 && dpi.1 > 0.0, "Invalid printer resolution");
        let area = (
            f64::from(GetDeviceCaps(dc, HORZRES as i32)) * 72.0 / dpi.0,
            f64::from(GetDeviceCaps(dc, VERTRES as i32)) * 72.0 / dpi.1,
        );
        for &number in pages {
            let image = raster_page(document, number, area, sizing)
                .with_context(|| format!("Failed to print page {}", number + 1))?;
            ensure!(StartPage(dc) > 0, "Could not start print page");
            SetStretchBltMode(dc, HALFTONE);
            SetBrushOrgEx(dc, 0, 0, ptr::null_mut());
            let bitmap = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: image.width,
                    biHeight: -image.height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let pixels = |points: f64, dpi: f64| (points * dpi / 72.0).round() as i32;
            let rows = StretchDIBits(
                dc,
                pixels(image.placement.x, dpi.0),
                pixels(image.placement.y, dpi.1),
                pixels(image.placement.width, dpi.0),
                pixels(image.placement.height, dpi.1),
                0,
                0,
                image.width,
                image.height,
                image.bgra.as_ptr().cast(),
                &bitmap,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            ensure!(
                rows != 0 && rows != -1,
                "The printer could not draw the page"
            );
            ensure!(EndPage(dc) > 0, "Could not finish print page");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_one_based_inclusive_and_do_not_duplicate_pages() {
        let ranges = [
            PRINTPAGERANGE {
                nFromPage: 4,
                nToPage: 7,
            },
            PRINTPAGERANGE {
                nFromPage: 2,
                nToPage: 4,
            },
        ];
        assert_eq!(
            selected_pages(8, true, &ranges).unwrap(),
            [1, 2, 3, 4, 5, 6]
        );
        assert_eq!(selected_pages(3, false, &[]).unwrap(), [0, 1, 2]);
        for (from, to) in [(0, 1), (4, 3), (1, 9)] {
            assert!(
                selected_pages(
                    8,
                    true,
                    &[PRINTPAGERANGE {
                        nFromPage: from,
                        nToPage: to
                    }]
                )
                .is_err()
            );
        }
        assert!(selected_pages(8, true, &[]).is_err());
    }
}
