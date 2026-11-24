use crate::store::TEMP_PREFIX;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Adapted from gpui-omarchy's palette watcher, which is private to that crate.
pub struct FolderWatcher {
    watcher: RecommendedWatcher,
    armed: BTreeMap<PathBuf, Identity>,
    parents: bool,
}

/// A folder can be deleted and recreated at the same path before its events are read,
/// and the old watch then refers to a folder that's gone.
type Identity = (u64, u64);

impl FolderWatcher {
    /// With `parents`, a folder's parent is watched too, which sees it being
    /// replaced, and so is the nearest ancestor of a missing one, which sees it
    /// appear. That suits small folders that come and go. It doesn't suit the notes:
    /// their parent can be a whole synced folder, which a Mac reports on in full.
    pub fn new(
        folders: &[PathBuf],
        parents: bool,
    ) -> notify::Result<(Self, async_channel::Receiver<()>)> {
        let (sender, receiver) = async_channel::bounded(1);
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if !matches!(&event, Ok(event) if is_noise(event)) {
                let _ = sender.try_send(());
            }
        })?;
        let mut watcher = Self {
            watcher,
            armed: BTreeMap::new(),
            parents,
        };
        watcher.arm(folders);
        Ok((watcher, receiver))
    }

    pub fn arm(&mut self, folders: &[PathBuf]) {
        let mut wanted = BTreeMap::new();
        for folder in folders {
            if !self.parents {
                if folder.is_dir() {
                    add_folder(folder, &mut wanted);
                }
                continue;
            }
            let Some(nearest) = folder.ancestors().find(|path| path.is_dir()) else {
                continue;
            };
            add_folder(nearest, &mut wanted);
            if nearest == folder
                && let Some(parent) = nearest.parent()
            {
                add_folder(parent, &mut wanted);
            }
        }
        self.armed.retain(|path, identity| {
            let unchanged = wanted.get(path) == Some(identity);
            if !unchanged {
                let _ = self.watcher.unwatch(path);
            }
            unchanged
        });
        for (path, identity) in wanted {
            if !self.armed.contains_key(&path) {
                match self.watcher.watch(&path, RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        self.armed.insert(path, identity);
                    }
                    Err(error) => eprintln!("brouyon: can't watch {}: {error}", path.display()),
                }
            }
        }
    }
}

fn is_noise(event: &notify::Event) -> bool {
    let saving = |path: &PathBuf| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(TEMP_PREFIX))
    };
    event.kind.is_access() || (!event.paths.is_empty() && event.paths.iter().all(saving))
}

fn add_folder(path: &Path, folders: &mut BTreeMap<PathBuf, Identity>) {
    if let Ok(path) = fs::canonicalize(path)
        && let Ok(metadata) = fs::metadata(&path)
    {
        folders.insert(path, (metadata.dev(), metadata.ino()));
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::{
        thread,
        time::{Duration, Instant},
    };

    pub(crate) const QUIET: Duration = Duration::from_millis(100);

    pub(crate) fn wait_for_event(events: &async_channel::Receiver<()>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while events.try_recv().is_err() {
            assert!(Instant::now() < deadline, "no event arrived");
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn settle(events: &async_channel::Receiver<()>) {
        thread::sleep(QUIET);
        while events.try_recv().is_ok() {}
    }

    pub(crate) fn nothing_arrives(events: &async_channel::Receiver<()>) {
        thread::sleep(QUIET);
        assert!(events.try_recv().is_err(), "an event arrived");
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};
    use crate::{checked::NotesDir, store::Store};

    fn notes() -> (tempfile::TempDir, Store, PathBuf) {
        let outside = tempfile::tempdir().unwrap();
        let dir = outside.path().join("notes");
        let store = Store::open(&NotesDir::new(&dir)).unwrap();
        (outside, store, dir)
    }

    fn watch(store: &Store) -> (FolderWatcher, async_channel::Receiver<()>) {
        let (watcher, events) = FolderWatcher::new(&store.folders(), false).unwrap();
        settle(&events);
        (watcher, events)
    }

    #[test]
    fn a_note_renamed_into_place_is_noticed() {
        let (outside, store, dir) = notes();
        let arriving = outside.path().join("arriving");
        fs::write(&arriving, "from another device").unwrap();
        let (_watcher, events) = watch(&store);

        fs::rename(&arriving, dir.join("note.md")).unwrap();
        wait_for_event(&events);
    }

    #[test]
    fn a_change_in_the_trash_is_noticed() {
        let (_outside, store, dir) = notes();
        let (_watcher, events) = watch(&store);

        fs::write(dir.join(".trash/note.md"), "trashed elsewhere").unwrap();
        wait_for_event(&events);
    }

    #[test]
    fn a_save_in_progress_is_not_a_change_until_it_is_renamed() {
        let (_outside, store, dir) = notes();
        let (_watcher, events) = watch(&store);

        let temporary = dir.join(format!("{TEMP_PREFIX}note.md.tmp"));
        fs::write(&temporary, "being saved").unwrap();
        nothing_arrives(&events);

        fs::rename(&temporary, dir.join("note.md")).unwrap();
        wait_for_event(&events);
    }

    #[test]
    fn only_the_notes_and_the_trash_are_watched_not_what_is_around_them() {
        let (outside, store, dir) = notes();
        let (_watcher, events) = watch(&store);

        fs::write(outside.path().join("unrelated"), "x").unwrap();
        nothing_arrives(&events);

        fs::write(dir.join("note.md"), "x").unwrap();
        wait_for_event(&events);
    }

    #[test]
    fn reading_the_notes_does_not_set_off_the_watcher() {
        let (_outside, store, _dir) = notes();
        store.create("a note").unwrap();
        let (_watcher, events) = watch(&store);

        store.list().unwrap();
        nothing_arrives(&events);
    }
}
