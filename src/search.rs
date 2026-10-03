use anyhow::Result;

use crate::document::{PdfDocument, SearchMatch};

#[derive(Default)]
pub struct Search {
    pub open: bool,
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub selected: Option<usize>,
    pub next_page: Option<usize>,
    pub submitted: String,
    start_page: usize,
}

impl Search {
    pub fn clear_results(&mut self) {
        self.matches.clear();
        self.selected = None;
        self.next_page = None;
        self.submitted.clear();
    }

    pub fn start(&mut self, document: &PdfDocument) {
        self.clear_results();
        self.submitted = self.query.trim().to_owned();
        if !self.submitted.is_empty() {
            self.start_page = document.current_page();
            self.next_page = Some(0);
        }
    }

    /// Search one page per frame, allowing input between pages.
    pub fn step(&mut self, document: &PdfDocument) -> Result<Option<usize>> {
        let Some(page) = self.next_page else {
            return Ok(None);
        };
        match document.search_page(page, &self.submitted) {
            Ok(matches) => self.matches.extend(matches),
            Err(error) => {
                self.clear_results();
                return Err(error);
            }
        }
        if page + 1 < document.page_count() {
            self.next_page = Some(page + 1);
            return Ok(None);
        }
        self.next_page = None;
        if self.matches.is_empty() {
            return Ok(None);
        }
        let index = self
            .matches
            .partition_point(|hit| hit.page < self.start_page);
        self.selected = Some(index % self.matches.len());
        Ok(self.selected.map(|index| self.matches[index].page))
    }

    pub fn advance(&mut self, current_page: usize, backwards: bool) -> Option<usize> {
        if self.matches.is_empty() || self.next_page.is_some() {
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

#[cfg(test)]
mod tests {
    use super::Search;
    use crate::document::tests::sample_document;

    #[test]
    fn search_starts_at_current_page_and_wraps_both_ways() {
        let mut document = sample_document();
        document.go_to_page(1);
        let mut search = Search {
            query: " ALPHA ".into(),
            ..Default::default()
        };
        search.start(&document);
        assert_eq!(search.step(&document).unwrap(), None);
        assert_eq!(search.next_page, Some(1));
        assert_eq!(search.step(&document).unwrap(), Some(1));
        assert_eq!(search.matches.len(), 3);
        assert_eq!(search.selected, Some(2));
        assert_eq!(search.advance(1, false), Some(0));
        assert_eq!(search.selected, Some(0));
        assert_eq!(search.advance(0, true), Some(1));
        assert_eq!(search.selected, Some(2));
    }

    #[test]
    fn query_changes_cancel_inflight_work_and_remove_old_results() {
        let document = sample_document();
        let mut search = Search {
            query: "alpha".into(),
            ..Default::default()
        };
        search.start(&document);
        search.step(&document).unwrap();
        search.query = "not present".into();
        search.start(&document);
        assert!(search.matches.is_empty());
        for _ in 0..2 {
            search.step(&document).unwrap();
        }
        assert_eq!(search.selected, None);
        assert_eq!(search.advance(0, false), None);
        search.query = "   ".into();
        search.start(&document);
        assert_eq!(search.next_page, None);
        assert!(search.matches.is_empty());
    }
}
