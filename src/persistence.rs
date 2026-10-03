//! Private, bounded metadata only: never serialize a document, its text, or credentials.
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{
    layout::{LayoutMode, Rotation},
    zoom::Zoom,
};

pub const MAX_RECENT: usize = 32;
pub const MAX_BOOKMARKS: usize = 512;
pub const MAX_SESSION: usize = 16;
const MAX_STATE_BYTES: u64 = 2 * 1024 * 1024;
const SAVE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadingState {
    pub page: usize,
    /// Viewport top-left in original page-relative PDF points, independent of
    /// zoom/DPI. May lie outside the active page in a multi-page layout.
    pub scroll: [f32; 2],
    pub zoom: Zoom,
    pub layout: LayoutMode,
    pub rotation: Rotation,
}

impl ReadingState {
    fn sanitize(&mut self) {
        if let Zoom::Percent(value) = self.zoom
            && (!value.is_finite() || !(0.1..=16.0).contains(&value))
        {
            self.zoom = Zoom::default();
        }
        for offset in &mut self.scroll {
            if !offset.is_finite() {
                *offset = 0.0;
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarState {
    pub open: bool,
    pub width: f32,
    pub pages: bool,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            open: true,
            width: 240.0,
            pages: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub size: [f64; 2],
    /// Physical desktop coordinates, when the window system supports placement.
    pub position: Option<[i32; 2]>,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            size: [960.0, 720.0],
            position: None,
            maximized: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentFile {
    #[serde(with = "native_path")]
    pub path: PathBuf,
    pub reading: ReadingState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    #[serde(with = "native_path")]
    pub path: PathBuf,
    pub reading: ReadingState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionFile {
    #[serde(with = "native_path")]
    pub path: PathBuf,
    pub reading: ReadingState,
    #[serde(default)]
    pub sidebar: SidebarState,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub files: Vec<SessionFile>,
    pub active: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    version: u32,
    pub appearance: crate::native_ui::Appearance,
    pub sidebar: SidebarState,
    pub window: WindowState,
    pub recent: Vec<RecentFile>,
    pub bookmarks: Vec<Bookmark>,
    pub restore_session: bool,
    pub session: Session,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            appearance: crate::native_ui::Appearance::default(),
            sidebar: SidebarState::default(),
            window: WindowState::default(),
            recent: Vec::new(),
            bookmarks: Vec::new(),
            restore_session: false,
            session: Session::default(),
        }
    }
}

impl State {
    fn sanitize(&mut self) {
        if !self.restore_session {
            self.session = Session::default();
        }
        self.session.files.truncate(MAX_SESSION);
        let active_path = self
            .session
            .files
            .get(self.session.active)
            .map(|file| file.path.clone());
        let mut session_paths = std::collections::HashSet::new();
        self.session
            .files
            .retain(|file| file.path.is_absolute() && session_paths.insert(file.path.clone()));
        self.session.active = active_path
            .and_then(|path| self.session.files.iter().position(|file| file.path == path))
            .unwrap_or(0);
        for file in &mut self.session.files {
            file.reading.sanitize();
            if !file.sidebar.width.is_finite() {
                file.sidebar.width = 240.0;
            }
            file.sidebar.width = file.sidebar.width.clamp(200.0, 400.0);
        }
        self.recent.truncate(MAX_RECENT);
        self.bookmarks.truncate(MAX_BOOKMARKS);
        let mut paths = std::collections::HashSet::new();
        self.recent
            .retain(|file| file.path.is_absolute() && paths.insert(file.path.clone()));
        for file in &mut self.recent {
            file.reading.sanitize();
        }
        let mut destinations = std::collections::HashSet::new();
        self.bookmarks.retain(|bookmark| {
            bookmark.path.is_absolute()
                && destinations.insert((bookmark.path.clone(), bookmark.reading.page))
        });
        for bookmark in &mut self.bookmarks {
            bookmark.reading.sanitize();
        }
        if !self.sidebar.width.is_finite() {
            self.sidebar.width = 240.0;
        }
        self.sidebar.width = self.sidebar.width.clamp(200.0, 400.0);
        if self
            .window
            .size
            .iter()
            .any(|size| !size.is_finite() || *size < 320.0 || *size > 16384.0)
        {
            self.window.size = WindowState::default().size;
        }
    }

    pub fn reading(&self, path: &Path) -> Option<&ReadingState> {
        self.recent
            .iter()
            .find(|file| file.path == path)
            .map(|file| &file.reading)
    }

    /// Only call after a successful open; failures must not enter or reorder history.
    pub fn opened(&mut self, path: PathBuf, reading: ReadingState) {
        self.recent.retain(|file| file.path != path);
        self.recent.insert(0, RecentFile { path, reading });
        self.recent.truncate(MAX_RECENT);
    }

    pub fn update_reading(&mut self, path: &Path, reading: ReadingState) {
        if let Some(file) = self.recent.iter_mut().find(|file| file.path == path) {
            file.reading = reading;
        }
    }

    pub fn clear_history(&mut self) {
        self.recent.clear();
        self.session = Session::default();
    }

    pub fn toggle_bookmark(&mut self, path: PathBuf, reading: ReadingState) {
        if let Some(index) = self
            .bookmarks
            .iter()
            .position(|bookmark| bookmark.path == path && bookmark.reading.page == reading.page)
        {
            self.bookmarks.remove(index);
        } else if self.bookmarks.len() < MAX_BOOKMARKS {
            self.bookmarks.push(Bookmark { path, reading });
        }
    }
}

pub fn file_key(path: &Path) -> PathBuf {
    // Resolve relative paths and symlinks so reopening through a chooser or recent
    // files reaches the same metadata. Never use a lossy string as the lookup key.
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

mod native_path {
    use super::*;

    pub fn serialize<S: serde::Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(path) = path.to_str() {
            return serializer.serialize_str(path);
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            serializer.serialize_bytes(path.as_os_str().as_bytes())
        }
        #[cfg(not(unix))]
        Err(serde::ser::Error::custom("PDF path is not valid Unicode"))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Encoded {
            Unicode(String),
            Bytes(Vec<u8>),
        }
        match Encoded::deserialize(deserializer)? {
            Encoded::Unicode(path) => Ok(path.into()),
            Encoded::Bytes(bytes) => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    Ok(std::ffi::OsString::from_vec(bytes).into())
                }
                #[cfg(not(unix))]
                {
                    let _ = bytes;
                    Err(serde::de::Error::custom(
                        "Non-Unicode Unix path in reading state",
                    ))
                }
            }
        }
    }
}

pub struct Store {
    pub state: State,
    path: Option<PathBuf>,
    saved: State,
    next_write: Instant,
}

impl Store {
    pub fn load() -> (Self, Option<String>) {
        let path = ProjectDirs::from("org", "spiralpoint", "Review").map(|dirs| {
            dirs.state_dir()
                .unwrap_or(dirs.data_local_dir())
                .join("state.json")
        });
        Self::load_from(path)
    }

    fn load_from(path: Option<PathBuf>) -> (Self, Option<String>) {
        let loaded = (|| -> Result<State> {
            let path = path
                .as_ref()
                .context("User state directory is unavailable")?;
            let mut file = match File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(State::default());
                }
                Err(error) => return Err(error.into()),
            };
            ensure!(
                file.metadata()?.len() <= MAX_STATE_BYTES,
                "Reading state is too large"
            );
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(MAX_STATE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_STATE_BYTES,
                "Reading state is too large"
            );
            let mut state: State = serde_json::from_slice(&bytes)?;
            ensure!(state.version == 1, "Unsupported reading-state version");
            state.sanitize();
            Ok(state)
        })();
        let (state, error) = match loaded {
            Ok(state) => (state, None),
            Err(error) => (
                State::default(),
                Some(format!(
                    "Could not load reading state; using defaults. {error}"
                )),
            ),
        };
        (
            Self {
                saved: state.clone(),
                state,
                path,
                next_write: Instant::now(),
            },
            error,
        )
    }

    pub fn save_deadline(&self) -> Option<Instant> {
        (self.state != self.saved).then_some(self.next_write)
    }

    pub fn save(&mut self, force: bool) -> Result<()> {
        if self.state == self.saved || (!force && Instant::now() < self.next_write) {
            return Ok(());
        }
        self.next_write = Instant::now() + SAVE_INTERVAL;
        let path = self
            .path
            .as_ref()
            .context("User state directory is unavailable")?;
        let directory = path.parent().context("Invalid reading-state path")?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory)?;
        // Same-directory tempfile + atomic replacement, including on Windows.
        // tempfile uses private permissions on Unix and removes failed writes.
        let bytes = serde_json::to_vec_pretty(&self.state)?;
        ensure!(
            (bytes.len() as u64) < MAX_STATE_BYTES,
            "Reading state is too large"
        );
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(&bytes)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .context("Could not replace reading state")?;
        #[cfg(unix)]
        File::open(directory)?.sync_all()?;
        self.saved = self.state.clone();
        Ok(())
    }

    #[cfg(test)]
    pub fn temporary(directory: &Path) -> Self {
        Self::load_from(Some(directory.join("state.json"))).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn non_unicode_paths_round_trip_without_lossy_keys() {
        use std::os::unix::ffi::OsStringExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory
            .path()
            .join(std::ffi::OsString::from_vec(b"paper-\xff.pdf".to_vec()));
        let mut store = Store::temporary(directory.path());
        store.state.opened(path.clone(), ReadingState::default());
        store
            .state
            .toggle_bookmark(path.clone(), ReadingState::default());
        store.save(true).unwrap();
        let restored = Store::temporary(directory.path());
        assert_eq!(restored.state.recent[0].path, path);
        assert_eq!(restored.state.bookmarks[0].path, path);
    }

    #[test]
    fn restart_round_trip_and_atomic_replacement_store_only_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("résumé.pdf");
        let reading = ReadingState {
            page: 16,
            scroll: [-137.0, 419.5],
            zoom: Zoom::Percent(1.375),
            layout: LayoutMode::Facing,
            rotation: Rotation::Counterclockwise,
        };
        let mut store = Store::temporary(directory.path());
        store.state.opened(path.clone(), reading.clone());
        store.state.toggle_bookmark(path.clone(), reading.clone());
        store.state.appearance = crate::native_ui::Appearance::HighContrast;
        store.state.sidebar = SidebarState {
            open: false,
            width: 327.0,
            pages: true,
        };
        store.state.window = WindowState {
            size: [1137.0, 823.0],
            position: Some([-50, 72]),
            maximized: true,
        };
        store.save(true).unwrap();
        store.state.update_reading(
            &path,
            ReadingState {
                page: 22,
                zoom: Zoom::FitWidth,
                ..reading
            },
        );
        store.save(true).unwrap();
        let restored = Store::temporary(directory.path());
        assert_eq!(store.state, restored.state);
        assert_eq!(restored.state.reading(&path).unwrap().page, 22);
        assert_eq!(restored.state.bookmarks[0].reading.page, 16);
        let json = fs::read_to_string(directory.path().join("state.json")).unwrap();
        for forbidden in ["password", "contents", "text", "query"] {
            assert!(!json.contains(forbidden));
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.path().join("state.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn history_is_deduplicated_bounded_and_clear_does_not_erase_bookmarks() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::temporary(directory.path());
        let path = directory.path().join("first.pdf");
        let reading = ReadingState {
            page: 4,
            ..Default::default()
        };
        store.state.toggle_bookmark(path.clone(), reading.clone());
        for index in 0..MAX_RECENT + 9 {
            store.state.opened(
                directory.path().join(format!("{index}.pdf")),
                ReadingState::default(),
            );
        }
        store.state.opened(path.clone(), reading.clone());
        store.state.opened(path.clone(), reading);
        assert_eq!(store.state.recent.len(), MAX_RECENT);
        assert_eq!(store.state.recent[0].path, path);
        assert_eq!(store.state.recent[1].path, directory.path().join("40.pdf"));
        store.save(true).unwrap();
        store.state.clear_history();
        store.save(true).unwrap();
        let restored = Store::temporary(directory.path());
        assert!(restored.state.recent.is_empty());
        assert_eq!(restored.state.bookmarks.len(), 1);
        // The active viewer cannot silently re-add a cleared history entry.
        store.state.update_reading(&path, ReadingState::default());
        assert!(store.state.recent.is_empty());
        store.state.toggle_bookmark(
            path,
            ReadingState {
                page: 4,
                ..Default::default()
            },
        );
        assert!(store.state.bookmarks.is_empty());
    }

    #[test]
    fn missing_corrupt_oversized_future_and_invalid_values_recover() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        assert!(Store::load_from(Some(path.clone())).1.is_none());
        for bytes in [
            b"not JSON".to_vec(),
            b"{\"version\":99}".to_vec(),
            vec![b' '; MAX_STATE_BYTES as usize + 1],
        ] {
            fs::write(&path, bytes).unwrap();
            let (mut store, error) = Store::load_from(Some(path.clone()));
            assert!(error.is_some());
            assert_eq!(store.state, State::default());
            store.state.sidebar.open = false;
            store.save(true).unwrap();
            assert!(Store::load_from(Some(path.clone())).1.is_none());
        }
        fs::write(&path, format!(r#"{{"sidebar":{{"width":-100}},"window":{{"size":[-10,22]}},"recent":[{{"path":{},"reading":{{"page":999,"scroll":[-3,87],"zoom":{{"Percent":99}}}}}}]}}"#, serde_json::to_string(&directory.path().join("paper.pdf")).unwrap())).unwrap();
        let (store, error) = Store::load_from(Some(path));
        assert!(error.is_none());
        assert_eq!(store.state.sidebar.width, 200.0);
        assert_eq!(store.state.window.size, [960.0, 720.0]);
        assert_eq!(store.state.recent[0].reading.scroll, [-3.0, 87.0]);
        assert_eq!(store.state.recent[0].reading.zoom, Zoom::FitPage);
    }

    #[test]
    fn session_load_is_bounded_deduplicated_sanitized_and_backward_compatible() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, r#"{"version":1}"#).unwrap();
        let legacy = Store::temporary(directory.path());
        assert!(!legacy.state.restore_session);
        assert!(legacy.state.session.files.is_empty());
        let file = |name: &str| SessionFile {
            path: directory.path().join(name),
            reading: ReadingState {
                page: 1,
                scroll: [-17.0, 73.0],
                zoom: Zoom::Percent(99.0),
            },
            sidebar: SidebarState {
                width: 999.0,
                ..Default::default()
            },
        };
        let mut state = State {
            restore_session: true,
            ..Default::default()
        };
        let mut relative = file("ignored.pdf");
        relative.path = "relative.pdf".into();
        state.session.files = vec![
            relative,
            file("first.pdf"),
            file("first.pdf"),
            file("active.pdf"),
        ];
        state.session.active = 3;
        for index in 0..MAX_SESSION + 8 {
            state.session.files.push(file(&format!("{index}.pdf")));
        }
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        let loaded = Store::temporary(directory.path());
        assert_eq!(loaded.state.session.files.len(), 14);
        assert_eq!(loaded.state.session.active, 1);
        let active = &loaded.state.session.files[1];
        assert_eq!(active.path, directory.path().join("active.pdf"));
        assert_eq!(active.reading.scroll, [0.0, 73.0]);
        assert_eq!(active.reading.zoom, Zoom::FitPage);
        assert_eq!(active.sidebar.width, 400.0);
        state.restore_session = false;
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(
            Store::temporary(directory.path())
                .state
                .session
                .files
                .is_empty()
        );
    }

    #[test]
    fn bookmark_limit_preserves_existing_bookmarks() {
        let mut state = State::default();
        for page in 0..MAX_BOOKMARKS + 1 {
            state.toggle_bookmark(
                PathBuf::from("/paper.pdf"),
                ReadingState {
                    page,
                    ..Default::default()
                },
            );
        }
        assert_eq!(state.bookmarks.len(), MAX_BOOKMARKS);
        assert_eq!(state.bookmarks[0].reading.page, 0);
        assert_eq!(
            state.bookmarks.last().unwrap().reading.page,
            MAX_BOOKMARKS - 1
        );
    }
}
