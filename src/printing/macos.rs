use anyhow::{Context, Result};
use objc2::AllocAnyThread;
use objc2_app_kit::{NSPrintInfo, NSPrintPanelOptions};
use objc2_foundation::{MainThreadMarker, NSData, NSString};
use objc2_pdf_kit::{PDFDocument, PDFPrintScalingMode};

use super::Sizing;
use crate::document::PdfDocument;

pub(super) fn print(document: &PdfDocument, sizing: Sizing) -> Result<()> {
    let mtm = MainThreadMarker::new().context("Printing must run on the main thread")?;
    let data = NSData::with_bytes(&document.print_pdf()?);
    // SAFETY: NSData owns the PDF bytes; PDFKit retains the data it needs.
    let pdf = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
        .context("PDFKit could not read the print document")?;
    let info = NSPrintInfo::sharedPrintInfo();
    info.setScalingFactor(1.0);
    let scaling = match sizing {
        Sizing::Fit => PDFPrintScalingMode::PageScaleToFit,
        Sizing::Actual => PDFPrintScalingMode::PageScaleNone,
    };
    // SAFETY: PDFKit supplies its own print view and NSPrintOperation; disabling
    // auto-rotate respects the orientation chosen in the native print panel.
    let operation = unsafe {
        pdf.printOperationForPrintInfo_scalingMode_autoRotate(Some(&info), scaling, false, mtm)
    }
    .context("PDFKit could not create a print operation")?;
    operation.setJobTitle(Some(&NSString::from_str(&document.name())));
    operation.setShowsPrintPanel(true);
    operation.setShowsProgressPanel(true);
    let panel = operation.printPanel();
    panel.setOptions(
        panel.options()
            | NSPrintPanelOptions::ShowsPageRange
            | NSPrintPanelOptions::ShowsPaperSize
            | NSPrintPanelOptions::ShowsOrientation
            | NSPrintPanelOptions::ShowsCopies
            | NSPrintPanelOptions::ShowsPreview,
    );
    // AppKit owns range selection, printer-supported duplex and submission.
    // false also denotes user cancellation; don't report cancellation as error.
    operation.runOperation();
    Ok(())
}
