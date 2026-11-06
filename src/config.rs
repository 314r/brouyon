//! User settings in `config.toml`.

use crate::store::write_atomically;
use serde::Deserialize;
use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};
use toml_edit::DocumentMut;

const DEFAULT_IDLE_MINUTES: f64 = 5.;
/// The size gpui-omarchy's metrics assume, matching Omarchy's default text size.
const BASE_REM_SIZE: f32 = 16.;

pub const MIN_TEXT_PERCENT: u32 = 80;
pub const MAX_TEXT_PERCENT: u32 = 200;
pub const TEXT_PERCENT_STEP: u32 = 10;
pub const DEFAULT_TEXT_PERCENT: u32 = 100;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follows the desktop's theme and its changes.
    #[default]
    Omarchy,
    Light,
    Dark,
}

impl ThemeMode {
    fn key(self) -> &'static str {
        match self {
            ThemeMode::Omarchy => "omarchy",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// The file these settings are saved to.
    path: PathBuf,
    pub default_notes_dir: PathBuf,
    pub notes_dir: PathBuf,
    /// How long without activity before moving to a blank note. `None` disables it.
    pub idle: Option<Duration>,
    pub theme: ThemeMode,
    /// Scales the desktop's text size throughout the app.
    pub text_percent: u32,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    idle_minutes: Option<f64>,
    notes_dir: Option<String>,
    theme: Option<ThemeMode>,
    text_percent: Option<u32>,
}

impl Config {
    /// Reads `$XDG_CONFIG_HOME/brouyon/config.toml`. Defaults apply when it is
    /// missing or invalid, and what's wrong with an invalid one comes back to be shown.
    pub fn load() -> (Self, Option<String>) {
        let home = home_dir();
        let path = xdg_dir("XDG_CONFIG_HOME", ".config", &home).join("brouyon/config.toml");
        let data_home = xdg_dir("XDG_DATA_HOME", ".local/share", &home);
        let (file, problem) = read_file_config(&path);
        (Self::resolve(file, path, &data_home, &home), problem)
    }

    fn resolve(file: FileConfig, path: PathBuf, data_home: &Path, home: &Path) -> Self {
        let default_notes_dir = data_home.join("brouyon");
        let notes_dir = file
            .notes_dir
            .map(|dir| match dir.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(dir),
            })
            .unwrap_or_else(|| default_notes_dir.clone());
        let minutes = file
            .idle_minutes
            .filter(|minutes| minutes.is_finite() && *minutes >= 0.)
            .unwrap_or(DEFAULT_IDLE_MINUTES);
        Self {
            path,
            default_notes_dir,
            notes_dir,
            idle: (minutes > 0.).then(|| Duration::from_secs_f64(minutes * 60.)),
            theme: file.theme.unwrap_or_default(),
            text_percent: normalized_text_percent(
                file.text_percent.unwrap_or(DEFAULT_TEXT_PERCENT),
            ),
        }
    }

    /// Settings for a test, saved inside `notes_dir` under a folder that notes
    /// never list, so nothing reaches the real configuration.
    #[cfg(test)]
    pub fn for_test(notes_dir: &Path) -> Self {
        Self {
            path: notes_dir.join(".settings/config.toml"),
            default_notes_dir: notes_dir.to_owned(),
            notes_dir: notes_dir.to_owned(),
            idle: Some(Duration::from_secs(300)),
            theme: ThemeMode::default(),
            text_percent: DEFAULT_TEXT_PERCENT,
        }
    }

    #[cfg(test)]
    pub fn saved_text(&self) -> String {
        fs::read_to_string(&self.path).unwrap_or_default()
    }

    /// The window's rem size for the desktop's text size and the chosen scale.
    pub fn rem_px(&self, desktop_text_scale: f32) -> f32 {
        BASE_REM_SIZE * desktop_text_scale * self.text_percent as f32 / 100.
    }

    /// Each setter applies for this session even when saving fails, and returns
    /// that failure for the app to report.
    pub fn set_theme(&mut self, theme: ThemeMode) -> io::Result<()> {
        self.theme = theme;
        self.save("theme", Some(theme.key().into()))
    }

    pub fn set_text_percent(&mut self, percent: u32) -> io::Result<()> {
        self.text_percent = normalized_text_percent(percent);
        self.save("text_percent", Some(i64::from(self.text_percent).into()))
    }

    pub fn set_idle(&mut self, idle: Option<Duration>) -> io::Result<()> {
        self.idle = idle;
        let minutes = idle.map_or(0., |idle| idle.as_secs_f64() / 60.);
        let value = if minutes.fract() == 0. {
            (minutes as i64).into()
        } else {
            minutes.into()
        };
        self.save("idle_minutes", Some(value))
    }

    /// `None` goes back to the default folder.
    pub fn set_notes_dir(&mut self, dir: Option<PathBuf>) -> io::Result<()> {
        let value = match &dir {
            Some(dir) => Some(
                dir.to_str()
                    .ok_or_else(|| invalid(&self.path, "the folder's name isn't UTF-8"))?
                    .into(),
            ),
            None => None,
        };
        self.notes_dir = dir.unwrap_or_else(|| self.default_notes_dir.clone());
        self.save("notes_dir", value)
    }

    /// Changes one key and leaves the rest of the file as it is, comments
    /// included. A file the app couldn't read back is reported and never replaced.
    fn save(&self, key: &str, value: Option<toml_edit::Value>) -> io::Result<()> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let mut document: DocumentMut = contents
            .parse()
            .map_err(|error| invalid(&self.path, error))?;
        match value {
            Some(value) => document[key] = toml_edit::Item::Value(value),
            None => {
                document.remove(key);
            }
        }
        let updated = document.to_string();
        toml::from_str::<FileConfig>(&updated).map_err(|error| invalid(&self.path, error))?;

        let dir = self.path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        let name = self.path.file_name().and_then(|name| name.to_str());
        write_atomically(dir, name.unwrap_or("config.toml"), &updated)
    }
}

/// The settings in `path`, or the defaults and what went wrong. A missing file is fine.
fn read_file_config(path: &Path) -> (FileConfig, Option<String>) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return (FileConfig::default(), Some(unreadable(path, error))),
    };
    match toml::from_str(&contents) {
        Ok(file) => (file, None),
        Err(error) => (
            FileConfig::default(),
            Some(unreadable(path, error.message())),
        ),
    }
}

fn unreadable(path: &Path, error: impl fmt::Display) -> String {
    format!(
        "Can't read {}: {error}. Using the default settings.",
        path.display()
    )
}

fn invalid(path: &Path, error: impl fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{}: {error}", path.display()),
    )
}

/// The nearest offered size: whole steps within the limits.
pub fn normalized_text_percent(percent: u32) -> u32 {
    let steps = (percent + TEXT_PERCENT_STEP / 2) / TEXT_PERCENT_STEP;
    (steps * TEXT_PERCENT_STEP).clamp(MIN_TEXT_PERCENT, MAX_TEXT_PERCENT)
}

pub(crate) fn home_dir() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub(crate) fn xdg_dir(variable: &str, fallback: &str, home: &Path) -> PathBuf {
    env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(fallback))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(toml_text: &str) -> Config {
        let file: FileConfig = toml::from_str(toml_text).unwrap();
        Config::resolve(
            file,
            "/config/config.toml".into(),
            Path::new("/data"),
            Path::new("/home/me"),
        )
    }

    /// Settings saved to a real file in a temporary folder.
    fn in_temp_dir(contents: Option<&str>) -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brouyon/config.toml");
        if let Some(contents) = contents {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
        }
        let config = Config::resolve(FileConfig::default(), path, dir.path(), dir.path());
        (dir, config)
    }

    fn saved(config: &Config) -> String {
        fs::read_to_string(&config.path).unwrap()
    }

    /// What the app would read from the file at its next launch.
    fn reloaded(config: &Config) -> Config {
        let file = toml::from_str(&saved(config)).unwrap();
        Config::resolve(
            file,
            config.path.clone(),
            config.default_notes_dir.parent().unwrap(),
            config.default_notes_dir.parent().unwrap(),
        )
    }

    #[test]
    fn defaults_use_the_data_folder_five_minutes_and_the_desktop_theme() {
        let config = resolve("");
        assert_eq!(config.notes_dir, PathBuf::from("/data/brouyon"));
        assert_eq!(config.default_notes_dir, PathBuf::from("/data/brouyon"));
        assert_eq!(config.idle, Some(Duration::from_secs(300)));
        assert_eq!(config.theme, ThemeMode::Omarchy);
        assert_eq!(config.text_percent, 100);
    }

    #[test]
    fn idle_minutes_accepts_fractions_and_zero_disables_it() {
        assert_eq!(
            resolve("idle_minutes = 0.5").idle,
            Some(Duration::from_secs(30))
        );
        assert_eq!(resolve("idle_minutes = 0").idle, None);
    }

    #[test]
    fn a_negative_idle_delay_falls_back_to_the_default() {
        assert_eq!(
            resolve("idle_minutes = -3.0").idle,
            Some(Duration::from_secs(300))
        );
    }

    #[test]
    fn notes_dir_expands_a_leading_tilde() {
        assert_eq!(
            resolve(r#"notes_dir = "~/Notes""#).notes_dir,
            PathBuf::from("/home/me/Notes")
        );
        assert_eq!(
            resolve(r#"notes_dir = "/srv/notes""#).notes_dir,
            PathBuf::from("/srv/notes")
        );
    }

    #[test]
    fn invalid_config_is_rejected_rather_than_half_applied() {
        assert!(toml::from_str::<FileConfig>("idle_minutes = \"soon\"").is_err());
        assert!(toml::from_str::<FileConfig>("unknown_key = 1").is_err());
        assert!(toml::from_str::<FileConfig>("theme = \"blue\"").is_err());
    }

    #[test]
    fn text_size_snaps_to_whole_steps_within_the_limits() {
        assert_eq!(resolve("text_percent = 134").text_percent, 130);
        assert_eq!(resolve("text_percent = 135").text_percent, 140);
        assert_eq!(resolve("text_percent = 10").text_percent, 80);
        assert_eq!(resolve("text_percent = 900").text_percent, 200);
    }

    #[test]
    fn rem_size_combines_the_desktop_size_and_the_chosen_scale() {
        let config = resolve("text_percent = 150");
        assert_eq!(config.rem_px(1.), 24.);
        assert_eq!(config.rem_px(2.), 48.);
    }

    #[test]
    fn a_missing_settings_file_is_not_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        let (_, problem) = read_file_config(&dir.path().join("config.toml"));
        assert_eq!(problem, None);
    }

    #[test]
    fn an_invalid_settings_file_gives_the_defaults_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "idle_minutes = 7\nunknown_key = 1\n").unwrap();

        let (file, problem) = read_file_config(&path);
        assert_eq!(file.idle_minutes, None, "not half applied");
        let problem = problem.unwrap();
        assert!(problem.contains(path.to_str().unwrap()), "{problem}");
        assert!(problem.contains("unknown_key"), "{problem}");
        assert!(problem.contains("default settings"), "{problem}");
    }

    #[test]
    fn a_settings_file_that_cannot_be_opened_is_a_problem_too() {
        let dir = tempfile::tempdir().unwrap();
        // A folder where the file should be fails with something other than "not found".
        let (_, problem) = read_file_config(dir.path());
        assert!(problem.unwrap().contains("default settings"));
    }

    #[test]
    fn saving_creates_the_file_and_every_setting_survives_a_restart() {
        let (_dir, mut config) = in_temp_dir(None);
        config.set_theme(ThemeMode::Light).unwrap();
        config.set_text_percent(120).unwrap();
        config.set_idle(Some(Duration::from_secs(600))).unwrap();
        config.set_notes_dir(Some("/srv/notes".into())).unwrap();
        assert_eq!(reloaded(&config), config);
        assert_eq!(config.theme, ThemeMode::Light);
        assert_eq!(config.text_percent, 120);
        assert_eq!(config.idle, Some(Duration::from_secs(600)));
        assert_eq!(config.notes_dir, PathBuf::from("/srv/notes"));
    }

    #[test]
    fn idle_minutes_are_written_as_people_would_write_them() {
        let (_dir, mut config) = in_temp_dir(None);
        config.set_idle(Some(Duration::from_secs(600))).unwrap();
        assert_eq!(saved(&config), "idle_minutes = 10\n");
        config.set_idle(None).unwrap();
        assert_eq!(saved(&config), "idle_minutes = 0\n");
        config.set_idle(Some(Duration::from_secs(90))).unwrap();
        assert_eq!(saved(&config), "idle_minutes = 1.5\n");
        assert_eq!(reloaded(&config).idle, Some(Duration::from_secs(90)));
    }

    #[test]
    fn the_default_folder_is_stored_by_having_no_key() {
        let (_dir, mut config) = in_temp_dir(None);
        config.set_notes_dir(Some("/srv/notes".into())).unwrap();
        assert!(saved(&config).contains("notes_dir"));
        config.set_notes_dir(None).unwrap();
        assert!(!saved(&config).contains("notes_dir"));
        assert_eq!(config.notes_dir, config.default_notes_dir);
    }

    #[test]
    fn saving_keeps_comments_and_other_settings() {
        let (_dir, mut config) = in_temp_dir(Some(
            "# my setup\nnotes_dir = \"~/Notes\" # synced\nidle_minutes = 5\n",
        ));
        config.set_theme(ThemeMode::Dark).unwrap();
        config.set_idle(Some(Duration::from_secs(60))).unwrap();
        let text = saved(&config);
        assert!(text.contains("# my setup"), "{text}");
        assert!(text.contains("notes_dir = \"~/Notes\" # synced"), "{text}");
        assert!(text.contains("theme = \"dark\""), "{text}");
        assert!(text.contains("idle_minutes = 1"), "{text}");
    }

    #[test]
    fn a_file_the_app_cannot_read_is_never_replaced() {
        for broken in ["this is not toml =", "unknown_key = 1\n"] {
            let (_dir, mut config) = in_temp_dir(Some(broken));
            let error = config.set_theme(ThemeMode::Dark).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(saved(&config), broken, "the file is untouched");
            assert_eq!(config.theme, ThemeMode::Dark, "still applies this session");
        }
    }

    #[test]
    fn saving_leaves_no_temporary_file() {
        let (_dir, mut config) = in_temp_dir(None);
        config.set_theme(ThemeMode::Dark).unwrap();
        let names: Vec<_> = fs::read_dir(config.path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.toml"]);
    }
}
