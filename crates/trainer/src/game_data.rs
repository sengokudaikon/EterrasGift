//! The game's definitions, kept by the app: read from the shipped files (`crate::files`) on a thread of their own, saved
//! beside the trainer's other data, and shared through a watch channel. Nothing here touches the game process, and
//! nothing asks the game for definitions; a session only checks that it runs the build they were read from.
//!
//! A start with an unchanged install loads the saved catalog: every file it was read from, the two hashed binaries
//! among them, still has the size and write time it had, so nothing is decoded or hashed again. A changed install (a
//! Steam update) is read again, at start and whenever a connect asks for the folder its game runs from.

use crate::catalog::{CatalogIdentity, EXTRACTION_VERSION, GameLocale};
use crate::catalog_cache::CatalogCache;
use crate::files::{self, FileCatalog, Fingerprint, SAVED_LOCALE};
use eg_core::fault::FaultLog;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime};
use tokio::sync::{oneshot, watch};

/// The index of the saved catalog: the folder and the files it was read from.
const INDEX_FILE: &str = "files.json";
const INDEX_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq)]
pub enum GameData {
    /// No game folder is known yet.
    NoGame,
    Reading,
    Ready(Arc<FileCatalog>),
    /// The files could not be read; the reason is shown, and the trainer cannot open without them.
    Unavailable(String),
}

/// The service's side for its users: the current state, and a way to ask for a folder's catalog.
#[derive(Clone)]
pub struct GameDataHandle {
    requests: Option<mpsc::Sender<Request>>,
    state: watch::Receiver<GameData>,
}

struct Request {
    /// The game's folder; none for the one kept.
    folder: Option<PathBuf>,
    /// Read the files even if a current catalog is kept or saved.
    again: bool,
    reply: Option<oneshot::Sender<GameData>>,
}

impl std::fmt::Debug for GameDataHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameDataHandle").field("state", &*self.state.borrow()).finish_non_exhaustive()
    }
}

impl GameDataHandle {
    /// A handle that holds one state and reads nothing, for tests and fakes.
    pub fn fixed(state: GameData) -> Self {
        Self::published(watch::channel(state).1)
    }

    /// A handle that follows a state published elsewhere and reads nothing: a harness whose game ships no Last Epoch
    /// files stands in for the service with it.
    pub fn published(state: watch::Receiver<GameData>) -> Self {
        Self { requests: None, state }
    }

    pub fn subscribe(&self) -> watch::Receiver<GameData> {
        self.state.clone()
    }

    pub fn current(&self) -> GameData {
        self.state.borrow().clone()
    }

    /// The catalog for the game in `folder`, current: read again first if the folder differs from the one kept or a
    /// file it was read from changed since.
    pub async fn for_folder(&self, folder: &Path) -> GameData {
        self.ask(Some(folder), false).await
    }

    /// Reads the files again, whatever is kept or saved (Settings' Read again): those in `folder`, else the kept
    /// folder's.
    pub async fn read_again(&self, folder: Option<&Path>) -> GameData {
        self.ask(folder, true).await
    }

    async fn ask(&self, folder: Option<&Path>, again: bool) -> GameData {
        let Some(requests) = &self.requests else {
            return self.current();
        };
        let stopped = || GameData::Unavailable("The game data service has stopped.".into());
        let (reply, answer) = oneshot::channel();
        let request = Request {
            folder: folder.map(Path::to_path_buf),
            again,
            reply: Some(reply),
        };
        if requests.send(request).is_err() {
            return stopped();
        }
        answer.await.unwrap_or_else(|_| stopped())
    }
}

/// Starts the service on its own thread. With `folder` (the installed game, when found), it reads at once; with
/// `store`, it saves what it reads and loads it back on the next start. The thread ends with the last handle.
pub fn start(folder: Option<PathBuf>, store: Option<PathBuf>, faults: Option<FaultLog>) -> std::io::Result<GameDataHandle> {
    let (publish, state) = watch::channel(GameData::NoGame);
    let (requests, incoming) = mpsc::channel::<Request>();
    if folder.is_some() {
        let _ = requests.send(Request {
            folder,
            again: false,
            reply: None,
        });
    }
    let mut service = Service {
        store: store.map(Store::new),
        faults,
        publish,
        folder: None,
    };
    std::thread::Builder::new().name("game data".into()).spawn(move || {
        while let Ok(request) = incoming.recv() {
            let state = match request.folder.or_else(|| service.folder.clone()) {
                Some(folder) => service.serve(&folder, request.again),
                None => GameData::NoGame,
            };
            if let Some(reply) = request.reply {
                let _ = reply.send(state);
            }
        }
    })?;
    Ok(GameDataHandle { requests: Some(requests), state })
}

struct Service {
    store: Option<Store>,
    faults: Option<FaultLog>,
    publish: watch::Sender<GameData>,
    /// The folder the published state is for.
    folder: Option<PathBuf>,
}

impl Service {
    fn serve(&mut self, folder: &Path, again: bool) -> GameData {
        let same_folder = self.folder.as_deref().is_some_and(|kept| eg_core::paths::same_folder(kept, folder));
        if !again
            && same_folder
            && let GameData::Ready(catalog) = &*self.publish.borrow()
            && catalog.is_current()
        {
            return GameData::Ready(catalog.clone());
        }
        self.folder = Some(folder.to_path_buf());
        self.publish.send_replace(GameData::Reading);
        let saved = match &self.store {
            Some(store) if !again => store.load(folder).unwrap_or_else(|e| self.fault("Loading the saved game data", &e)),
            _ => None,
        };
        let state = match saved {
            Some(catalog) => GameData::Ready(Arc::new(catalog)),
            None => match files::read(folder) {
                Ok(catalog) => {
                    if let Some(Err(e)) = self.store.as_ref().map(|store| store.save(folder, &catalog)) {
                        self.fault::<()>("Saving the game data", &e);
                    }
                    GameData::Ready(Arc::new(catalog))
                }
                Err(reason) => {
                    self.fault::<()>("Reading game data from the game files", &reason);
                    GameData::Unavailable(reason)
                }
            },
        };
        self.publish.send_replace(state.clone());
        state
    }

    fn fault<T: Default>(&self, operation: &str, reason: &str) -> T {
        if let Some(faults) = &self.faults {
            faults.write(operation, Some(reason), None, Some("The game files are read again when a game connects."));
        }
        T::default()
    }
}

/// The saved catalog: one build's, under `<store>/catalog` (a [`CatalogCache`]), with its index.
struct Store {
    directory: PathBuf,
    cache: CatalogCache,
}

impl Store {
    fn new(directory: PathBuf) -> Self {
        Self {
            cache: CatalogCache::new(directory.join("catalog")),
            directory,
        }
    }

    fn identity(binary_hash: &str, metadata_hash: &str) -> CatalogIdentity {
        CatalogIdentity {
            binary_hash: binary_hash.to_owned(),
            metadata_hash: metadata_hash.to_owned(),
            locale: GameLocale {
                language: SAVED_LOCALE.into(),
                game_version: String::new(),
            },
            extraction_version: EXTRACTION_VERSION,
        }
    }

    /// The saved catalog for `folder`, if every file it was read from is unchanged. No index, another folder or a
    /// changed file is a miss; a damaged save is an error (and then a miss).
    fn load(&self, folder: &Path) -> Result<Option<FileCatalog>, String> {
        let index = match std::fs::read(self.directory.join(INDEX_FILE)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        let index: Value = serde_json::from_slice(&index).map_err(|e| format!("{INDEX_FILE}: {e}"))?;
        let text = |key: &str| index[key].as_str().map(str::to_owned).ok_or_else(|| format!("{INDEX_FILE}: no {key}"));
        if index["version"].as_u64() != Some(INDEX_VERSION) || index["extraction_version"].as_u64() != Some(u64::from(EXTRACTION_VERSION)) {
            return Ok(None);
        }
        if !eg_core::paths::same_folder(Path::new(&text("folder")?), folder) {
            return Ok(None);
        }
        let sources = index["sources"]
            .as_array()
            .ok_or_else(|| format!("{INDEX_FILE}: no sources"))?
            .iter()
            .map(|source| {
                let path = PathBuf::from(source["path"].as_str().ok_or_else(|| format!("{INDEX_FILE}: a source without a path"))?);
                let print = match (source["len"].as_u64(), source["modified"].as_u64()) {
                    (Some(len), Some(nanos)) => Some((len, SystemTime::UNIX_EPOCH + Duration::from_nanos(nanos))),
                    _ => None,
                };
                Ok((path, print))
            })
            .collect::<Result<Vec<(PathBuf, Option<Fingerprint>)>, String>>()?;
        if sources.is_empty() || sources.iter().any(|(path, print)| files::fingerprint(path) != *print) {
            return Ok(None);
        }
        let identity = Self::identity(&text("binary_hash")?, &text("metadata_hash")?);
        let Some(saved) = self.cache.load(&identity).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        Ok(Some(FileCatalog::restore(saved, text("build")?, sources)))
    }

    /// Saves the catalog, then its index; an index is never left naming a catalog that was not written.
    fn save(&self, folder: &Path, catalog: &FileCatalog) -> Result<(), String> {
        let saved = catalog.saved().map_err(|e| format!("{e:?}"))?;
        self.cache.save(&saved).map_err(|e| e.to_string())?;
        let sources: Vec<Value> = catalog
            .sources()
            .iter()
            .map(|(path, print)| {
                let (len, modified) = print
                    .and_then(|(len, time)| Some((len, u64::try_from(time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos()).ok()?)))
                    .unzip();
                json!({"path": path, "len": len, "modified": modified})
            })
            .collect();
        let index = json!({
            "version": INDEX_VERSION,
            "extraction_version": EXTRACTION_VERSION,
            "folder": folder,
            "binary_hash": catalog.binary_hash,
            "metadata_hash": catalog.metadata_hash,
            "build": catalog.build,
            "sources": sources,
        });
        eg_core::atomic::write(&self.directory.join(INDEX_FILE), serde_json::to_string_pretty(&index).unwrap_or_default().as_bytes()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalog read from `folder`, whose one source file is there.
    fn catalog(folder: &Path) -> FileCatalog {
        let file = folder.join("GameAssembly.dll");
        std::fs::write(&file, b"build one").unwrap();
        files::test_catalog(vec![(file.clone(), files::fingerprint(&file))])
    }

    #[test]
    fn a_saved_catalog_loads_back_while_its_files_are_unchanged() {
        let (game, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let store = Store::new(data.path().to_path_buf());
        assert_eq!(store.load(game.path()).unwrap(), None, "nothing saved yet");
        let read = catalog(game.path());
        store.save(game.path(), &read).unwrap();
        let loaded = store.load(game.path()).unwrap().expect("loads back");
        assert_eq!(loaded.saved().unwrap(), read.saved().unwrap());
        assert_eq!((loaded.build.as_str(), loaded.sources()), (read.build.as_str(), read.sources()));
        assert_eq!(store.load(data.path()).unwrap(), None, "another folder's game is not this one");
        std::fs::write(game.path().join("GameAssembly.dll"), b"build two, longer").unwrap();
        assert_eq!(store.load(game.path()).unwrap(), None, "a changed file is read again");
        std::fs::write(data.path().join(INDEX_FILE), b"not json").unwrap();
        assert!(store.load(game.path()).is_err(), "a damaged index is reported");
    }

    #[test]
    fn the_service_loads_a_saved_catalog_and_reports_a_folder_it_cannot_read() {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (game, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // The folder holds no game, so a catalog that comes back was loaded, not read.
        let read = catalog(game.path());
        Store::new(data.path().to_path_buf()).save(game.path(), &read).unwrap();
        let service = start(None, Some(data.path().to_path_buf()), None).unwrap();
        assert_eq!(service.current(), GameData::NoGame);
        let GameData::Ready(loaded) = runtime.block_on(service.for_folder(game.path())) else {
            panic!("the saved catalog loads");
        };
        assert_eq!(loaded.saved().unwrap(), read.saved().unwrap());
        assert_eq!(service.current(), GameData::Ready(loaded.clone()), "and is published");
        assert!(
            matches!(runtime.block_on(service.for_folder(game.path())), GameData::Ready(again) if Arc::ptr_eq(&again, &loaded)),
            "kept while current"
        );

        assert!(
            matches!(runtime.block_on(service.read_again(None)), GameData::Unavailable(_)),
            "Read again reads the files, which hold no game, instead of loading the saved catalog"
        );

        let elsewhere = data.path().join("no game here");
        assert!(matches!(runtime.block_on(service.for_folder(&elsewhere)), GameData::Unavailable(_)));
        assert!(matches!(service.current(), GameData::Unavailable(_)));
    }

    #[test]
    fn a_fixed_handle_answers_with_its_state() {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let handle = GameDataHandle::fixed(GameData::Unavailable("fixture".into()));
        assert_eq!(runtime.block_on(handle.for_folder(Path::new("anywhere"))), GameData::Unavailable("fixture".into()));
    }
}
