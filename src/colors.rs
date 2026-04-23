use anstyle::{AnsiColor, Color, Style};
use anyhow::{anyhow, Context, Result};
use clap::builder::Styles;
use clap::ColorChoice;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::{self, Display};
use std::io::{stderr, stdout, IsTerminal};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn parse(input: Option<&str>) -> Result<Self> {
        match input.unwrap_or("auto").trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            other => Err(anyhow!(
                "invalid color mode '{other}', expected auto|always|never"
            )),
        }
    }

    pub fn should_color(self) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => {
                std::env::var_os("NO_COLOR").is_none()
                    && (stdout().is_terminal() || stderr().is_terminal())
            }
        }
    }

    pub fn clap_choice(self) -> ColorChoice {
        match self {
            Self::Auto => ColorChoice::Auto,
            Self::Always => ColorChoice::Always,
            Self::Never => ColorChoice::Never,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ThemeOverrides {
    banner: Option<String>,
    header: Option<String>,
    accent: Option<String>,
    muted: Option<String>,
    success: Option<String>,
    warning: Option<String>,
    error: Option<String>,
    usage: Option<String>,
    literal: Option<String>,
    placeholder: Option<String>,
    valid: Option<String>,
    invalid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Theme {
    use_color: bool,
    pub banner: Style,
    pub header: Style,
    pub accent: Style,
    pub muted: Style,
    pub success: Style,
    pub warning: Style,
    pub error: Style,
    pub usage: Style,
    pub literal: Style,
    pub placeholder: Style,
    pub valid: Style,
    pub invalid: Style,
}

impl Theme {
    pub fn from_env_and_mode(color_scheme: Option<&str>, mode: ColorMode) -> Result<Self> {
        let mut theme = preset("default");

        if let Some(raw) = color_scheme
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if raw.starts_with('{') {
                let overrides: ThemeOverrides =
                    serde_json::from_str(raw).context("failed to parse ACLI_COLOR_SCHEME JSON")?;
                apply_overrides(&mut theme, overrides)?;
            } else {
                let preset_name = raw.to_ascii_lowercase();
                match preset_name.as_str() {
                    "default" | "mono" | "ocean" | "sunset" => {
                        theme = preset(&preset_name);
                    }
                    _ => {
                        let overrides = parse_inline_theme(raw)?;
                        apply_overrides(&mut theme, overrides)?;
                    }
                }
            }
        }

        theme.use_color = mode.should_color();
        Ok(theme)
    }

    pub fn clap_styles(&self) -> Styles {
        Styles::styled()
            .header(self.header)
            .usage(self.usage)
            .literal(self.literal)
            .placeholder(self.placeholder)
            .valid(self.valid)
            .invalid(self.invalid)
            .error(self.error)
    }

    pub fn render(&self, style: Style, value: impl Display) -> String {
        if self.use_color {
            format!("{style}{value}{style:#}")
        } else {
            value.to_string()
        }
    }

    pub fn banner(&self, value: impl Display) -> String {
        self.render(self.banner, value)
    }

    pub fn header(&self, value: impl Display) -> String {
        self.render(self.header, value)
    }

    pub fn accent(&self, value: impl Display) -> String {
        self.render(self.accent, value)
    }

    pub fn muted(&self, value: impl Display) -> String {
        self.render(self.muted, value)
    }

    pub fn success(&self, value: impl Display) -> String {
        self.render(self.success, value)
    }

    pub fn warning(&self, value: impl Display) -> String {
        self.render(self.warning, value)
    }
}

fn preset(name: &str) -> Theme {
    fn must(spec: &str) -> Style {
        style(spec).expect("preset style must be valid")
    }

    let (
        banner,
        header,
        accent,
        muted,
        success,
        warning,
        error,
        usage,
        literal,
        placeholder,
        valid,
        invalid,
    ) = match name {
        "mono" => (
            must("bold"),
            must("bold"),
            must("bold"),
            must("dim"),
            must("bold"),
            must("bold"),
            must("bold"),
            must("bold"),
            must("bold"),
            must(""),
            must("bold underline"),
            must("bold underline"),
        ),
        "ocean" => (
            must("bright-cyan bold"),
            must("bright-blue bold"),
            must("cyan bold"),
            must("bright-black"),
            must("green bold"),
            must("yellow bold"),
            must("bright-red bold"),
            must("bright-blue bold"),
            must("cyan bold"),
            must("blue"),
            must("green underline"),
            must("red bold"),
        ),
        "sunset" => (
            must("bright-magenta bold"),
            must("bright-yellow bold"),
            must("magenta bold"),
            must("bright-black"),
            must("green bold"),
            must("yellow bold"),
            must("bright-red bold"),
            must("bright-yellow bold"),
            must("magenta bold"),
            must("yellow"),
            must("green underline"),
            must("red bold"),
        ),
        _ => (
            must("bright-cyan bold"),
            must("bright-cyan bold"),
            must("cyan bold"),
            must("bright-black"),
            must("green bold"),
            must("yellow bold"),
            must("bright-red bold"),
            must("bright-blue bold"),
            must("green bold"),
            must("yellow"),
            must("green underline"),
            must("red bold"),
        ),
    };

    Theme {
        use_color: true,
        banner,
        header,
        accent,
        muted,
        success,
        warning,
        error,
        usage,
        literal,
        placeholder,
        valid,
        invalid,
    }
}

fn parse_inline_theme(input: &str) -> Result<ThemeOverrides> {
    let mut map = BTreeMap::new();

    for chunk in input.split(',') {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }

        let (key, value) = chunk.split_once('=').ok_or_else(|| {
            anyhow!("invalid inline color theme segment '{chunk}', expected key=value")
        })?;
        map.insert(key.trim().to_string(), value.trim().to_string());
    }

    let as_json = serde_json::to_string(&map)?;
    Ok(serde_json::from_str(&as_json)?)
}

fn apply_overrides(theme: &mut Theme, overrides: ThemeOverrides) -> Result<()> {
    if let Some(value) = overrides.banner {
        theme.banner = style(&value)?;
    }
    if let Some(value) = overrides.header {
        theme.header = style(&value)?;
    }
    if let Some(value) = overrides.accent {
        theme.accent = style(&value)?;
    }
    if let Some(value) = overrides.muted {
        theme.muted = style(&value)?;
    }
    if let Some(value) = overrides.success {
        theme.success = style(&value)?;
    }
    if let Some(value) = overrides.warning {
        theme.warning = style(&value)?;
    }
    if let Some(value) = overrides.error {
        theme.error = style(&value)?;
    }
    if let Some(value) = overrides.usage {
        theme.usage = style(&value)?;
    }
    if let Some(value) = overrides.literal {
        theme.literal = style(&value)?;
    }
    if let Some(value) = overrides.placeholder {
        theme.placeholder = style(&value)?;
    }
    if let Some(value) = overrides.valid {
        theme.valid = style(&value)?;
    }
    if let Some(value) = overrides.invalid {
        theme.invalid = style(&value)?;
    }

    Ok(())
}

fn style(spec: &str) -> Result<Style> {
    let mut style = Style::new();
    let spec = spec.trim();

    if spec.is_empty() {
        return Ok(style);
    }

    for token in spec
        .split(|c: char| c.is_whitespace() || c == '+' || c == ',')
        .filter(|token| !token.is_empty())
    {
        let token = token.trim().to_ascii_lowercase();
        match token.as_str() {
            "bold" => style = style.bold(),
            "dim" | "dimmed" => style = style.dimmed(),
            "italic" => style = style.italic(),
            "underline" => style = style.underline(),
            "blink" => style = style.blink(),
            "invert" | "inverse" => style = style.invert(),
            "hidden" => style = style.hidden(),
            "none" | "default" | "reset" => {}
            other => {
                let color = parse_color(other)
                    .with_context(|| format!("unknown color or style token '{other}'"))?;
                style = style.fg_color(Some(color));
            }
        }
    }

    Ok(style)
}

fn parse_color(token: &str) -> Result<Color> {
    let normalized = token.replace('_', "-");
    let ansi = match normalized.as_str() {
        "black" => AnsiColor::Black,
        "red" => AnsiColor::Red,
        "green" => AnsiColor::Green,
        "yellow" => AnsiColor::Yellow,
        "blue" => AnsiColor::Blue,
        "magenta" => AnsiColor::Magenta,
        "cyan" => AnsiColor::Cyan,
        "white" => AnsiColor::White,
        "bright-black" | "light-black" | "gray" | "grey" => AnsiColor::BrightBlack,
        "bright-red" | "light-red" => AnsiColor::BrightRed,
        "bright-green" | "light-green" => AnsiColor::BrightGreen,
        "bright-yellow" | "light-yellow" => AnsiColor::BrightYellow,
        "bright-blue" | "light-blue" => AnsiColor::BrightBlue,
        "bright-magenta" | "light-magenta" => AnsiColor::BrightMagenta,
        "bright-cyan" | "light-cyan" => AnsiColor::BrightCyan,
        "bright-white" | "light-white" => AnsiColor::BrightWhite,
        _ => return Err(anyhow!("unsupported ANSI color '{token}'")),
    };

    Ok(ansi.into())
}

impl fmt::Display for ColorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Always => write!(f, "always"),
            Self::Never => write!(f, "never"),
        }
    }
}
