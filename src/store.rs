use crate::checked::NotesDir;
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Write},
    iter,
    path::{Path, PathBuf},
    time::SystemTime,
};

const TRASH_DIR: &str = ".trash";
const EXTENSION: &str = "md";
pub const TEMP_PREFIX: &str = ".~";
const PREVIEW_CHARS: usize = 200;

pub type NoteId = String;

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: NoteId,
    pub title: String,
    pub preview: String,
    pub modified: SystemTime,
    text: String,
}

impl Note {
    pub fn new(id: NoteId, text: String, modified: SystemTime) -> Self {
        Self {
            id,
            title: title_of(&text),
            preview: preview_of(&text),
            modified,
            text,
        }
    }

    pub fn matches(&self, terms: &[String]) -> bool {
        if terms.is_empty() {
            return true;
        }
        let text = self.text.to_lowercase();
        terms.iter().all(|term| text.contains(term.as_str()))
    }

    pub fn found_line(&self, terms: &[String]) -> Option<String> {
        if terms.is_empty() {
            return None;
        }
        content_lines(&self.text)
            .skip(1)
            .find(|line| {
                let line = line.to_lowercase();
                terms.iter().any(|term| line.contains(term.as_str()))
            })
            .map(|line| line.chars().take(PREVIEW_CHARS).collect())
    }
}

pub fn terms(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Saved {
    Written,
    Changed,
}

pub struct Store {
    dir: PathBuf,
    trash: PathBuf,
}

impl Store {
    pub fn open(dir: &NotesDir) -> io::Result<Self> {
        let dir = dir.to_path_buf();
        let trash = dir.join(TRASH_DIR);
        fs::create_dir_all(&trash)?;
        Ok(Self { dir, trash })
    }

    pub fn folders(&self) -> Vec<PathBuf> {
        vec![self.dir.clone(), self.trash.clone()]
    }

    pub fn list(&self) -> io::Result<Vec<Note>> {
        scan(&self.dir)
    }

    pub fn list_trash(&self) -> io::Result<Vec<Note>> {
        scan(&self.trash)
    }

    pub fn read(&self, id: &str) -> io::Result<String> {
        fs::read_to_string(self.dir.join(id))
    }

    pub fn read_trashed(&self, id: &str) -> io::Result<String> {
        fs::read_to_string(self.trash.join(id))
    }

    pub fn modified(&self, id: &str) -> io::Result<SystemTime> {
        fs::metadata(self.dir.join(id))?.modified()
    }

    pub fn create(&self, text: &str) -> io::Result<NoteId> {
        let stem = chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
        let temp = write_temp(&self.dir, &stem, text)?;
        move_to_free_name(&temp, &self.dir, &stem).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
    }

    /// Atomically replaces a note's contents with `text`, unless its file holds
    /// something other than `loaded`, the text this note had when it was last read
    /// or written. A crash mid-save leaves the previous version intact.
    ///
    /// A note deleted elsewhere is written again, because an edit is worth more
    /// than a deletion. One that already holds `text` is left as it is, date
    /// included.
    pub fn save(&self, id: &str, loaded: &str, text: &str) -> io::Result<Saved> {
        let mut already_there = false;
        let written = write_if(&self.dir, id, text, || {
            match fs::read(self.dir.join(id)) {
                Ok(held) if held == text.as_bytes() => {
                    already_there = true;
                    Ok(false)
                }
                // Bytes, so a file that is no longer text counts as changed.
                Ok(held) => Ok(held == loaded.as_bytes()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
                Err(error) => Err(error),
            }
        })?;
        Ok(if written || already_there {
            Saved::Written
        } else {
            Saved::Changed
        })
    }

    pub fn remove_unchanged(&self, id: &str, loaded: &str) -> io::Result<bool> {
        remove_if_holds(&self.dir.join(id), loaded)
    }

    pub fn trash(&self, id: &str) -> io::Result<NoteId> {
        move_to_free_name(&self.dir.join(id), &self.trash, stem_of(id))
    }

    pub fn restore(&self, id: &str) -> io::Result<NoteId> {
        move_to_free_name(&self.trash.join(id), &self.dir, stem_of(id))
    }

    pub fn purge(&self, id: &str, shown: &str) -> io::Result<bool> {
        remove_if_holds(&self.trash.join(id), shown)
    }

    pub fn empty_trash(&self) -> io::Result<()> {
        for note in self.list_trash()? {
            gone(fs::remove_file(self.trash.join(&note.id)))?;
        }
        Ok(())
    }
}

fn remove_if_holds(path: &Path, text: &str) -> io::Result<bool> {
    match fs::read(path) {
        Ok(held) if held != text.as_bytes() => Ok(false),
        Ok(_) => gone(fs::remove_file(path)).map(|()| true),
        Err(error) => gone(Err(error)).map(|()| true),
    }
}

fn gone(removed: io::Result<()>) -> io::Result<()> {
    match removed {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub fn write_atomically(dir: &Path, name: &str, text: &str) -> io::Result<()> {
    write_if(dir, name, text, || Ok(true)).map(|_| ())
}

/// The temporary file's name starts with `.~`, which Dropbox never syncs, and with
/// a dot, which keeps it out of the list of notes.
fn write_if(
    dir: &Path,
    name: &str,
    text: &str,
    may_replace: impl FnOnce() -> io::Result<bool>,
) -> io::Result<bool> {
    let temp = write_temp(dir, name, text)?;
    let result = may_replace().and_then(|may| {
        if may {
            replace(&temp, &dir.join(name))?;
        }
        Ok(may)
    });
    if !matches!(result, Ok(true)) {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_temp(dir: &Path, name: &str, text: &str) -> io::Result<PathBuf> {
    let temp = dir.join(temp_name(name));
    let written = (|| {
        let mut file = File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    match written {
        Ok(()) => Ok(temp),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "a save replaces the file it compared"
)]
fn replace(temp: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temp, target)
}

fn move_new(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::{fs, io::Errno};
    match fs::renameat_with(fs::CWD, from, fs::CWD, to, fs::RenameFlags::NOREPLACE) {
        Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => move_new_in_two(from, to),
        moved => Ok(moved?),
    }
}

/// For a folder whose file system can't refuse to replace, such as NFS.
#[expect(clippy::disallowed_methods, reason = "asked just before")]
fn move_new_in_two(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    fs::rename(from, to)
}

fn move_to_free_name(from: &Path, dir: &Path, stem: &str) -> io::Result<NoteId> {
    let numbered = (2..).map(|n| format!("{stem}-{n}.{EXTENSION}"));
    for id in iter::once(format!("{stem}.{EXTENSION}")).chain(numbered) {
        match move_new(from, &dir.join(&id)) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            moved => return moved.map(|()| id),
        }
    }
    unreachable!("an unbounded range always has another name")
}

fn stem_of(id: &str) -> &str {
    id.strip_suffix(".md").unwrap_or(id)
}

fn temp_name(name: &str) -> String {
    format!("{TEMP_PREFIX}{name}.tmp")
}

fn scan(dir: &Path) -> io::Result<Vec<Note>> {
    let mut notes = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(id) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if id.starts_with('.') || path.extension() != Some(OsStr::new(EXTENSION)) {
            continue;
        }
        let (Ok(text), Ok(metadata)) = (fs::read_to_string(&path), fs::metadata(&path)) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        notes.push(Note::new(id.to_string(), text, metadata.modified()?));
    }
    notes.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.id.cmp(&a.id)));
    Ok(notes)
}

fn title_of(text: &str) -> String {
    content_lines(text).next().unwrap_or_default().to_string()
}

fn preview_of(text: &str) -> String {
    content_lines(text)
        .nth(1)
        .unwrap_or_default()
        .chars()
        .take(PREVIEW_CHARS)
        .collect()
}

fn content_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !is_fence(line))
        .map(clean_line)
        .filter(|line| !line.is_empty())
}

pub(crate) fn is_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

fn clean_line(line: &str) -> &str {
    let mut s = line.trim().trim_start_matches(['#', '>', ' ', '\t']);
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(marker) {
            s = rest;
            break;
        }
    }
    if let Some((number, rest)) = s.split_once(". ")
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
    {
        s = rest;
    }
    for task in ["[ ] ", "[x] ", "[X] "] {
        if let Some(rest) = s.strip_prefix(task) {
            s = rest;
            break;
        }
    }
    let s = unwrapped(s.trim()).trim();
    if s.bytes().all(|b| matches!(b, b'-' | b'=' | b' ')) {
        return "";
    }
    s
}

fn unwrapped(s: &str) -> &str {
    let is_mark = |c: char| matches!(c, '*' | '_' | '`' | '~');
    let opening = &s[..s.len() - s.trim_start_matches(is_mark).len()];
    let closing = &s[s.trim_end_matches(is_mark).len()..];
    let inner = s.trim_matches(is_mark);
    if opening.chars().rev().eq(closing.chars()) && !inner.contains(is_mark) {
        inner
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::testing::QUIET;
    use std::{
        os::unix::fs::{MetadataExt, PermissionsExt},
        time::Duration,
    };

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&NotesDir::new(dir.path())).unwrap();
        (dir, store)
    }

    fn set_modified(path: &Path, secs: u64) {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    #[test]
    fn a_new_folder_has_no_notes_and_a_trash_folder() {
        let (dir, store) = store();
        assert!(store.list().unwrap().is_empty());
        assert!(dir.path().join(".trash").is_dir());
    }

    #[test]
    fn create_writes_the_exact_text_as_a_plain_md_file() {
        let (dir, store) = store();
        let id = store.create("# Héllo\n\nwörld").unwrap();
        assert!(id.ends_with(".md"));
        assert_eq!(
            fs::read_to_string(dir.path().join(&id)).unwrap(),
            "# Héllo\n\nwörld"
        );
        assert_eq!(store.read(&id).unwrap(), "# Héllo\n\nwörld");
    }

    #[test]
    fn new_notes_are_named_after_their_creation_time() {
        let (_dir, store) = store();
        let id = store.create("x").unwrap();
        let stem = id.strip_suffix(".md").unwrap();
        assert!(chrono::NaiveDateTime::parse_from_str(stem, "%Y-%m-%d %H-%M-%S").is_ok());
    }

    #[test]
    fn notes_from_earlier_versions_of_the_app_work_like_any_other() {
        let (dir, store) = store();
        let name = "2026-10-28 00-15-15.md";
        fs::write(dir.path().join(name), "# Older note\nbody").unwrap();
        fs::write(dir.path().join(".trash/2026-10-28 00-13-49.md"), "binned").unwrap();

        let notes = store.list().unwrap();
        assert_eq!(notes[0].id, name);
        assert_eq!(notes[0].title, "Older note");
        assert_eq!(store.list_trash().unwrap().len(), 1);

        assert_eq!(
            store.save(name, "# Older note\nbody", "edited").unwrap(),
            Saved::Written
        );
        assert_eq!(store.read(name).unwrap(), "edited");
        let trashed = store.trash(name).unwrap();
        assert_eq!(trashed, name, "keeps its name through the trash");
        assert_eq!(store.restore(&trashed).unwrap(), name);
    }

    #[test]
    fn notes_created_in_the_same_second_get_different_files() {
        let (_dir, store) = store();
        let ids: Vec<_> = (0..3)
            .map(|n| store.create(&format!("note {n}")).unwrap())
            .collect();
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
        assert_ne!(ids[0], ids[2]);
        assert_eq!(store.list().unwrap().len(), 3);
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name != ".trash")
            .collect();
        names.sort();
        names
    }

    #[test]
    fn saving_replaces_the_contents_and_leaves_no_temp_file() {
        let (dir, store) = store();
        let id = store.create("old").unwrap();
        assert_eq!(store.save(&id, "old", "new").unwrap(), Saved::Written);
        assert_eq!(store.read(&id).unwrap(), "new");
        assert_eq!(files(dir.path()), [id]);
    }

    #[test]
    fn a_failed_save_keeps_the_previous_contents_and_cleans_up() {
        let (dir, store) = store();
        let id = store.create("old").unwrap();
        if fs::metadata(dir.path()).unwrap().uid() != 0 {
            let set_mode = |mode| {
                fs::set_permissions(dir.path(), fs::Permissions::from_mode(mode)).unwrap();
            };
            set_mode(0o500);
            let saved = store.save(&id, "old", "new");
            set_mode(0o700);
            assert!(saved.is_err());
            assert_eq!(store.read(&id).unwrap(), "old");
            assert_eq!(files(dir.path()), [id.as_str()]);
        }

        fs::create_dir(dir.path().join("blocked.md")).unwrap();
        assert!(store.save("blocked.md", "", "new").is_err());
        assert!(write_atomically(dir.path(), "blocked.md", "new").is_err());
        assert_eq!(store.read(&id).unwrap(), "old");
        assert_eq!(files(dir.path()), [id, "blocked.md".to_string()]);
    }

    #[test]
    fn a_save_in_progress_has_a_name_dropbox_never_syncs() {
        use notify::{RecursiveMode, Watcher};
        let (dir, store) = store();
        let id = store.create("old").unwrap();
        let (sender, events) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender).unwrap();
        watcher
            .watch(dir.path(), RecursiveMode::NonRecursive)
            .unwrap();
        while events.recv_timeout(QUIET).is_ok() {}

        store.save(&id, "old", "new").unwrap();
        let names: Vec<_> = iter::from_fn(|| events.recv_timeout(Duration::from_secs(1)).ok())
            .flat_map(|event| event.unwrap().paths)
            .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();
        let never_synced = |name: &String| name.starts_with(".~");
        assert!(names.iter().any(never_synced), "{names:?}");
        assert!(
            names.iter().all(|name| *name == id || never_synced(name)),
            "{names:?}"
        );
    }

    #[test]
    fn saving_leaves_a_file_changed_elsewhere_alone() {
        let (dir, store) = store();
        let id = store.create("as loaded").unwrap();
        fs::write(dir.path().join(&id), "changed elsewhere").unwrap();

        assert_eq!(
            store.save(&id, "as loaded", "typed here").unwrap(),
            Saved::Changed
        );
        assert_eq!(store.read(&id).unwrap(), "changed elsewhere");
        assert_eq!(files(dir.path()), [id]);
    }

    #[test]
    fn saving_a_note_deleted_elsewhere_brings_it_back() {
        let (dir, store) = store();
        let id = store.create("as loaded").unwrap();
        fs::remove_file(dir.path().join(&id)).unwrap();

        assert_eq!(
            store.save(&id, "as loaded", "typed here").unwrap(),
            Saved::Written
        );
        assert_eq!(store.read(&id).unwrap(), "typed here");
    }

    #[test]
    fn saving_text_the_file_already_holds_keeps_its_date() {
        let (dir, store) = store();
        let id = store.create("as loaded").unwrap();
        fs::write(dir.path().join(&id), "the same on both").unwrap();
        set_modified(&dir.path().join(&id), 1000);

        assert_eq!(
            store.save(&id, "as loaded", "the same on both").unwrap(),
            Saved::Written
        );
        assert_eq!(
            store.modified(&id).unwrap(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1000)
        );
    }

    #[test]
    fn a_file_that_is_no_longer_text_is_never_overwritten() {
        let (dir, store) = store();
        let id = store.create("as loaded").unwrap();
        fs::write(dir.path().join(&id), [0xff, 0xfe, 0x00]).unwrap();

        assert_eq!(
            store.save(&id, "as loaded", "typed here").unwrap(),
            Saved::Changed
        );
        assert_eq!(fs::read(dir.path().join(&id)).unwrap(), [0xff, 0xfe, 0x00]);
    }

    #[test]
    fn an_emptied_note_is_removed_unless_it_changed_elsewhere() {
        let (dir, store) = store();
        let id = store.create("as loaded").unwrap();
        fs::write(dir.path().join(&id), "changed elsewhere").unwrap();
        assert!(!store.remove_unchanged(&id, "as loaded").unwrap());
        assert_eq!(store.read(&id).unwrap(), "changed elsewhere");

        assert!(store.remove_unchanged(&id, "changed elsewhere").unwrap());
        assert!(files(dir.path()).is_empty());
        assert!(
            store.remove_unchanged(&id, "changed elsewhere").unwrap(),
            "one already gone is as gone as was asked"
        );
    }

    #[test]
    fn list_is_newest_modified_first() {
        let (dir, store) = store();
        for (name, secs) in [("a.md", 100), ("b.md", 300), ("c.md", 200)] {
            fs::write(dir.path().join(name), name).unwrap();
            set_modified(&dir.path().join(name), secs);
        }
        let ids: Vec<_> = store.list().unwrap().into_iter().map(|n| n.id).collect();
        assert_eq!(ids, ["b.md", "c.md", "a.md"]);
    }

    #[test]
    fn list_ignores_dotfiles_other_extensions_folders_and_non_utf8_files() {
        let (dir, store) = store();
        fs::write(dir.path().join(".hidden.md"), "x").unwrap();
        fs::write(dir.path().join(temp_name("note.md")), "x").unwrap();
        fs::write(dir.path().join("image.png"), "x").unwrap();
        fs::create_dir(dir.path().join("folder.md")).unwrap();
        fs::write(dir.path().join("binary.md"), [0xff, 0xfe, 0x00]).unwrap();
        fs::write(dir.path().join("real.md"), "real").unwrap();
        let ids: Vec<_> = store.list().unwrap().into_iter().map(|n| n.id).collect();
        assert_eq!(ids, ["real.md"]);
        assert_eq!(
            fs::read(dir.path().join("binary.md")).unwrap(),
            [0xff, 0xfe, 0x00]
        );
    }

    #[test]
    fn trash_and_restore_round_trip_keeps_text_and_date() {
        let (dir, store) = store();
        let id = store.create("keep me").unwrap();
        set_modified(&dir.path().join(&id), 1000);
        let trashed = store.trash(&id).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert_eq!(store.list_trash().unwrap().len(), 1);
        assert_eq!(store.read_trashed(&trashed).unwrap(), "keep me");

        let restored = store.restore(&trashed).unwrap();
        assert!(store.list_trash().unwrap().is_empty());
        assert_eq!(store.read(&restored).unwrap(), "keep me");
        assert_eq!(
            store.list().unwrap()[0].modified,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1000)
        );
    }

    #[test]
    fn restoring_never_overwrites_a_note_with_the_same_name() {
        let (dir, store) = store();
        fs::write(dir.path().join(".trash/same.md"), "trashed").unwrap();
        fs::write(dir.path().join("same.md"), "current").unwrap();
        let restored = store.restore("same.md").unwrap();
        assert_ne!(restored, "same.md");
        assert_eq!(store.read("same.md").unwrap(), "current");
        assert_eq!(store.read(&restored).unwrap(), "trashed");
    }

    #[test]
    fn a_name_taken_in_the_meantime_is_never_replaced() {
        let (dir, _store) = store();
        let (from, taken) = (dir.path().join("from"), dir.path().join("taken.md"));
        fs::write(&from, "mine").unwrap();
        fs::write(&taken, "theirs").unwrap();
        for attempt in [move_new, move_new_in_two] {
            let error = attempt(&from, &taken).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read_to_string(&taken).unwrap(), "theirs");
        }
        assert_eq!(
            move_to_free_name(&from, dir.path(), "taken").unwrap(),
            "taken-2.md"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("taken-2.md")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn this_file_system_can_refuse_to_replace() {
        use rustix::{fs as raw, io::Errno};
        let (dir, _store) = store();
        let (from, taken) = (dir.path().join("from"), dir.path().join("taken"));
        fs::write(&from, "").unwrap();
        fs::write(&taken, "").unwrap();
        let moved = raw::renameat_with(
            raw::CWD,
            &from,
            raw::CWD,
            &taken,
            raw::RenameFlags::NOREPLACE,
        );
        assert_eq!(moved, Err(Errno::EXIST));
    }

    #[test]
    fn purging_leaves_a_note_that_holds_something_else() {
        let (dir, store) = store();
        fs::write(dir.path().join(".trash/one.md"), "theirs").unwrap();
        assert!(!store.purge("one.md", "shown").unwrap());
        assert_eq!(store.read_trashed("one.md").unwrap(), "theirs");
    }

    #[test]
    fn purge_and_empty_trash_only_touch_the_trash() {
        let (_dir, store) = store();
        let kept = store.create("kept").unwrap();
        for text in ["one", "two", "three"] {
            let id = store.create(text).unwrap();
            store.trash(&id).unwrap();
        }
        let first = store.list_trash().unwrap()[0].id.clone();
        let shown = store.read_trashed(&first).unwrap();
        assert!(store.purge(&first, &shown).unwrap());
        assert_eq!(store.list_trash().unwrap().len(), 2);
        store.empty_trash().unwrap();
        assert!(store.list_trash().unwrap().is_empty());
        assert_eq!(store.read(&kept).unwrap(), "kept");
    }

    #[test]
    fn purging_a_note_that_is_already_gone_is_not_a_failure() {
        let (dir, store) = store();
        fs::write(dir.path().join(".trash/one.md"), "one").unwrap();
        fs::write(dir.path().join(".trash/two.md"), "two").unwrap();
        fs::remove_file(dir.path().join(".trash/one.md")).unwrap();
        assert!(store.purge("one.md", "one").unwrap());
        assert_eq!(store.list_trash().unwrap().len(), 1);
    }

    #[test]
    fn titles_come_from_the_first_line_with_content() {
        for (text, title) in [
            ("Plain title\nsecond", "Plain title"),
            ("\n\n  \nAfter blanks", "After blanks"),
            ("# Heading", "Heading"),
            ("### Deep heading ", "Deep heading"),
            ("> Quoted", "Quoted"),
            ("- item", "item"),
            ("* item", "item"),
            ("12. numbered", "numbered"),
            ("- [ ] task", "task"),
            ("- [x] done", "done"),
            ("**bold**", "bold"),
            ("---\nAfter a rule", "After a rule"),
            ("Title\n=====", "Title"),
            ("#\nAfter an empty heading", "After an empty heading"),
            ("--help flag", "--help flag"),
            ("é accentué", "é accentué"),
            ("", ""),
            ("  \n \n", ""),
            ("```sh\nmake install\n```", "make install"),
            ("~~~\ncode\n~~~", "code"),
            ("~~struck~~", "struck"),
            ("_**both**_", "both"),
            ("***", ""),
            (
                "Ask about the `opening hours`",
                "Ask about the `opening hours`",
            ),
            ("**Note:** read this", "**Note:** read this"),
            ("*one* and *two*", "*one* and *two*"),
            ("__init__.py", "__init__.py"),
            ("snake_case_name", "snake_case_name"),
        ] {
            assert_eq!(note(text).title, title, "for {text:?}");
        }
    }

    #[test]
    fn previews_come_from_the_next_line_with_content() {
        for (text, preview) in [
            ("", ""),
            ("Title", ""),
            ("Title\n\n  \n", ""),
            ("Title\nSecond", "Second"),
            ("\nTitle\n  \nSecond line\nThird", "Second line"),
            ("# Title\n\n- milk\n- eggs", "milk"),
            ("Title\n---\nnext", "next"),
            ("Title\n=====\nnext", "next"),
            ("Install\n```sh\nmake install\n```", "make install"),
            ("Title\n- [x] done", "done"),
            ("Title\nsee `this`", "see `this`"),
        ] {
            assert_eq!(note(text).preview, preview, "for {text:?}");
        }
    }

    fn note(text: &str) -> Note {
        Note::new("a.md".into(), text.into(), SystemTime::UNIX_EPOCH)
    }

    #[test]
    fn a_note_is_found_by_every_word_in_any_order_and_case() {
        let note = note("# Courses du Samedi\n\n- Pain\n- café moulu");
        for (query, found) in [
            ("", true),
            ("   ", true),
            ("pain", true),
            ("PAIN", true),
            ("samedi courses", true),
            ("café pain", true),
            ("CAFÉ", true),
            ("moul", true),
            ("pain thé", false),
            ("dimanche", false),
        ] {
            assert_eq!(note.matches(&terms(query)), found, "for {query:?}");
        }
    }

    #[test]
    fn the_line_a_note_was_found_by_is_one_under_its_title() {
        let note = note("# Plumber\n\nCall on Monday\n- about the **tap**\n- and the Tap again");
        assert_eq!(
            note.found_line(&terms("TAP")).as_deref(),
            Some("about the **tap**")
        );
        assert_eq!(
            note.found_line(&terms("plumber monday")).as_deref(),
            Some("Call on Monday"),
            "any of the words will do"
        );
        assert_eq!(note.found_line(&terms("plumber")), None, "only the title");
        assert_eq!(note.found_line(&terms("")), None, "nothing looked for");
    }

    #[test]
    fn a_long_found_line_is_cut_between_characters() {
        let note = note(&format!("Title\n{}", "é".repeat(PREVIEW_CHARS * 2)));
        assert_eq!(
            note.found_line(&terms("é")),
            Some("é".repeat(PREVIEW_CHARS))
        );
    }

    #[test]
    fn a_long_preview_is_cut_between_characters() {
        let note = note(&format!("Title\n{}", "é".repeat(PREVIEW_CHARS * 2)));
        assert_eq!(note.preview, "é".repeat(PREVIEW_CHARS));
    }

    #[test]
    fn the_list_carries_each_notes_preview() {
        let (dir, store) = store();
        fs::write(dir.path().join("a.md"), "# Groceries\n\n- milk").unwrap();
        let note = &store.list().unwrap()[0];
        assert_eq!(note.title, "Groceries");
        assert_eq!(note.preview, "milk");
    }
}
