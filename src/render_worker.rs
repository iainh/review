use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Condvar, Mutex, mpsc},
};

use crate::document::{PageImage, PdfDocument, RecordedPage, WorkerSource};

const CACHE_BYTES: usize = 128 * 1024 * 1024;
const CACHE_ENTRIES: usize = 12;
const TILE_ENTRIES: usize = 128;
const RASTER_WORKERS: usize = 2;
// Separate from retained cache pixels: includes concurrent RGB pixmaps (with
// guard pixels) AND their RGBA conversion buffers, not just completed images.
const IN_FLIGHT_BYTES: u64 = 256 * 1024 * 1024;
pub const TILE_SIDE: u32 = 1024;

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

    pub fn image_size(self, size: (f32, f32)) -> [usize; 2] {
        match self.region {
            Some([x0, y0, x1, y1]) => [(x1 - x0) as usize, (y1 - y0) as usize],
            None => [
                (f64::from(size.0) * f64::from(self.scale())).ceil() as usize,
                (f64::from(size.1) * f64::from(self.scale())).ceil() as usize,
            ],
        }
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
        self.result.as_ref().map_or(0, |image| image.rgba().len())
    }
}

#[derive(Default)]
struct State {
    queue: VecDeque<RenderKey>,
    wanted: HashSet<RenderKey>,
    active: HashSet<RenderKey>,
    source: Option<WorkerSource>,
    generation: u64,
    cache: HashMap<RenderKey, Cached>,
    clock: u64,
    stopped: bool,
    repaint: Option<egui::Context>,
    max_side: usize,
}

impl State {
    fn finish(
        &mut self,
        generation: u64,
        key: RenderKey,
        result: RenderResult,
    ) -> Option<egui::Context> {
        if generation != self.generation {
            return None;
        }
        self.active.remove(&key);
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
            .is_ok_and(|image| image.rgba().len() > CACHE_BYTES)
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

/// One owner opens documents and records pages; two persistent raster threads
/// replay completed lists into private pixmaps. Only one page list is retained,
/// shared by a bounded wave of jobs, and released when idle or changing pages.
/// Frame replacement cancels queued jobs. Source replacement also invalidates
/// in-flight results without spawning another pool or moving native documents.
pub struct RenderWorker {
    shared: Arc<(Mutex<State>, Condvar)>,
    frame: Vec<(RenderKey, Priority)>,
}

impl RenderWorker {
    pub fn new(source: WorkerSource) -> Self {
        let shared = Arc::new((
            Mutex::new(State {
                source: Some(source),
                ..State::default()
            }),
            Condvar::new(),
        ));
        let worker = shared.clone();
        std::thread::spawn(move || run(worker));
        Self {
            shared,
            frame: Vec::new(),
        }
    }

    pub fn replace_source(&mut self, source: WorkerSource) {
        let mut state = self.shared.0.lock().unwrap();
        state.generation += 1;
        state.source = Some(source);
        state.queue.clear();
        state.wanted.clear();
        state.active.clear();
        state.cache.clear();
        self.frame.clear();
        self.shared.1.notify_one();
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
                !state.active.contains(key) && !state.cache.contains_key(key) && queued.insert(*key)
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

struct TileJob {
    key: RenderKey,
    generation: u64,
    page: Arc<RecordedPage>,
}

fn complete(
    shared: &(Mutex<State>, Condvar),
    generation: u64,
    key: RenderKey,
    result: RenderResult,
) {
    let repaint = shared.0.lock().unwrap().finish(generation, key, result);
    if let Some(ctx) = repaint {
        ctx.request_repaint();
    }
}

fn run(shared: Arc<(Mutex<State>, Condvar)>) {
    // No raster backlog: the owner submits at most two jobs, then waits for
    // both before recording another page. A list cannot accumulate per tile,
    // page, scale, or revision. Native resources referenced by a single list
    // have document-dependent size; there is deliberately no list cache.
    let (send, receive) = mpsc::sync_channel::<TileJob>(RASTER_WORKERS);
    let receive = Mutex::new(receive);
    let (done, completed) = mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..RASTER_WORKERS {
            let (receive, shared, done) = (&receive, &shared, &done);
            scope.spawn(move || {
                loop {
                    let Ok(job) = receive.lock().unwrap().recv() else {
                        break;
                    };
                    let wanted = {
                        let state = shared.0.lock().unwrap();
                        !state.stopped
                            && state.generation == job.generation
                            && state.wanted.contains(&job.key)
                    };
                    if wanted {
                        let result = job
                            .page
                            .render_region(job.key.scale(), job.key.region.unwrap())
                            .map(Arc::new)
                            .map_err(|error| format!("{error:#}"));
                        complete(shared, job.generation, job.key, result);
                    } else {
                        let mut state = shared.0.lock().unwrap();
                        if state.generation == job.generation {
                            state.active.remove(&job.key);
                        }
                    }
                    drop(job);
                    done.send(()).unwrap();
                }
            });
        }
        let mut document = None;
        let mut recording = None;
        loop {
            let (keys, generation, max_side, source) = {
                let mut state = shared.0.lock().unwrap();
                while !state.stopped && state.queue.is_empty() && state.source.is_none() {
                    recording = None;
                    state = shared.1.wait(state).unwrap();
                }
                if state.stopped {
                    break;
                }
                let source = state.source.take();
                let generation = state.generation;
                let max_side = state.max_side;
                let mut keys = Vec::new();
                let mut bytes = 0_u64;
                while let Some(&key) = state.queue.front() {
                    if keys.len() == RASTER_WORKERS
                        || keys.first().is_some_and(|first: &RenderKey| {
                            first.page != key.page || first.region.is_none() || key.region.is_none()
                        })
                    {
                        break;
                    }
                    // A tile holds RGB guard pixels while constructing RGBA.
                    // Invalid requests are reported by the normal preflight.
                    let reservation = key.region.map_or(0, |[x0, y0, x1, y1]| {
                        let w = u64::from(x1.saturating_sub(x0));
                        let h = u64::from(y1.saturating_sub(y0));
                        (w + 4)
                            .saturating_mul(h + 4)
                            .saturating_mul(3)
                            .saturating_add(w.saturating_mul(h).saturating_mul(4))
                    });
                    if !keys.is_empty() && bytes.saturating_add(reservation) > IN_FLIGHT_BYTES {
                        break;
                    }
                    state.queue.pop_front();
                    if state.cache.contains_key(&key) || !state.wanted.contains(&key) {
                        continue;
                    }
                    if key.region.is_some() {
                        let validation = validate_region(key, max_side).and_then(|()| {
                            anyhow::ensure!(
                                reservation <= IN_FLIGHT_BYTES,
                                "rendering region exceeds the in-flight memory limit"
                            );
                            Ok(())
                        });
                        if let Err(error) = validation {
                            if let Some(ctx) =
                                state.finish(generation, key, Err(format!("{error:#}")))
                            {
                                ctx.request_repaint();
                            }
                            continue;
                        }
                    }
                    bytes += reservation;
                    state.active.insert(key);
                    keys.push(key);
                }
                (keys, generation, max_side, source)
            };
            if let Some(source) = source {
                recording = None;
                drop(document.take());
                document = Some(source.open().map_err(|error| format!("{error:#}")));
            }
            let Some(&first) = keys.first() else { continue };
            let document = document.as_ref().unwrap();
            if first.region.is_some()
                && recording
                    .as_ref()
                    .is_none_or(|(page, _)| *page != first.page)
            {
                drop(recording.take());
                recording = Some((
                    first.page,
                    document.as_ref().map_err(Clone::clone).and_then(|doc| {
                        doc.record_page(first.page)
                            .map(Arc::new)
                            .map_err(|error| format!("{error:#}"))
                    }),
                ));
            } else if first.region.is_none() {
                recording = None;
            }
            let mut dispatched = 0;
            for key in keys {
                let state = shared.0.lock().unwrap();
                let wanted =
                    !state.stopped && state.generation == generation && state.wanted.contains(&key);
                drop(state);
                if !wanted {
                    let mut state = shared.0.lock().unwrap();
                    if state.generation == generation {
                        state.active.remove(&key);
                    }
                    continue;
                }
                let result = document
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|document| {
                        if key.region.is_some() {
                            let page = recording
                                .as_ref()
                                .unwrap()
                                .1
                                .as_ref()
                                .map_err(Clone::clone)?;
                            send.send(TileJob {
                                key,
                                generation,
                                page: page.clone(),
                            })
                            .unwrap();
                            dispatched += 1;
                            Ok(None)
                        } else {
                            render(document, key, max_side)
                                .map(Arc::new)
                                .map(Some)
                                .map_err(|error| format!("{error:#}"))
                        }
                    });
                match result {
                    Ok(None) => {}
                    Ok(Some(image)) => complete(&shared, generation, key, Ok(image)),
                    Err(error) => complete(&shared, generation, key, Err(error)),
                }
            }
            for _ in 0..dispatched {
                completed.recv().unwrap();
            }
        }
        drop(send);
    });
}

fn validate_region(key: RenderKey, max_side: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        key.scale().is_finite() && key.scale() > 0.0,
        "invalid rendering scale"
    );
    let [x0, y0, x1, y1] = key.region.unwrap();
    anyhow::ensure!(
        x0 < x1 && y0 < y1 && x1 <= i32::MAX as u32 && y1 <= i32::MAX as u32,
        "invalid rendering region"
    );
    anyhow::ensure!(
        (x1 - x0) as usize <= max_side && (y1 - y0) as usize <= max_side,
        "rendering region exceeds the graphics texture limit"
    );
    anyhow::ensure!(
        u64::from(x1 - x0) * u64::from(y1 - y0) * 4 <= CACHE_BYTES as u64,
        "rendering region exceeds the memory limit"
    );
    Ok(())
}

fn render(document: &PdfDocument, key: RenderKey, max_side: usize) -> anyhow::Result<PageImage> {
    anyhow::ensure!(
        key.scale().is_finite() && key.scale() > 0.0,
        "invalid rendering scale"
    );
    if let Some([x0, y0, x1, y1]) = key.region {
        validate_region(key, max_side)?;
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
        image.rgba().len() <= CACHE_BYTES,
        "This zoom exceeds the page rendering memory limit; reduce the zoom"
    );
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn images(worker: &mut RenderWorker, keys: &[RenderKey]) -> Vec<Arc<PageImage>> {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            worker.begin_frame();
            let images: Vec<_> = keys
                .iter()
                .map(|&key| worker.image(key, Priority::Page))
                .collect();
            worker.end_frame(&ctx);
            if let Some(images) = images.into_iter().collect::<Option<Vec<_>>>() {
                return images.into_iter().map(Result::unwrap).collect();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn parallel_tiles_follow_source_replacement_layers_annotations_and_undo() {
        use crate::annotations::{Geometry, Kind};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layers.pdf");
        std::fs::write(&path, crate::inspection::tests::fixture()).unwrap();
        let mut document = PdfDocument::open(path).unwrap();
        let keys = [
            [183, 531, 423, 657],
            [0, 0, 201, 301],
            [399, 499, 599, 799],
            [39, 699, 155, 799],
        ]
        .map(|region| RenderKey {
            region: Some(region),
            ..RenderKey::new(0, 2.0)
        });
        let mut worker = RenderWorker::new(document.worker_source());
        let check = |worker: &mut RenderWorker, document: &PdfDocument| {
            let images = images(worker, &keys);
            for (image, key) in images.iter().zip(keys) {
                assert_eq!(
                    image.rgba(),
                    document
                        .render_region(0, key.scale(), key.region.unwrap())
                        .unwrap()
                        .rgba()
                );
            }
            images
        };
        let original = check(&mut worker, &document);
        let mut layers = crate::inspection::Inspection::read(document.pdf())
            .layers
            .unwrap();
        layers[0].enabled = false;
        document.set_layer_visibility(&layers).unwrap();
        worker.replace_source(document.worker_source());
        let hidden = check(&mut worker, &document);
        assert_ne!(original[0].pixels, hidden[0].pixels);
        document
            .add_annotation(
                0,
                Kind::Ink,
                Geometry::Ink(vec![vec![[0.1, 0.1], [0.3, 0.25]]]),
                "parallel ink",
                [1.0, 0.0, 0.0],
                3.0,
            )
            .unwrap();
        worker.replace_source(document.worker_source());
        let annotated = check(&mut worker, &document);
        assert_ne!(hidden[1].pixels, annotated[1].pixels);
        document.undo().unwrap();
        // Replace queued requests repeatedly, including identical render keys.
        // Old revisions must not satisfy the final frame or retain a pool.
        for _ in 0..20 {
            worker.replace_source(document.worker_source());
            worker.begin_frame();
            for key in keys {
                worker.image(key, Priority::Page);
            }
            worker.end_frame(&egui::Context::default());
        }
        let undone = check(&mut worker, &document);
        for (image, expected) in undone.iter().zip(&hidden) {
            assert_eq!(image.pixels, expected.pixels);
        }
        layers[0].enabled = true;
        document.set_layer_visibility(&layers).unwrap();
        worker.replace_source(document.worker_source());
        let restored = check(&mut worker, &document);
        for (image, expected) in restored.iter().zip(&original) {
            assert_eq!(image.pixels, expected.pixels);
        }
    }

    #[test]
    fn asymmetric_visible_tiles_have_exact_cores_and_overlap_bounds() {
        let visible = egui::Rect::from_min_max(egui::pos2(0.58, 0.46), egui::pos2(0.99, 0.90));
        let tiles = visible_tiles(RenderKey::new(7, 1.0), (3500.0, 2100.0), visible, 8192).unwrap();
        assert_eq!(tiles.len(), 6);
        assert_eq!(tiles[0].key.region, Some([1023, 0, 2049, 1025]));
        assert_eq!(tiles[5].key.region, Some([3071, 1023, 3500, 2049]));
        assert_eq!(tiles[0].key.image_size((3500.0, 2100.0)), [1026, 1025]);
        assert_eq!(RenderKey::new(0, 1.5).image_size((10.5, 20.75)), [16, 32]);
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
        let mut worker = RenderWorker::new(document.worker_source());
        let keys: Vec<_> = tiles.iter().map(|tile| tile.key).collect();
        let rendered = images(&mut worker, &keys);
        for (tile, image) in tiles.into_iter().zip(rendered) {
            let [x0, y0, x1, y1] = tile.key.region.unwrap();
            assert_eq!((image.width, image.height), (x1 - x0, y1 - y0));
            for y in y0..y1 {
                let full_start = ((y * full.width + x0) * 4) as usize;
                let start = ((y - y0) * image.width * 4) as usize;
                let length = (image.width * 4) as usize;
                assert!(
                    image.rgba()[start..start + length]
                        == full.rgba()[full_start..full_start + length],
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
        state.finish(0, key(1001), Err("old pan".into()));
        state.finish(
            0,
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
        state.finish(0, wanted, Err("current".into()));
        assert!(state.cache.contains_key(&wanted));
    }

    #[test]
    fn tile_sources_capture_edits_and_undo_without_mutating_old_snapshots() {
        use crate::annotations::{Geometry, Kind};
        let mut document = crate::document::tests::sample_document();
        let key = RenderKey {
            region: Some([29, 39, 181, 223]),
            ..RenderKey::new(0, 2.0)
        };
        let before = render(&document, key, 2048).unwrap().pixels;
        let original = document.worker_source();
        document
            .add_annotation(
                0,
                Kind::Ink,
                Geometry::Ink(vec![vec![[0.1, 0.1], [0.3, 0.25]]]),
                "tile ink",
                [1.0, 0.0, 0.0],
                3.0,
            )
            .unwrap();
        let edited = render(&document, key, 2048).unwrap().pixels;
        assert_ne!(edited, before);
        let current = document.worker_source();
        document.undo().unwrap();
        let undone = document.worker_source();
        std::thread::spawn(move || {
            assert_eq!(
                render(&original.open().unwrap(), key, 2048).unwrap().pixels,
                before
            );
            assert_eq!(
                render(&current.open().unwrap(), key, 2048).unwrap().pixels,
                edited
            );
            assert_eq!(
                render(&undone.open().unwrap(), key, 2048).unwrap().pixels,
                before
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn cropbox_intrinsic_rotation_and_transparency_match_tile_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("geometry.pdf");
        crate::document::tests::geometry_fixture(&path);
        let document = PdfDocument::open(path).unwrap();
        let full = document.render_at_scale(0, 2.0).unwrap();
        let tile = document.render_region(0, 2.0, [39, 71, 221, 401]).unwrap();
        let mut worker = RenderWorker::new(document.worker_source());
        let key = RenderKey {
            region: Some([39, 71, 221, 401]),
            ..RenderKey::new(0, 2.0)
        };
        assert_eq!(images(&mut worker, &[key])[0].pixels, tile.pixels);
        assert_eq!((tile.width, tile.height), (182, 330));
        for y in 0..330 {
            let start = y * 182 * 4;
            let full_start = ((y + 71) * 300 + 39) * 4;
            assert!(
                tile.rgba()[start..start + 182 * 4]
                    == full.rgba()[full_start..full_start + 182 * 4],
                "rotated alpha crop row {y}"
            );
        }
    }

    #[test]
    fn authenticated_worker_renders_tiles_and_replacement_has_independent_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("encrypted.pdf");
        crate::document::tests::encrypted_fixture(
            &path,
            "open-secret",
            mupdf::pdf::Permission::ACCESSIBILITY,
            mupdf::pdf::Encryption::Aes256,
        );
        let document = PdfDocument::open_with_password(path, Some("open-secret"))
            .unwrap()
            .unwrap();
        assert!(!document.permissions().copy);
        let mut worker = RenderWorker::new(document.worker_source());
        let key = RenderKey {
            region: Some([12, 32, 269, 377]),
            ..RenderKey::new(0, 3.0)
        };
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let image = loop {
            worker.begin_frame();
            let image = worker.image(key, Priority::Page);
            worker.end_frame(&ctx);
            if let Some(image) = image {
                break image.unwrap();
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!((image.width, image.height), (257, 345));
        assert_eq!(
            image.pixels,
            document
                .render_region(0, 3.0, [12, 32, 269, 377])
                .unwrap()
                .pixels
        );
        let shared = worker.shared.clone();
        drop(worker);
        assert!(shared.0.lock().unwrap().stopped);
        assert!(
            !RenderWorker::new(document.worker_source())
                .shared
                .0
                .lock()
                .unwrap()
                .stopped
        );
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
        assert_eq!(image.rgba().len(), 600 * 300 * 4);
        assert!(
            image
                .rgba()
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
        state.active.insert(stale);
        state.finish(0, stale, Err("stale".into()));
        assert!(state.cache.is_empty());
        assert!(state.active.is_empty());
        state.finish(0, wanted, Err("wanted".into()));
        assert!(state.cache.contains_key(&wanted));
        state.cache.clear();
        state.generation = 1;
        state.active.insert(wanted);
        state.finish(0, wanted, Err("old revision, identical key".into()));
        assert!(state.cache.is_empty());
        assert!(state.active.contains(&wanted));
        state.stopped = true;
        state.finish(1, wanted, Err("after stop".into()));
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
                    pixels: Arc::new(egui::ColorImage::filled([2048, 2048], egui::Color32::WHITE)),
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
        let image = Arc::new(PageImage {
            width: 1026,
            height: 1026,
            pixels: Arc::new(egui::ColorImage::filled([1026, 1026], egui::Color32::WHITE)),
        });
        for x in 0..40 {
            state.insert(
                RenderKey {
                    region: Some([x * 1024, 0, x * 1024 + 1026, 1026]),
                    ..RenderKey::new(0, 1.0)
                },
                Ok(image.clone()),
            );
            assert!(state.cache.values().map(Cached::bytes).sum::<usize>() <= CACHE_BYTES);
        }
        // The byte cap, not the old 12-page cap or 128-tile entry cap, wins.
        assert_eq!(state.cache.len(), 31);
        assert!(state.cache.keys().all(|key| key.region.is_some()));
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
