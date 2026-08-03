use std::{borrow::Cow, collections::BTreeMap, env};

use crate::error::Result;

pub const PROJECT_NAME: &str = env!("CARGO_CRATE_NAME");

#[cfg_attr(feature = "full-config", derive(serde::Deserialize))]
#[derive(Debug, Default, PartialEq, Eq)]
pub enum DirStyle {
    #[default]
    Plain,

    #[cfg_attr(feature = "full-config", serde(alias = "heavy"))]
    Thick,
}

#[cfg_attr(feature = "full-config", derive(serde::Deserialize))]
#[derive(Debug, Default)]
pub struct ThemeSpec {
    pub dirs: Vec<String>,
    pub files: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_color_shift() -> f32 {
    0.2
}

#[cfg_attr(feature = "full-config", derive(serde::Deserialize))]
#[derive(Debug, Default)]
pub struct Config {
    /// Render for a darker background
    #[cfg_attr(feature = "full-config", serde(default = "default_true"))]
    pub dark_mode: bool,

    /// Name of theme for color gradients to apply to files and directories
    #[cfg_attr(feature = "full-config", serde(default))]
    pub colors: Option<String>,

    /// Apply lightening or darkening depending on dark_mode
    #[cfg_attr(feature = "full-config", serde(default = "default_color_shift"))]
    pub color_shift: f32,

    /// Render directories with heavy lines
    #[cfg_attr(feature = "full-config", serde(default))]
    pub dir_style: DirStyle,

    #[cfg_attr(feature = "full-config", serde(default))]
    pub themes: BTreeMap<String, ThemeSpec>,
}

#[derive(Debug, Default)]
struct EnvWrap(BTreeMap<String, String>);

impl EnvWrap {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|it| it.as_str())
    }

    pub fn one_of<S: AsRef<str>>(&self, keys: impl IntoIterator<Item = S>) -> Option<String> {
        keys.into_iter()
            .find_map(|k| self.get(k.as_ref()))
            .map(|s| s.to_lowercase())
    }

    pub fn lower(&self, key: &str) -> Option<Cow<'_, str>> {
        self.get(key).map(|s| Cow::Owned(s.to_lowercase()))
    }
}

impl<T: IntoIterator<Item = (String, String)>> From<T> for EnvWrap {
    fn from(value: T) -> Self {
        Self(value.into_iter().collect())
    }
}

impl Config {
    /// Apply overrides from environment
    pub fn with_env(mut self, env: impl IntoIterator<Item = (String, String)>) -> Self {
        let namespace = PROJECT_NAME.to_uppercase(); // need compile-time uppercase

        let vars = EnvWrap::from(env);

        self.dark_mode = match vars.lower(&format!("{namespace}_DARK_MODE")).as_deref() {
            Some("0" | "no" | "false") => false,
            Some("1" | "yes" | "true") => true,
            _ => self.dark_mode,
        };

        self.dark_mode = match vars.lower(&format!("{namespace}_BG")).as_deref() {
            Some("light") => false,
            Some("dark") => true,
            _ => self.dark_mode,
        };

        self.colors = match vars
            .one_of([format!("{namespace}_COLORS"), format!("{namespace}_THEME")])
            .as_deref()
        {
            Some("none" | "mono" | "monochrome") => Some("mono".into()),
            Some(value) => Some(value.into()),
            _ => self.colors,
        };

        self.color_shift = match vars.lower(&format!("{namespace}_COLOR_SHIFT")).as_deref() {
            Some("no" | "none" | "false") => 0.0,
            Some("yes" | "true") => 0.2,
            Some(value) if let Ok(num) = value.parse::<f32>() => num,
            _ => self.color_shift,
        };

        self.dir_style = match vars.lower(&format!("{namespace}_DIR_STYLE")).as_deref() {
            Some("heavy" | "thick" | "strong") => DirStyle::Thick,
            _ => self.dir_style,
        };

        self
    }

    /// Loads file from config directory
    pub fn load() -> Result<Self> {
        let path = dirs::config_dir()
            .unwrap_or_default()
            .join(PROJECT_NAME)
            .join("settings.toml");

        let Ok(text) = std::fs::read_to_string(path) else {
            return Ok(Default::default());
        };

        #[cfg(feature = "full-config")]
        let config = toml::from_str(&text);
        #[cfg(not(feature = "full-config"))]
        let config = Self::parse_nano(&text);

        diag_debug!(?config, "Loaded settings from file");
        Ok(config?)
    }

    #[cfg(not(feature = "full-config"))]
    fn parse_nano(text: &str) -> Result<Self> {
        let mut config = Self {
            dark_mode: default_true(),
            color_shift: default_color_shift(),
            ..Default::default()
        };
        let mut section: Option<String> = None;
        let mut theme_fields = BTreeMap::<String, (bool, bool)>::new();
        let mut lines = text.lines().enumerate();

        while let Some((line_number, raw_line)) = lines.next() {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with('[') {
                let name = line
                    .strip_prefix("[themes.")
                    .and_then(|value| value.strip_suffix(']'))
                    .ok_or_else(|| config_error(line_number, "unsupported section"))?;
                let name = parse_section_name(name)
                    .map_err(|message| config_error(line_number, &message))?;
                config.themes.entry(name.clone()).or_default();
                theme_fields.entry(name.clone()).or_default();
                section = Some(name);
                continue;
            }

            let (key, initial_value) = line
                .split_once('=')
                .ok_or_else(|| config_error(line_number, "expected key = value"))?;
            let key = key.trim();
            let mut value = initial_value.trim().to_string();

            if value.starts_with('[') {
                while !array_is_complete(&value) {
                    let (_, next_line) = lines
                        .next()
                        .ok_or_else(|| config_error(line_number, "unterminated string array"))?;
                    value.push('\n');
                    value.push_str(strip_comment(next_line));
                }
            }

            if let Some(theme_name) = &section {
                let theme = config.themes.get_mut(theme_name).unwrap();
                let fields = theme_fields.get_mut(theme_name).unwrap();
                match key {
                    "dirs" => {
                        theme.dirs = parse_string_array(&value)
                            .map_err(|message| config_error(line_number, &message))?;
                        fields.0 = true;
                    }
                    "files" => {
                        theme.files = parse_string_array(&value)
                            .map_err(|message| config_error(line_number, &message))?;
                        fields.1 = true;
                    }
                    _ => {}
                }
                continue;
            }

            match key {
                "dark_mode" => {
                    config.dark_mode = match value.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => return Err(config_error(line_number, "expected true or false")),
                    };
                }
                "colors" => {
                    config.colors = Some(
                        parse_string(&value)
                            .map_err(|message| config_error(line_number, &message))?,
                    );
                }
                "color_shift" => {
                    config.color_shift = value
                        .parse()
                        .map_err(|_| config_error(line_number, "expected a number"))?;
                }
                "dir_style" => {
                    config.dir_style = match parse_string(&value)
                        .map_err(|message| config_error(line_number, &message))?
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "plain" => DirStyle::Plain,
                        "thick" | "heavy" => DirStyle::Thick,
                        _ => {
                            return Err(config_error(
                                line_number,
                                "expected plain, thick, or heavy",
                            ));
                        }
                    };
                }
                _ => {}
            }
        }

        if let Some((name, _)) = theme_fields
            .iter()
            .find(|(_, (dirs, files))| !dirs || !files)
        {
            return Err(config_error(
                0,
                &format!("theme {name:?} requires dirs and files arrays"),
            ));
        }

        Ok(config)
    }
}

#[cfg(not(feature = "full-config"))]
fn config_error(line_number: usize, message: &str) -> crate::error::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("settings.toml line {}: {message}", line_number + 1),
    )
    .into()
}

#[cfg(not(feature = "full-config"))]
fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote == Some('"') {
            escaped = true;
        } else if matches!(character, '"' | '\'') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            }
        } else if character == '#' && quote.is_none() {
            return &line[..index];
        }
    }
    line
}

#[cfg(not(feature = "full-config"))]
fn array_is_complete(value: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quote == Some('"') {
            escaped = true;
        } else if matches!(character, '"' | '\'') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            }
        } else if character == ']' && quote.is_none() {
            return true;
        }
    }
    false
}

#[cfg(not(feature = "full-config"))]
fn parse_section_name(value: &str) -> std::result::Result<String, String> {
    let value = value.trim();
    if value.starts_with(['"', '\'']) {
        parse_string(value)
    } else if value.is_empty() {
        Err("theme name cannot be empty".to_string())
    } else {
        Ok(value.to_string())
    }
}

#[cfg(not(feature = "full-config"))]
fn parse_string(value: &str) -> std::result::Result<String, String> {
    let value = value.trim();
    let quote = value
        .chars()
        .next()
        .filter(|character| matches!(character, '"' | '\''))
        .ok_or_else(|| "expected a quoted string".to_string())?;
    if !value.ends_with(quote) || value.len() < 2 {
        return Err("unterminated string".to_string());
    }

    let value = &value[1..value.len() - 1];
    if quote == '\'' {
        return Ok(value.to_string());
    }

    let mut output = String::with_capacity(value.len());
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            output.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                _ => return Err(format!("unsupported escape \\{character}")),
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            output.push(character);
        }
    }
    if escaped {
        return Err("unterminated escape".to_string());
    }
    Ok(output)
}

#[cfg(not(feature = "full-config"))]
fn parse_string_array(value: &str) -> std::result::Result<Vec<String>, String> {
    let value = value.trim();
    let inner = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| "expected a string array".to_string())?;
    let mut values = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut start = 0;

    for (index, character) in inner.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quote == Some('"') {
            escaped = true;
        } else if matches!(character, '"' | '\'') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            }
        } else if character == ',' && quote.is_none() {
            let item = inner[start..index].trim();
            if !item.is_empty() {
                values.push(parse_string(item)?);
            }
            start = index + 1;
        }
    }

    let item = inner[start..].trim();
    if !item.is_empty() {
        values.push(parse_string(item)?);
    }
    Ok(values)
}

#[cfg(all(test, not(feature = "full-config")))]
mod nano_config_tests {
    use super::{Config, DirStyle};

    #[test]
    fn parses_supported_settings_and_custom_themes() {
        let config = Config::parse_nano(
            r##"
                dark_mode = false
                colors = "custom" # selected theme
                color_shift = 0.35
                dir_style = "heavy"

                [themes.custom]
                dirs = [
                  "#4ac16d",
                  "hsl(120, 100%, 25%)",
                ]
                files = ["#f5db4c", "red"]
            "##,
        )
        .unwrap();

        assert!(!config.dark_mode);
        assert_eq!(config.colors.as_deref(), Some("custom"));
        assert_eq!(config.color_shift, 0.35);
        assert_eq!(config.dir_style, DirStyle::Thick);
        assert_eq!(
            config.themes["custom"].dirs,
            ["#4ac16d", "hsl(120, 100%, 25%)"]
        );
        assert_eq!(config.themes["custom"].files, ["#f5db4c", "red"]);
    }

    #[test]
    fn applies_file_defaults() {
        let config = Config::parse_nano("colors = 'fall'").unwrap();
        assert!(config.dark_mode);
        assert_eq!(config.color_shift, 0.2);
        assert_eq!(config.dir_style, DirStyle::Plain);
    }

    #[test]
    fn rejects_incomplete_themes() {
        let error = Config::parse_nano("[themes.broken]\ndirs = ['red']")
            .err()
            .unwrap();
        assert!(error.to_string().contains("requires dirs and files"));
    }
}
