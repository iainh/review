use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Condvar, Mutex},
};

use crate::document::{PageImage, PdfDocument, WorkerSource};

const CACHE_BYTES: usize = 128 * 1024 * 1024;
const CACHE_ENTRIES: usize = 12;
const TILE_ENTRIES: usize = 128;
const TILE_SIDE: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderKey {
    pub page: usize,
    scale: u32,
    region: Option<[u32; 4]>,
}

impl RenderKey {
    pub fn new(page: usize, scale: f32) -> Self {
        Self {
            page,
            scale: scale.to_bits(),
            region: None,
        }
    }

    pub fn scale(self) -> f32 {
        f32::from_bits(self.scale)
    }
}

pub struct PageTile {
    pub key: RenderKey,
    /// Original normalized page coordinates, excluding the overlap pixels.
    pub bounds: egui::Rect,
    pub uv: egui::Rect,
}

/// Plan only the visible region; page dimensions never determine allocation
/// or iteration count. Overlap pixels keep linear filtering continuous at
/// shared edges. The last pixel is clipped to the exact native page size.
pub fn visible_tiles(
    key: RenderKey,
    size: (f32, f32),
    visible: egui::Rect,
    max_side: usize,
) -> anyhow::Result<Vec<PageTile>> {
    let width = f64::from(size.0) * f64::from(key.scale());
    let height = f64::from(size.1) * f64::from(key.scale());
    anyhow::ensure!(
        width.is_finite()
            && height.is_finite()
            && width > 0.0
            && height > 0.0
            && width.ceil() < f64::from(i32::MAX)
            && height.ceil() < f64::from(i32::MAX),
        "page has invalid raster bounds"
    );
    anyhow::ensure!(max_side >= 3, "graphics texture limit is too small");
    let side = TILE_SIDE.min((max_side - 2).min(u32::MAX as usize) as u32);
    let unit = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    let visible = visible.intersect(unit);
    if !visible.is_positive() {
        return Ok(Vec::new());
    }
    if width.ceil() <= f64::from(side) && height.ceil() <= f64::from(side) {
        return Ok(vec![PageTile {
            key,
            bounds: unit,
            uv: unit,
        }]);
    }
    let columns = (width / f64::from(side)).ceil() as u32;
    let rows = (height / f64::from(side)).ceil() as u32;
    let first_x = (f64::from(visible.min.x) * width / f64::from(side)).floor() as u32;
    let first_y = (f64::from(visible.min.y) * height / f64::from(side)).floor() as u32;
    let last_x = ((f64::from(visible.max.x) * width / f64::from(side)).ceil() as u32).min(columns);
    let last_y = ((f64::from(visible.max.y) * height / f64::from(side)).ceil() as u32).min(rows);
    let mut tiles = Vec::new();
    for y in first_y..last_y {
        for x in first_x..last_x {
            let x0 = x * side;
            let y0 = y * side;
            let x1 = (x0 + side).min(width.ceil() as u32);
            let y1 = (y0 + side).min(height.ceil() as u32);
            let region = [
                x0.saturating_sub(1),
                y0.saturating_sub(1),
                (x1 + 1).min(width.ceil() as u32),
                (y1 + 1).min(height.ceil() as u32),
            ];
            let right = f64::from(x1).min(width);
            let bottom = f64::from(y1).min(height);
            tiles.push(PageTile {
                key: RenderKey {
                    region: Some(region),
                    ..key
                },
                bounds: egui::Rect::from_min_max(
                    egui::pos2(
                        (f64::from(x0) / width) as f32,
                        (f64::from(y0) / height) as f32,
                    ),
                    egui::pos2((right / width) as f32, (bottom / height) as f32),
                ),
                uv: egui::Rect::from_min_max(
                    egui::pos2(
                        (x0 - region[0]) as f32 / (region[2] - region[0]) as f32,
                        (y0 - region[1]) as f32 / (region[3] - region[1]) as f32,
                    ),
                    egui::pos2(
                        ((right - f64::from(region[0])) / f64::from(region[2] - region[0])) as f32,
                        ((bottom - f64::from(region[1])) / f64::from(region[3] - region[1])) as f32,
                    ),
                ),
            });
        }
    }
    Ok(tiles)
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
        // No single result can defeat the budget, even if a caller bypasses
        // the raster preflight. Full pages retain their original 12-entry cap;
        // smaller tiles share the byte budget with up to 128 total entries.
        if result
            .as_ref()
            .is_ok_and(|image| image.rgba.len() > CACHE_BYTES)
        {
            return;
        }
        self.clock += 1;
        self.cache.insert(
            key,
            Cached {
                result,
                used: self.clock,
            },
        );
        while self.cache.len() > TILE_ENTRIES
            || self.cache.keys().filter(|key| key.region.is_none()).count() > CACHE_ENTRIES
            || self.cache.values().map(Cached::bytes).sum::<usize>() > CACHE_BYTES
        {
            let full_pages =
                self.cache.keys().filter(|key| key.region.is_none()).count() > CACHE_ENTRIES;
            let oldest = self
                .cache
                .iter()
                .filter(|(candidate, _)| {
                    **candidate != key && (!full_pages || candidate.region.is_none())
                })
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
        let mut queued = HashSet::new();
        state.queue = self
            .frame
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| {
                state.active != Some(*key) && !state.cache.contains_key(key) && queued.insert(*key)
            })
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
    anyhow::ensure!(
        key.scale().is_finite() && key.scale() > 0.0,
        "invalid rendering scale"
    );
    if let Some([x0, y0, x1, y1]) = key.region {
        anyhow::ensure!(x0 < x1 && y0 < y1, "invalid rendering region");
        anyhow::ensure!(
            (x1 - x0) as usize <= max_side && (y1 - y0) as usize <= max_side,
            "rendering region exceeds the graphics texture limit"
        );
        anyhow::ensure!(
            u64::from(x1 - x0) * u64::from(y1 - y0) * 4 <= CACHE_BYTES as u64,
            "rendering region exceeds the memory limit"
        );
        return document.render_region(key.page, key.scale(), [x0, y0, x1, y1]);
    }
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
    fn asymmetric_visible_tiles_have_exact_cores_and_overlap_bounds() {
        let visible = egui::Rect::from_min_max(egui::pos2(0.58, 0.46), egui::pos2(0.99, 0.90));
        let tiles = visible_tiles(RenderKey::new(7, 1.0), (3500.0, 2100.0), visible, 8192).unwrap();
        assert_eq!(tiles.len(), 6);
        assert_eq!(tiles[0].key.region, Some([1023, 0, 2049, 1025]));
        assert_eq!(tiles[5].key.region, Some([3071, 1023, 3500, 2049]));
        assert_eq!(tiles[0].bounds.min, egui::pos2(1024.0 / 3500.0, 0.0));
        assert_eq!(tiles[5].bounds.max, egui::pos2(1.0, 2048.0 / 2100.0));
        assert_eq!(tiles[0].bounds.max.x, tiles[1].bounds.min.x);
        assert_eq!(tiles[0].bounds.max.y, tiles[3].bounds.min.y);
        assert_eq!(tiles[0].uv.min, egui::pos2(1.0 / 1026.0, 0.0));
        assert_eq!(tiles[5].uv.max, egui::pos2(1.0, 1025.0 / 1026.0));
        assert_ne!(tiles[0].key, tiles[1].key);
        assert_ne!(tiles[0].key, RenderKey::new(7, 1.0));
        let fractional = visible_tiles(
            RenderKey::new(0, 1.0),
            (1024.25, 10.5),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            8192,
        )
        .unwrap();
        assert_eq!(fractional[1].key.region, Some([1023, 0, 1025, 11]));
        assert_eq!(fractional[1].bounds.max, egui::pos2(1.0, 1.0));
        assert_eq!(fractional[1].uv.max, egui::pos2(0.625, 10.5 / 11.0));
    }

    #[test]
    fn huge_page_planning_is_visible_only_and_respects_texture_limit() {
        let tiles = visible_tiles(
            RenderKey::new(0, 20.0),
            (100_000.0, 60_000.0),
            egui::Rect::from_min_max(egui::pos2(0.5, 0.5), egui::pos2(0.5005, 0.5007)),
            514,
        )
        .unwrap();
        assert!(tiles.len() <= 9, "{}", tiles.len());
        for tile in tiles {
            let [x0, y0, x1, y1] = tile.key.region.unwrap();
            assert!(x1 - x0 <= 514 && y1 - y0 <= 514);
            assert!(x0 > 900_000 && y0 > 500_000);
        }
        let outside = egui::Rect::from_min_max(egui::pos2(1.1, 0.1), egui::pos2(1.2, 0.2));
        assert!(
            visible_tiles(RenderKey::new(0, 1.0), (100.0, 100.0), outside, 8192)
                .unwrap()
                .is_empty()
        );
        for scale in [0.0, -1.0, f32::INFINITY, f32::NAN] {
            assert!(
                visible_tiles(RenderKey::new(0, scale), (100.0, 100.0), outside, 8192).is_err()
            );
        }
    }

    #[test]
    fn tiled_pixels_and_overlaps_match_whole_page_at_fractional_scale() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("seams.pdf");
        // Offset MediaBox, orthogonal strokes, text and an asymmetric fill
        // cross tile boundaries. MuPDF clips before scan conversion, so
        // diagonal paths can legitimately differ slightly between rasters;
        // this fixture has an independent exact cropped-pixel oracle.
        std::fs::write(&path, crate::document::tests::sample_pdf(
            "0.1 0.7 0.3 rg 35 87 219 246 re f 0 0 0 RG 2.7 w 10 123 m 310 123 l 310 381 l 10 381 l S BT /F1 24 Tf 44 250 Td (Asymmetric seams) Tj ET", false)).unwrap();
        let document = PdfDocument::open(path).unwrap();
        let key = RenderKey::new(0, 3.125);
        let full = document.render_at_scale(0, key.scale()).unwrap();
        let unit = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        let tiles = visible_tiles(key, (300.0, 400.0), unit, 258).unwrap();
        assert_eq!(tiles.len(), 20);
        for tile in tiles {
            let [x0, y0, x1, y1] = tile.key.region.unwrap();
            let image = render(&document, tile.key, 258).unwrap();
            assert_eq!((image.width, image.height), (x1 - x0, y1 - y0));
            for y in y0..y1 {
                let full_start = ((y * full.width + x0) * 4) as usize;
                let start = ((y - y0) * image.width * 4) as usize;
                let length = (image.width * 4) as usize;
                assert!(
                    &image.rgba[start..start + length]
                        == &full.rgba[full_start..full_start + length],
                    "tile {:?}, row {y}",
                    tile.key.region
                );
            }
        }
    }

    #[test]
    fn tile_cache_limits_and_stale_pan_zoom_results() {
        let mut state = State::default();
        let key = |x| RenderKey {
            region: Some([x, 0, x + 10, 10]),
            ..RenderKey::new(0, 2.0)
        };
        for x in 0..TILE_ENTRIES as u32 + 1 {
            state.insert(key(x), Err("tile".into()));
        }
        assert_eq!(state.cache.len(), TILE_ENTRIES);
        assert!(!state.cache.contains_key(&key(0)));
        let wanted = key(1000);
        state.wanted.insert(wanted);
        state.finish(key(1001), Err("old pan".into()));
        state.finish(
            RenderKey {
                scale: 1.0_f32.to_bits(),
                ..wanted
            },
            Err("old zoom".into()),
        );
        assert!(!state.cache.contains_key(&key(1001)));
        assert!(!state.cache.contains_key(&RenderKey {
            scale: 1.0_f32.to_bits(),
            ..wanted
        }));
        state.finish(wanted, Err("current".into()));
        assert!(state.cache.contains_key(&wanted));
    }

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
