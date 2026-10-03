use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Condvar, Mutex},
};

use crate::document::{PageImage, PdfDocument, WorkerSource};

const CACHE_BYTES: usize = 128 * 1024 * 1024;
const CACHE_ENTRIES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderKey {
    pub page: usize,
    scale: u32,
}

impl RenderKey {
    pub fn new(page: usize, scale: f32) -> Self {
        Self {
            page,
            scale: scale.to_bits(),
        }
    }

    pub fn scale(self) -> f32 {
        f32::from_bits(self.scale)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Page,
    Thumbnail,
    Prefetch,
}

type RenderResult = Result<Arc<PageImage>, String>;

struct Cached {
    result: RenderResult,
    used: u64,
}

impl Cached {
    fn bytes(&self) -> usize {
        self.result.as_ref().map_or(0, |image| image.rgba.len())
    }
}

#[derive(Default)]
struct State {
    queue: VecDeque<RenderKey>,
    wanted: HashSet<RenderKey>,
    active: Option<RenderKey>,
    cache: HashMap<RenderKey, Cached>,
    clock: u64,
    stopped: bool,
    repaint: Option<egui::Context>,
    max_side: usize,
}

impl State {
    fn finish(&mut self, key: RenderKey, result: RenderResult) -> Option<egui::Context> {
        self.active = None;
        if !self.stopped && self.wanted.contains(&key) {
            self.insert(key, result);
            self.repaint.clone()
        } else {
            None
        }
    }

    fn insert(&mut self, key: RenderKey, result: RenderResult) {
        self.clock += 1;
        self.cache.insert(
            key,
            Cached {
                result,
                used: self.clock,
            },
        );
        while self.cache.len() > CACHE_ENTRIES
            || self.cache.values().map(Cached::bytes).sum::<usize>() > CACHE_BYTES
        {
            let oldest = self
                .cache
                .iter()
                .filter(|(candidate, _)| **candidate != key)
                .min_by_key(|(_, value)| value.used)
                .map(|(key, _)| *key);
            let Some(oldest) = oldest else { break };
            self.cache.remove(&oldest);
        }
    }
}

/// MuPDF documents and pages stay on their creating thread. Only owned pixels
/// cross this boundary. Replacing a frame cancels queued work, and stale in-flight
/// results are discarded; MuPDF's current page operation finishes normally.
pub struct RenderWorker {
    shared: Arc<(Mutex<State>, Condvar)>,
    frame: Vec<(RenderKey, Priority)>,
}

impl RenderWorker {
    pub fn new(source: WorkerSource) -> Self {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = shared.clone();
        std::thread::spawn(move || {
            let document = source.open().map_err(|error| format!("{error:#}"));
            loop {
                let (key, max_side) = {
                    let (lock, wake) = &*worker;
                    let mut state = lock.lock().unwrap();
                    while !state.stopped && state.queue.is_empty() {
                        state = wake.wait(state).unwrap();
                    }
                    if state.stopped {
                        return;
                    }
                    let key = state.queue.pop_front().unwrap();
                    if state.cache.contains_key(&key) || !state.wanted.contains(&key) {
                        continue;
                    }
                    state.active = Some(key);
                    (key, state.max_side)
                };
                let result = match &document {
                    Ok(document) => render(document, key, max_side)
                        .map(Arc::new)
                        .map_err(|error| format!("{error:#}")),
                    Err(error) => Err(error.clone()),
                };
                let repaint = {
                    let mut state = worker.0.lock().unwrap();
                    if state.stopped {
                        return;
                    }
                    state.finish(key, result)
                };
                if let Some(ctx) = repaint {
                    ctx.request_repaint()
                }
            }
        });
        Self {
            shared,
            frame: Vec::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame.clear();
    }

    pub fn image(&mut self, key: RenderKey, priority: Priority) -> Option<RenderResult> {
        self.frame.push((key, priority));
        let mut state = self.shared.0.lock().unwrap();
        state.clock += 1;
        let clock = state.clock;
        state.cache.get_mut(&key).map(|cached| {
            cached.used = clock;
            cached.result.clone()
        })
    }

    pub fn end_frame(&mut self, ctx: &egui::Context) {
        self.frame.sort_by_key(|(_, priority)| *priority);
        let max_side = ctx.input(|input| input.max_texture_side);
        let mut state = self.shared.0.lock().unwrap();
        state.repaint = Some(ctx.clone());
        state.max_side = max_side;
        state.wanted = self.frame.iter().map(|(key, _)| *key).collect();
        state.queue = self
            .frame
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| state.active != Some(*key) && !state.cache.contains_key(key))
            .collect();
        self.shared.1.notify_one();
    }
}

impl Drop for RenderWorker {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap().stopped = true;
        self.shared.1.notify_one();
    }
}

fn render(document: &PdfDocument, key: RenderKey, max_side: usize) -> anyhow::Result<PageImage> {
    let size = document.page_size(key.page)?;
    let width = (size.0 * key.scale()).ceil();
    let height = (size.1 * key.scale()).ceil();
    anyhow::ensure!(
        width <= max_side as f32 && height <= max_side as f32,
        "This zoom exceeds the graphics texture limit; reduce the zoom"
    );
    anyhow::ensure!(
        width * height * 4.0 <= CACHE_BYTES as f32,
        "This zoom exceeds the page rendering memory limit; reduce the zoom"
    );
    let image = document.render_at_scale(key.page, key.scale())?;
    // MuPDF rounds the transformed bounding box to integer pixel edges.
    anyhow::ensure!(
        image.width as usize <= max_side && image.height as usize <= max_side,
        "This zoom exceeds the graphics texture limit; reduce the zoom"
    );
    anyhow::ensure!(
        image.rgba.len() <= CACHE_BYTES,
        "This zoom exceeds the page rendering memory limit; reduce the zoom"
    );
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_opens_and_rasterizes_on_its_thread_and_reuses_owned_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.pdf");
        std::fs::write(&path, crate::document::tests::sample_pdf("", false)).unwrap();
        let document = PdfDocument::open(path).unwrap();
        let mut worker = RenderWorker::new(document.worker_source());
        let key = RenderKey::new(1, 1.5);
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let image = loop {
            worker.begin_frame();
            let image = worker.image(key, Priority::Page);
            worker.end_frame(&ctx);
            if let Some(image) = image {
                break image.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!((image.width, image.height), (600, 300));
        assert_eq!(image.rgba.len(), 600 * 300 * 4);
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] < 128)
        );
        let cached = worker.image(key, Priority::Page).unwrap().unwrap();
        assert!(Arc::ptr_eq(&image, &cached));
    }

    #[test]
    fn stale_and_stopped_results_cannot_enter_the_cache() {
        let stale = RenderKey::new(0, 1.0);
        let wanted = RenderKey::new(1, 2.0);
        let mut state = State::default();
        state.wanted.insert(wanted);
        state.active = Some(stale);
        state.finish(stale, Err("stale".into()));
        assert!(state.cache.is_empty());
        assert_eq!(state.active, None);
        state.finish(wanted, Err("wanted".into()));
        assert!(state.cache.contains_key(&wanted));
        state.cache.clear();
        state.stopped = true;
        state.finish(wanted, Err("after stop".into()));
        assert!(state.cache.is_empty());
    }

    #[test]
    fn rendering_rejects_texture_and_memory_limits_before_allocation() {
        let document = crate::document::tests::sample_document();
        assert!(render(&document, RenderKey::new(0, 2.0), 799).is_err());
        assert_eq!(
            render(&document, RenderKey::new(0, 2.0), 800)
                .unwrap()
                .height,
            800
        );
        assert!(
            render(&document, RenderKey::new(0, 17.0), 16_384)
                .err()
                .unwrap()
                .to_string()
                .contains("memory limit")
        );
    }

    #[test]
    fn cache_budget_counts_pixel_bytes_not_just_page_count() {
        let mut state = State::default();
        for page in 0..9 {
            state.insert(
                RenderKey::new(page, 1.0),
                Ok(Arc::new(PageImage {
                    width: 2048,
                    height: 2048,
                    rgba: vec![255; 2048 * 2048 * 4],
                })),
            );
        }
        assert_eq!(state.cache.len(), 8);
        assert_eq!(
            state.cache.values().map(Cached::bytes).sum::<usize>(),
            128 * 1024 * 1024
        );
        assert!(!state.cache.contains_key(&RenderKey::new(0, 1.0)));
        assert!(state.cache.contains_key(&RenderKey::new(8, 1.0)));
    }

    #[test]
    fn cache_evicts_least_recently_used_entries_and_preserves_new_result() {
        let mut state = State::default();
        for page in 0..CACHE_ENTRIES {
            state.insert(RenderKey::new(page, 1.0), Err(format!("page {page}")));
        }
        state.cache.get_mut(&RenderKey::new(0, 1.0)).unwrap().used = 100;
        state.insert(RenderKey::new(CACHE_ENTRIES, 1.0), Err("new".into()));
        assert_eq!(state.cache.len(), CACHE_ENTRIES);
        assert!(state.cache.contains_key(&RenderKey::new(0, 1.0)));
        assert!(!state.cache.contains_key(&RenderKey::new(1, 1.0)));
        assert!(
            state
                .cache
                .contains_key(&RenderKey::new(CACHE_ENTRIES, 1.0))
        );
    }

    #[test]
    fn frame_replacement_drops_obsolete_requests_and_prioritizes_visible_page() {
        let mut worker = RenderWorker {
            shared: Arc::new((Mutex::new(State::default()), Condvar::new())),
            frame: Vec::new(),
        };
        let ctx = egui::Context::default();
        worker.image(RenderKey::new(0, 1.0), Priority::Page);
        worker.end_frame(&ctx);
        worker.begin_frame();
        worker.image(RenderKey::new(8, 0.2), Priority::Thumbnail);
        worker.image(RenderKey::new(7, 1.5), Priority::Page);
        worker.image(RenderKey::new(6, 1.5), Priority::Prefetch);
        worker.end_frame(&ctx);
        let state = worker.shared.0.lock().unwrap();
        assert!(!state.wanted.contains(&RenderKey::new(0, 1.0)));
        assert_eq!(
            state.queue.iter().copied().collect::<Vec<_>>(),
            [
                RenderKey::new(7, 1.5),
                RenderKey::new(8, 0.2),
                RenderKey::new(6, 1.5),
            ]
        );
    }
}
