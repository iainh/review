//! Background file reading and startup metadata. MuPDF objects never cross threads.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

use anyhow::{Context, Result};
use winit::event_loop::EventLoopProxy;
use zeroize::Zeroizing;

use crate::{AppEvent, document::PdfDocument, persistence::ReadingState};

pub struct Metadata {
    pub key: PathBuf,
    pub sizes: Vec<(f32, f32)>,
    pub outline: Result<Vec<mupdf::Outline>, String>,
}

// Deliberately not Debug: credentials remain session-only, including in transit.
pub struct LoadedDocument {
    path: PathBuf,
    bytes: Arc<Vec<u8>>,
    password: Option<Zeroizing<String>>,
    metadata: Metadata,
}

impl LoadedDocument {
    /// The editable document must still be created on the UI thread. Reuse the
    /// bytes and metadata rather than reading the file or scanning its pages again.
    pub fn open(self) -> Result<(PdfDocument, Metadata)> {
        let document = PdfDocument::from_bytes(
            &self.path,
            self.bytes,
            self.password.as_deref().map(String::as_str),
        )?
        .context("PDF could not be authenticated; reopen the document")?;
        Ok((document, self.metadata))
    }
}

pub struct Load {
    pub path: PathBuf,
    pub reading: Option<ReadingState>,
    pub password_attempt: bool,
    pub receiver: Receiver<(PathBuf, Result<Option<LoadedDocument>, String>)>,
    cancelled: Arc<AtomicBool>,
}

impl Load {
    pub fn start(
        path: PathBuf,
        reading: Option<ReadingState>,
        password: Option<Zeroizing<String>>,
        proxy: Option<EventLoopProxy<AppEvent>>,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let worker_path = path.clone();
        let password_attempt = password.is_some();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let key = crate::persistence::file_key(&worker_path);
            let result = prepare(worker_path, key.clone(), password, &worker_cancelled)
                .map_err(|error| format!("{error:#}"));
            if !worker_cancelled.load(Ordering::Relaxed)
                && sender.send((key, result)).is_ok()
                && let Some(proxy) = proxy
            {
                let _ = proxy.send_event(AppEvent::Repaint);
            }
        });
        Self {
            path,
            reading,
            password_attempt,
            receiver,
            cancelled,
        }
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        // Never join a file read or MuPDF operation from the event-loop thread.
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

fn prepare(
    path: PathBuf,
    key: PathBuf,
    password: Option<Zeroizing<String>>,
    cancelled: &AtomicBool,
) -> Result<Option<LoadedDocument>> {
    let bytes = Arc::new(
        std::fs::read(&path)
            .with_context(|| format!("failed to open PDF at {}", path.display()))?,
    );
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let Some(document) = PdfDocument::from_bytes(
        &path,
        bytes.clone(),
        password.as_deref().map(String::as_str),
    )?
    else {
        return Ok(None);
    };
    let outline = document
        .outlines()
        .map_err(|error| format!("Could not load outline: {error:#}"));
    let mut sizes = Vec::with_capacity(document.page_count());
    for page in 0..document.page_count() {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        sizes.push(document.page_size(page)?);
    }
    Ok(Some(LoadedDocument {
        path,
        bytes,
        password,
        metadata: Metadata {
            key,
            sizes,
            outline,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_snapshot_keeps_bytes_outline_and_asymmetric_geometry_after_file_removal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.pdf");
        std::fs::write(
            &path,
            crate::document::tests::sample_pdf(
                "BT /F1 16 Tf 40 350 Td (Original snapshot) Tj ET",
                true,
            ),
        )
        .unwrap();
        let load = Load::start(path.clone(), None, None, None);
        let (_, result) = load
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let loaded = result.unwrap().unwrap();
        std::fs::remove_file(path).unwrap();
        let (document, metadata) = loaded.open().unwrap();
        assert_eq!(document.page_count(), 2);
        assert_eq!(document.page_text(0).unwrap(), "Original snapshot");
        assert_eq!(metadata.sizes, [(300.0, 400.0), (400.0, 200.0)]);
        let outline = metadata.outline.unwrap();
        assert_eq!(outline[0].title, "Chapter one");
        assert_eq!(outline[0].down[0].title, "Nested chapter two");
        assert_eq!(outline[0].down[0].dest.unwrap().loc.page_number, 1);
    }
}
