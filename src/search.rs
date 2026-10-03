use std::{
    ops::Range,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use caseless::Caseless;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    document::{PdfDocument, WorkerSource},
    structured_text::{PageText, Quad},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub case_sensitive: bool,
    pub whole_word: bool,
}

pub struct SearchMatch {
    pub page: usize,
    pub quads: Vec<Quad>,
    pub snippet: String,
    /// Byte range in snippet, safe to slice even with non-ASCII text.
    pub emphasis: Range<usize>,
}

struct Request {
    generation: u64,
    query: String,
    options: Options,
    start_page: usize,
    repaint: egui::Context,
}

struct Update {
    generation: u64,
    result: Result<Vec<SearchMatch>, String>,
}

#[derive(Default)]
struct State {
    request: Option<Request>,
    updates: Vec<Update>,
    stopped: bool,
}

struct Worker {
    shared: Arc<(Mutex<State>, Condvar)>,
    generation: Arc<AtomicU64>,
}

impl Worker {
    fn new(source: WorkerSource) -> Self {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let generation = Arc::new(AtomicU64::new(0));
        let worker = shared.clone();
        let current = generation.clone();
        std::thread::spawn(move || {
            // Neither this document nor any of its MuPDF pointers leave here.
            let document = source.open().map_err(|error| format!("{error:#}"));
            loop {
                let request = {
                    let (lock, wake) = &*worker;
                    let mut state = lock.lock().unwrap();
                    while !state.stopped && state.request.is_none() {
                        state = wake.wait(state).unwrap();
                    }
                    if state.stopped {
                        return;
                    }
                    state.request.take().unwrap()
                };
                let cancelled = || current.load(Ordering::Relaxed) != request.generation;
                let pages = document.as_ref().map_or(1, PdfDocument::page_count);
                for offset in 0..pages {
                    if cancelled() {
                        break;
                    }
                    let page = (request.start_page + offset) % pages;
                    let result = match &document {
                        Ok(document) => document
                            .structured_text(page)
                            .map(|text| {
                                find_matches(
                                    &text,
                                    page,
                                    &request.query,
                                    request.options,
                                    cancelled,
                                )
                            })
                            .map_err(|error| format!("{error:#}")),
                        Err(error) => Err(error.clone()),
                    };
                    let failed = result.is_err();
                    {
                        let mut state = worker.0.lock().unwrap();
                        // Check under the same lock used to replace the request.
                        if state.stopped || cancelled() {
                            break;
                        }
                        state.updates.push(Update {
                            generation: request.generation,
                            result,
                        });
                    }
                    request.repaint.request_repaint();
                    if failed {
                        break;
                    }
                }
            }
        });
        Self { shared, generation }
    }

    fn cancel(&self) {
        let mut state = self.shared.0.lock().unwrap();
        self.generation.fetch_add(1, Ordering::Relaxed);
        state.request = None;
        state.updates.clear();
    }

    fn start(&self, query: String, options: Options, start_page: usize, repaint: &egui::Context) {
        let mut state = self.shared.0.lock().unwrap();
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        state.updates.clear();
        state.request = Some(Request {
            generation,
            query,
            options,
            start_page,
            repaint: repaint.clone(),
        });
        self.shared.1.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.shared.0.lock().unwrap().stopped = true;
        self.shared.1.notify_one();
    }
}

pub struct Search {
    pub open: bool,
    pub query: String,
    pub options: Options,
    pub matches: Vec<SearchMatch>,
    pub selected: Option<usize>,
    pub submitted: String,
    pub scanned: usize,
    pub total: usize,
    pub error: Option<String>,
    text_revision: u64,
    worker: Worker,
}

impl Search {
    pub fn new(document: &PdfDocument) -> Self {
        Self {
            open: false,
            query: String::new(),
            options: Options::default(),
            matches: Vec::new(),
            selected: None,
            submitted: String::new(),
            scanned: 0,
            total: document.page_count(),
            error: None,
            text_revision: document.text_revision(),
            worker: Worker::new(document.worker_source()),
        }
    }

    pub fn scanning(&self) -> bool {
        !self.submitted.is_empty() && self.error.is_none() && self.scanned < self.total
    }

    pub fn clear_results(&mut self) {
        self.worker.cancel();
        self.matches.clear();
        self.selected = None;
        self.submitted.clear();
        self.scanned = 0;
        self.error = None;
    }

    /// Layer visibility changes the worker source, not just its text cache.
    pub fn reload_document(&mut self, document: &PdfDocument, ctx: &egui::Context) {
        let restart = !self.submitted.is_empty();
        self.clear_results();
        self.worker = Worker::new(document.worker_source());
        self.text_revision = document.text_revision();
        if restart {
            self.start(document, ctx);
        }
    }

    pub fn start(&mut self, document: &PdfDocument, ctx: &egui::Context) {
        self.clear_results();
        self.text_revision = document.text_revision();
        self.submitted = self.query.trim().to_owned();
        if !self.submitted.is_empty() {
            self.worker.start(
                self.submitted.clone(),
                self.options,
                document.current_page(),
                ctx,
            );
        }
    }

    pub fn refresh_text(&mut self, document: &PdfDocument, ctx: &egui::Context) {
        if document.text_revision() != self.text_revision {
            if self.submitted.is_empty() {
                self.text_revision = document.text_revision();
            } else {
                self.start(document, ctx);
            }
        }
    }

    /// Only owned results are polled on the UI thread. The first available hit
    /// is selected once; later pages never steal navigation from the reader.
    pub fn poll(&mut self) -> Option<usize> {
        let updates = {
            let mut state = self.worker.shared.0.lock().unwrap();
            std::mem::take(&mut state.updates)
        };
        let generation = self.worker.generation.load(Ordering::Relaxed);
        let mut reveal = None;
        for update in updates {
            if update.generation != generation {
                continue;
            }
            match update.result {
                Ok(hits) => {
                    self.scanned += 1;
                    if hits.is_empty() {
                        continue;
                    }
                    let page = hits[0].page;
                    let index = self.matches.partition_point(|hit| hit.page < page);
                    if let Some(selected) = &mut self.selected {
                        if *selected >= index {
                            *selected += hits.len();
                        }
                    } else {
                        self.selected = Some(index);
                        reveal = Some(page);
                    }
                    self.matches.splice(index..index, hits);
                }
                Err(error) => self.error = Some(error),
            }
        }
        reveal
    }

    pub fn advance(&mut self, current_page: usize, backwards: bool) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let count = self.matches.len();
        let index = match self
            .selected
            .filter(|&index| self.matches[index].page == current_page)
        {
            Some(index) if backwards => (index + count - 1) % count,
            Some(index) => (index + 1) % count,
            None if backwards => {
                (self.matches.partition_point(|hit| hit.page <= current_page) + count - 1) % count
            }
            None => self.matches.partition_point(|hit| hit.page < current_page) % count,
        };
        self.selected = Some(index);
        Some(self.matches[index].page)
    }
}

/// Literal Unicode search over the shared text model. Whitespace runs (including
/// line/paragraph breaks) compare as one space. Case folding can expand scalars;
/// source ranges keep highlights and whole-word boundaries in original text.
pub(crate) fn find_matches(
    text: &PageText,
    page: usize,
    query: &str,
    options: Options,
    cancelled: impl Fn() -> bool,
) -> Vec<SearchMatch> {
    let (haystack, source) = normalized(
        text.chars
            .iter()
            .enumerate()
            .take_while(|(i, _)| i % 1024 != 0 || !cancelled())
            .map(|(_, ch)| ch.ch),
        options,
    );
    let (needle, _) = normalized(query.trim().chars(), options);
    if needle.is_empty() || haystack.len() < needle.len() || cancelled() {
        return Vec::new();
    }
    // Unicode segmentation uses byte offsets; convert once to scalar boundaries.
    let mut boundaries = Vec::new();
    if options.whole_word {
        boundaries.resize(text.chars.len() + 1, false);
        let plain = text.plain_text();
        let mut offset = 0;
        for piece in plain.split_word_bounds() {
            boundaries[offset] = true;
            offset += piece.chars().count();
        }
        boundaries[offset] = true;
    }

    // KMP avoids quadratic scanning for long repeated prefixes.
    let mut prefix = vec![0; needle.len()];
    let mut matched = 0;
    for i in 1..needle.len() {
        while matched > 0 && needle[i] != needle[matched] {
            matched = prefix[matched - 1];
        }
        if needle[i] == needle[matched] {
            matched += 1;
        }
        prefix[i] = matched;
    }
    let mut hits = Vec::new();
    matched = 0;
    for (i, &ch) in haystack.iter().enumerate() {
        if i % 1024 == 0 && cancelled() {
            return Vec::new();
        }
        while matched > 0 && ch != needle[matched] {
            matched = prefix[matched - 1];
        }
        if ch == needle[matched] {
            matched += 1;
        }
        if matched == needle.len() {
            let first = i + 1 - matched;
            let range = source[first].start..source[i].end;
            // Never match half a folded scalar (e.g. one 's' inside ß).
            let complete = (first == 0 || source[first - 1] != source[first])
                && (i + 1 == source.len() || source[i + 1] != source[i]);
            if complete
                && (!options.whole_word || (boundaries[range.start] && boundaries[range.end]))
            {
                let (snippet, emphasis) = snippet(text, range.clone());
                hits.push(SearchMatch {
                    page,
                    quads: text.chars[range].iter().filter_map(|ch| ch.quad).collect(),
                    snippet,
                    emphasis,
                });
                // Occurrences are non-overlapping, like the original search.
                matched = 0;
            } else {
                matched = prefix[matched - 1];
            }
        }
    }
    hits
}

fn normalized(
    chars: impl Iterator<Item = char>,
    options: Options,
) -> (Vec<char>, Vec<Range<usize>>) {
    let mut output = Vec::new();
    let mut source: Vec<Range<usize>> = Vec::new();
    for (i, ch) in chars.enumerate() {
        if ch.is_whitespace() {
            if output.last() == Some(&' ') {
                source.last_mut().unwrap().end = i + 1;
            } else {
                output.push(' ');
                source.push(i..i + 1);
            }
        } else if options.case_sensitive {
            output.push(ch);
            source.push(i..i + 1);
        } else {
            for folded in std::iter::once(ch).default_case_fold() {
                output.push(folded);
                source.push(i..i + 1);
            }
        }
    }
    (output, source)
}

fn snippet(text: &PageText, range: Range<usize>) -> (String, Range<usize>) {
    // Bound context and the displayed match, even for a very long query.
    let start = range.start.saturating_sub(36);
    let match_end = range.end.min(range.start + 80);
    let end = (match_end + 36).min(text.chars.len());
    let compact = |range| {
        text.text(range)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut snippet = if start > 0 {
        "…".into()
    } else {
        String::new()
    };
    let before = compact(start..range.start);
    snippet.push_str(&before);
    if !before.is_empty() && text.chars[range.start - 1].ch.is_whitespace() {
        snippet.push(' ');
    }
    let emphasis_start = snippet.len();
    snippet.push_str(&compact(range.start..match_end));
    let emphasis = emphasis_start..snippet.len();
    if match_end < range.end {
        snippet.push('…');
        return (snippet, emphasis);
    }
    let after = compact(match_end..end);
    if !after.is_empty()
        && text
            .chars
            .get(match_end)
            .is_some_and(|ch| ch.ch.is_whitespace())
    {
        snippet.push(' ');
    }
    snippet.push_str(&after);
    if end < text.chars.len() {
        snippet.push('…');
    }
    (snippet, emphasis)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{document::tests::sample_pdf, structured_text::TextChar};
    use std::time::{Duration, Instant};

    fn page(text: &str) -> PageText {
        PageText {
            chars: text
                .chars()
                .enumerate()
                .map(|(i, ch)| TextChar {
                    ch,
                    quad: (!ch.is_whitespace()).then_some([
                        [i as f32 / 100.0, 0.1],
                        [(i + 1) as f32 / 100.0, 0.1],
                        [(i + 1) as f32 / 100.0, 0.2],
                        [i as f32 / 100.0, 0.2],
                    ]),
                    bidi: 0,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn hits(text: &str, query: &str, options: Options) -> Vec<SearchMatch> {
        find_matches(&page(text), 7, query, options, || false)
    }

    pub fn finish(search: &mut Search) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while search.scanning() {
            search.poll();
            assert!(Instant::now() < deadline, "search did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(search.error.is_none(), "{:?}", search.error);
    }

    #[test]
    fn full_unicode_folding_maps_expansions_without_partial_scalar_matches() {
        let options = Options::default();
        let text = "Straße STRASSE strasse Σςσ École école İ i";
        let matches = hits(text, "strasse", options);
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].quads.len(), 6);
        assert_eq!(matches[1].quads.len(), 7);
        assert_eq!(&matches[0].snippet[matches[0].emphasis.clone()], "Straße");
        assert_eq!(hits(text, "σ", options).len(), 3);
        assert_eq!(hits(text, "ÉCOLE", options).len(), 2);
        assert_eq!(hits("ß s", "s", options).len(), 1);
        assert_eq!(hits("İ i", "i", options).len(), 1);
        assert_eq!(hits("İ i", "i\u{307}", options).len(), 1);
        let sensitive = Options {
            case_sensitive: true,
            ..options
        };
        assert_eq!(hits(text, "strasse", sensitive).len(), 1);
        assert_eq!(hits(text, "École", sensitive).len(), 1);
    }

    #[test]
    fn whole_words_use_unicode_boundaries_in_original_text() {
        let words = Options {
            whole_word: true,
            ..Default::default()
        };
        assert_eq!(
            hits("cat scatter cat_cat cat2 (cat) cat", "cat", words).len(),
            3
        );
        assert_eq!(hits("élan préélan élan", "ÉLAN", words).len(), 2);
        assert_eq!(hits("e\u{301} e e\u{301}x", "e", words).len(), 1);
        assert_eq!(hits("e\u{301} e e\u{301}x", "e\u{301}", words).len(), 1);
        assert_eq!(hits("אב אבג (אב)", "אב", words).len(), 2);
        assert_eq!(hits("中文 中", "中", words).len(), 2);
        assert_eq!(hits("Straße STRASSE straßex", "strasse", words).len(), 2);
    }

    #[test]
    fn multiline_matching_highlights_original_glyphs_and_compacts_snippets() {
        let text = crate::document::tests::sample_document()
            .structured_text(0)
            .unwrap();
        let matches = find_matches(&text, 0, " needle \t phrase ", Options::default(), || false);
        assert_eq!(matches.len(), 1);
        let hit = &matches[0];
        assert_eq!(hit.quads.len(), 12);
        assert!(hit.quads[6][0][1] > hit.quads[0][0][1]);
        assert_eq!(hit.snippet, "Alpha alpha Needle phrase");
        assert_eq!(&hit.snippet[hit.emphasis.clone()], "Needle phrase");
        let alpha = find_matches(&text, 0, "ALPHA", Options::default(), || false);
        assert_eq!(alpha.len(), 2);
        assert!((alpha[0].quads[0][0][0] - 0.1).abs() < 0.001);
        assert!(alpha[1].quads[0][0][0] > alpha[0].quads[4][1][0]);
        assert!(hits("abc", "  ", Options::default()).is_empty());
        assert!(hits("abc", "abcd", Options::default()).is_empty());
        let columns = crate::structured_text::tests::selection_pdf();
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), columns).unwrap();
        let text = PdfDocument::open(file.path())
            .unwrap()
            .structured_text(0)
            .unwrap();
        let rotated = find_matches(&text, 0, "Rotated", Options::default(), || false);
        assert_eq!(rotated.len(), 1);
        assert_eq!(rotated[0].quads[0][0][0], rotated[0].quads[0][1][0]);
        assert!(rotated[0].quads[0][0][1] > rotated[0].quads[0][1][1]);
    }

    #[test]
    fn snippets_preserve_unicode_context_boundaries_and_bound_long_matches() {
        for (text, query, expected) in [
            ("début\n\nStraße\t終わり", "strasse", "début Straße 終わり"),
            ("catapult", "cat", "catapult"),
            ("catapult", "pult", "catapult"),
        ] {
            let hits = hits(text, query, Options::default());
            assert_eq!(hits[0].snippet, expected);
            assert!(!hits[0].emphasis.is_empty());
        }
        let text = format!("{}needle{}", "前".repeat(80), "後".repeat(90));
        let matches = hits(&text, "needle", Options::default());
        assert_eq!(
            matches[0].snippet,
            format!("…{}needle{}…", "前".repeat(36), "後".repeat(36))
        );
        assert_eq!(&matches[0].snippet[matches[0].emphasis.clone()], "needle");
        let text = "長".repeat(300);
        let hits = hits(&text, &text, Options::default());
        assert_eq!(hits[0].snippet, format!("{}…", "長".repeat(80)));
    }

    #[test]
    fn cancellation_during_matching_discards_all_partial_hits() {
        let calls = std::cell::Cell::new(0);
        let hits = find_matches(
            &page(&"alpha ".repeat(1500)),
            0,
            "alpha",
            Options::default(),
            || {
                calls.set(calls.get() + 1);
                // Nine normalization checks, then one before scanning. Cancel
                // after the first 1024-character chunk has produced hits.
                calls.get() >= 12
            },
        );
        assert!(hits.is_empty());
        assert_eq!(calls.get(), 12);
    }

    #[test]
    fn partial_navigation_preserves_selection_when_earlier_pages_arrive() {
        let document = crate::document::tests::sample_document();
        let mut search = Search::new(&document);
        search.total = 5;
        search.submitted = "alpha".into();
        let append = |search: &Search, page| {
            search.worker.shared.0.lock().unwrap().updates.push(Update {
                generation: 0,
                result: Ok(find_matches(
                    &super::tests::page("alpha alpha"),
                    page,
                    "alpha",
                    Options::default(),
                    || false,
                )),
            });
        };
        append(&search, 3);
        assert_eq!(search.poll(), Some(3));
        assert!(search.scanning());
        assert_eq!(search.advance(3, false), Some(3));
        assert_eq!(search.selected, Some(1));
        append(&search, 4);
        assert_eq!(search.poll(), None);
        assert_eq!(search.advance(3, false), Some(4));
        append(&search, 0);
        assert_eq!(search.poll(), None);
        assert_eq!(search.selected, Some(4));
        assert_eq!(search.matches[4].page, 4);
        assert_eq!(search.advance(4, true), Some(3));
        search.selected = Some(0);
        assert_eq!(search.advance(0, true), Some(4));
        assert_eq!(search.advance(4, false), Some(0));
    }

    #[test]
    fn worker_authenticates_search_without_copy_permission_and_replaces_queries() {
        use mupdf::pdf::{Encryption, Permission};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked.pdf");
        crate::document::tests::encrypted_fixture(
            &path,
            "open-secret",
            Permission::ACCESSIBILITY,
            Encryption::Aes256,
        );
        let document = PdfDocument::open_with_password(&path, Some("open-secret"))
            .unwrap()
            .unwrap();
        assert!(!document.permissions().copy);
        let mut search = Search::new(&document);
        let ctx = egui::Context::default();
        search.query = "Chapter".into();
        search.start(&document, &ctx);
        let stale = search.worker.generation.load(Ordering::Relaxed);
        search.query = "alpha".into();
        search.start(&document, &ctx);
        // Model a late old page/error reaching the receiver after replacement.
        search.worker.shared.0.lock().unwrap().updates.push(Update {
            generation: stale,
            result: Err("stale error".into()),
        });
        search.worker.shared.0.lock().unwrap().updates.push(Update {
            generation: stale,
            result: Ok(hits("Chapter", "chapter", Options::default())),
        });
        finish(&mut search);
        assert_eq!(search.scanned, 2);
        assert_eq!(search.matches.len(), 1);
        assert_eq!(search.matches[0].page, 1);
        assert_eq!(search.matches[0].snippet, "Last alpha");
        search.query = "Chapter".into();
        search.start(&document, &ctx);
        search.clear_results();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(search.poll(), None);
        assert!(search.matches.is_empty());
        assert!(!search.scanning());
        search.query = " ".into();
        search.start(&document, &ctx);
        assert!(!search.scanning());
    }

    #[test]
    fn layer_changes_replace_the_worker_source_without_losing_query_options() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), crate::inspection::tests::fixture()).unwrap();
        let mut document = PdfDocument::open(file.path()).unwrap();
        let mut search = Search::new(&document);
        search.query = "Draft".into();
        search.open = true;
        search.options.case_sensitive = true;
        let ctx = egui::Context::default();
        search.start(&document, &ctx);
        finish(&mut search);
        assert_eq!(search.matches.len(), 1);
        let mut layers = crate::inspection::Inspection::read(document.pdf())
            .layers
            .unwrap();
        layers[0].enabled = false;
        document.set_layer_visibility(&layers).unwrap();
        search.reload_document(&document, &ctx);
        assert!(search.open && search.options.case_sensitive);
        assert_eq!(search.query, "Draft");
        assert_eq!(search.submitted, "Draft");
        assert!(search.scanning());
        finish(&mut search);
        assert!(search.matches.is_empty());
        layers[0].enabled = true;
        document.set_layer_visibility(&layers).unwrap();
        search.reload_document(&document, &ctx);
        finish(&mut search);
        assert_eq!(search.matches.len(), 1);
    }

    #[test]
    fn revision_change_restarts_submitted_query_and_rejects_previous_generation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), sample_pdf("", false)).unwrap();
        let document = PdfDocument::open(file.path()).unwrap();
        let mut search = Search::new(&document);
        let ctx = egui::Context::default();
        search.query = "amber".into();
        search.start(&document, &ctx);
        finish(&mut search);
        assert!(search.matches.is_empty());
        // The worker is already open before recognition changes the shared cache.
        let recognized = crate::ocr::tests::word_text("amber fox");
        let quads: Vec<_> = recognized.chars[..5]
            .iter()
            .filter_map(|ch| ch.quad)
            .collect();
        document.set_recognized_text(0, recognized).unwrap();
        assert_eq!(document.text_revision(), 1);
        let old_generation = search.worker.generation.load(Ordering::Relaxed);
        search.worker.shared.0.lock().unwrap().updates.push(Update {
            generation: old_generation,
            result: Ok(hits("stale amber", "amber", Options::default())),
        });
        search.refresh_text(&document, &ctx);
        assert!(search.matches.is_empty());
        assert_eq!(search.submitted, "amber");
        assert_eq!(search.text_revision, document.text_revision());
        assert!(search.worker.generation.load(Ordering::Relaxed) > old_generation);
        finish(&mut search);
        assert_eq!(search.matches.len(), 1);
        assert_eq!(search.matches[0].page, 0);
        assert_eq!(search.matches[0].snippet, "amber fox");
        assert_eq!(search.matches[0].quads, quads);
        assert!(
            document
                .set_recognized_text(1, crate::ocr::tests::word_text("amber"))
                .is_err()
        );
        assert_eq!(document.text_revision(), 1);
        document.invalidate_text();
        search.refresh_text(&document, &ctx);
        assert_eq!(search.text_revision, 2);
        assert!(search.matches.is_empty());
        finish(&mut search);
        assert!(search.matches.is_empty());
        assert_eq!(search.scanned, 2);
    }

    #[test]
    fn worker_starts_at_current_page_wraps_and_keeps_dense_page_results() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let text = format!(
            "BT /F1 8 Tf 40 380 Td {} ET",
            "(alpha) Tj 0 -10 Td ".repeat(30)
        );
        std::fs::write(file.path(), sample_pdf(&text, false)).unwrap();
        let mut document = PdfDocument::open(file.path()).unwrap();
        document.go_to_page(1);
        let mut search = Search::new(&document);
        search.query = " ALPHA ".into();
        search.start(&document, &egui::Context::default());
        finish(&mut search);
        assert_eq!(search.matches.len(), 31);
        assert_eq!(search.selected, Some(30));
        assert_eq!(search.advance(1, false), Some(0));
        assert_eq!(search.selected, Some(0));
        assert_eq!(search.advance(0, true), Some(1));
        assert_eq!(search.selected, Some(30));
    }

    #[test]
    #[ignore = "requires REVIEW_TEST_PDF pointing to the OpenID Connect handbook"]
    fn handbook_search_matches_known_page_numbers() {
        let document = PdfDocument::open(std::env::var("REVIEW_TEST_PDF").unwrap()).unwrap();
        assert_eq!(document.page_count(), 45);
        let mut search = Search::new(&document);
        search.query = "Recap".into();
        search.start(&document, &egui::Context::default());
        finish(&mut search);
        let pages: Vec<_> = search.matches.iter().map(|hit| hit.page + 1).collect();
        assert_eq!(pages, [2, 2, 2, 7, 17, 17, 44]);
    }

    #[test]
    #[ignore = "exports search PDF to REVIEW_FIXTURE_DIR for native tests"]
    fn export_search_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        std::fs::write(directory.join("search.pdf"), sample_pdf(
            "BT /F1 16 Tf 40 350 Td (Alpha alpha alphabet) Tj 0 -24 Td (Needle) Tj 0 -24 Td (phrase) Tj ET", false,
        )).unwrap();
        let filler = format!(
            "BT /F1 6 Tf 40 250 Td {} ET",
            "(Progressive scanning keeps navigation responsive.) Tj 0 -8 Td ".repeat(24)
        );
        let bytes = sample_pdf(
            &format!("BT /F1 16 Tf 40 350 Td (Alpha alpha alphabet) Tj ET {filler}"),
            false,
        );
        let mut pdf = mupdf::pdf::PdfDocument::try_from(
            mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap(),
        )
        .unwrap();
        for _ in 0..1398 {
            pdf.duplicate_page(0).unwrap();
        }
        pdf.save(directory.join("progress.pdf").to_str().unwrap())
            .unwrap();
    }
}
