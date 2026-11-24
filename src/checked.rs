//! Settings from a file, once they are fit to use. A number or a path only becomes one
//! of these here, so nothing else in the app can hold one that was never checked.

use std::{
    ops::Deref,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdleDelay(Duration);

impl IdleDelay {
    /// Shorter than this, a note would be gone between two keys.
    pub const MIN: Self = Self(Duration::from_secs(1));
    /// A year. Longer can't be told from never, and a file can hold numbers too large
    /// for a timer to count to.
    pub const MAX: Self = Self(Duration::from_secs(365 * 24 * 60 * 60));

    /// `None` is never, for 0 minutes or fewer.
    pub fn from_minutes(minutes: f64) -> Option<Self> {
        (minutes > 0.).then(|| {
            Duration::try_from_secs_f64(minutes * 60.)
                .map_or(Self::MAX, Self)
                .clamp(Self::MIN, Self::MAX)
        })
    }

    pub fn get(self) -> Duration {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextPercent(u32);

impl TextPercent {
    pub const MIN: Self = Self(80);
    pub const MAX: Self = Self(200);
    const STEP: u32 = 10;

    /// The nearest offered size: whole steps within the limits.
    pub fn nearest(percent: u32) -> Self {
        let steps = percent.saturating_add(Self::STEP / 2) / Self::STEP;
        Self((steps * Self::STEP).clamp(Self::MIN.0, Self::MAX.0))
    }

    pub fn larger(self) -> Self {
        Self::nearest(self.0 + Self::STEP)
    }

    pub fn smaller(self) -> Self {
        Self::nearest(self.0 - Self::STEP)
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for TextPercent {
    fn default() -> Self {
        Self(100)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextScale(f32);

impl TextScale {
    pub fn new(scale: f32) -> Option<Self> {
        (scale > 0. && scale.is_finite()).then_some(Self(scale))
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

impl Default for TextScale {
    fn default() -> Self {
        Self(1.)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NotesDir(PathBuf);

impl NotesDir {
    /// Joining leaves an absolute folder as it is. Any other starts from the root, and
    /// not from whichever folder the app was launched from.
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self(Path::new("/").join(dir))
    }
}

impl Deref for NotesDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notes_folder_is_never_relative() {
        for dir in ["", "~", "Notes", "../Notes"] {
            assert!(NotesDir::new(dir).is_absolute(), "{dir}");
        }
        assert_eq!(&*NotesDir::new("/srv/notes"), Path::new("/srv/notes"));
    }
}
