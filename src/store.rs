//! Notes as plain files. Each note is one UTF-8 `.md` file in a single folder,
//! trashed notes move to its `.trash` subfolder, and no other metadata exists.

use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

const TRASH_DIR: &str = ".trash";
const EXTENSION: &str = "md";
/// More than any list is wide, and a long line costs nothing to keep out of it.
const PREVIEW_CHARS: usize = 200;

/// A note's file name, such as `2026-11-06 09-42-15.md`. Set at creation and never
/// changed, so editing the title does not rename the file.
pub type NoteId = String;

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: NoteId,
    pub title: String,
    /// The line under the title in the list. Empty for a note of one line.
    pub preview: String,
    pub modified: SystemTime,
}

impl Note {
    pub fn new(id: NoteId, text: &str, modified: SystemTime) -> Self {
        Self {
            id,
            title: title_of(text),
            preview: preview_of(text),
            modified,
        }
    }
}

pub struct Store {
    dir: PathBuf,
    trash: PathBuf,
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        let trash = dir.join(TRASH_DIR);
        fs::create_dir_all(&trash)?;
        Ok(Self { dir, trash })
    }

    /// Notes, most recently modified first.
    pub fn list(&self) -> io::Result<Vec<Note>> {
        scan(&self.dir)
    }

    /// Trashed notes, most recently modified first.
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

    /// Creates a new note file holding `text` and returns its id.
    pub fn create(&self, text: &str) -> io::Result<NoteId> {
        let id = free_name(
            &self.dir,
            &chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string(),
        );
        self.write(&id, text)?;
        Ok(id)
    }

    /// Atomically replaces a note's contents: a crash mid-save leaves the
    /// previous version intact.
    pub fn write(&self, id: &str, text: &str) -> io::Result<()> {
        write_atomically(&self.dir, id, text)
    }

    /// Deletes a note's file. Only for notes that have become empty, which have
    /// nothing worth keeping in the trash.
    pub fn remove(&self, id: &str) -> io::Result<()> {
        fs::remove_file(self.dir.join(id))
    }

    pub fn trash(&self, id: &str) -> io::Result<NoteId> {
        let target = free_name(&self.trash, id.strip_suffix(".md").unwrap_or(id));
        fs::rename(self.dir.join(id), self.trash.join(&target))?;
        Ok(target)
    }

    pub fn restore(&self, id: &str) -> io::Result<NoteId> {
        let target = free_name(&self.dir, id.strip_suffix(".md").unwrap_or(id));
        fs::rename(self.trash.join(id), self.dir.join(&target))?;
        Ok(target)
    }

    /// Permanently deletes one trashed note. One that something else already
    /// deleted is as gone as was asked.
    pub fn purge(&self, id: &str) -> io::Result<()> {
        match fs::remove_file(self.trash.join(id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Permanently deletes every trashed note.
    pub fn empty_trash(&self) -> io::Result<()> {
        for note in self.list_trash()? {
            self.purge(&note.id)?;
        }
        Ok(())
    }
}

/// Replaces `dir/name` all at once, through a temporary file beside it.
pub fn write_atomically(dir: &Path, name: &str, text: &str) -> io::Result<()> {
    let temp = dir.join(format!(".{name}.tmp"));
    let result = (|| {
        let mut file = File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, dir.join(name))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// A file name free in `dir`, so moving or creating a note never overwrites one.
fn free_name(dir: &Path, stem: &str) -> NoteId {
    let free = |id: &str| !dir.join(id).exists();
    let id = format!("{stem}.{EXTENSION}");
    if free(&id) {
        return id;
    }
    (2..)
        .map(|n| format!("{stem}-{n}.{EXTENSION}"))
        .find(|id| free(id))
        .expect("an unbounded range always finds a free name")
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
        // A file that isn't UTF-8 stays unlisted, so autosave can never touch it.
        let (Ok(text), Ok(metadata)) = (fs::read_to_string(&path), fs::metadata(&path)) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        notes.push(Note::new(id.to_string(), &text, metadata.modified()?));
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

/// The lines with content, without their markdown markers. A code fence is no line
/// of content, whether or not it names a language, but the code inside it is.
fn content_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !is_fence(line))
        .map(clean_line)
        .filter(|line| !line.is_empty())
}

fn is_fence(line: &str) -> bool {
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
    // Horizontal rules and setext underlines carry no title.
    if s.bytes().all(|b| matches!(b, b'-' | b'=' | b' ')) {
        return "";
    }
    s
}

/// Without the emphasis around a whole line, such as `**this**`. Marks around only
/// part of a line stay, or `see `this`` would lose its last backtick.
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
    use std::time::Duration;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
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

        store.write(name, "edited").unwrap();
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

    #[test]
    fn write_replaces_the_contents_and_leaves_no_temp_file() {
        let (dir, store) = store();
        let id = store.create("old").unwrap();
        store.write(&id, "new").unwrap();
        assert_eq!(store.read(&id).unwrap(), "new");
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|name| name != ".trash")
            .collect();
        assert_eq!(entries, vec![id]);
    }

    #[test]
    fn a_failed_write_keeps_the_previous_contents_and_cleans_up() {
        let (dir, store) = store();
        let id = store.create("old").unwrap();
        // A directory in the way makes the final rename fail.
        fs::create_dir(dir.path().join("blocked.md")).unwrap();
        assert!(store.write("blocked.md", "new").is_err());
        assert_eq!(store.read(&id).unwrap(), "old");
        assert!(!dir.path().join(".blocked.md.tmp").exists());
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
        fs::write(dir.path().join(".note.md.tmp"), "x").unwrap();
        fs::write(dir.path().join("image.png"), "x").unwrap();
        fs::create_dir(dir.path().join("folder.md")).unwrap();
        fs::write(dir.path().join("binary.md"), [0xff, 0xfe, 0x00]).unwrap();
        fs::write(dir.path().join("real.md"), "real").unwrap();
        let ids: Vec<_> = store.list().unwrap().into_iter().map(|n| n.id).collect();
        assert_eq!(ids, ["real.md"]);
        // The skipped file is left exactly as it was.
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
    fn purge_and_empty_trash_only_touch_the_trash() {
        let (_dir, store) = store();
        let kept = store.create("kept").unwrap();
        for text in ["one", "two", "three"] {
            let id = store.create(text).unwrap();
            store.trash(&id).unwrap();
        }
        let first = store.list_trash().unwrap()[0].id.clone();
        store.purge(&first).unwrap();
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
        store.purge("one.md").unwrap();
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
            assert_eq!(title_of(text), title, "for {text:?}");
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
            assert_eq!(preview_of(text), preview, "for {text:?}");
        }
    }

    #[test]
    fn a_long_preview_is_cut_between_characters() {
        let text = format!("Title\n{}", "é".repeat(PREVIEW_CHARS * 2));
        assert_eq!(preview_of(&text), "é".repeat(PREVIEW_CHARS));
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
