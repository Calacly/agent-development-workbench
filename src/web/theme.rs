//! Themes for the server-rendered UI: the built-in themes (themes.json, same
//! set as the former Svelte frontend) plus the user's custom themes, written
//! as CSS custom properties into each page.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::models::user_settings::CustomTheme;

#[derive(Debug, Clone, Deserialize)]
pub struct Theme {
    pub name: String,
    pub mode: String,
    pub colors: HashMap<String, String>,
}

/// Built-in themes, embedded at compile time
pub fn builtin_themes() -> &'static [Theme] {
    static THEMES: OnceLock<Vec<Theme>> = OnceLock::new();
    THEMES.get_or_init(|| {
        serde_json::from_str(include_str!("themes.json")).expect("valid themes.json")
    })
}

impl From<&CustomTheme> for Theme {
    fn from(custom: &CustomTheme) -> Self {
        let c = &custom.colors;
        let colors = [
            ("primary", &c.primary),
            ("secondary", &c.secondary),
            ("accent", &c.accent),
            ("neutral", &c.neutral),
            ("base-100", &c.base_100),
            ("base-200", &c.base_200),
            ("base-300", &c.base_300),
            ("info", &c.info),
            ("success", &c.success),
            ("warning", &c.warning),
            ("error", &c.error),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
        Theme { name: custom.name.clone(), mode: custom.mode.clone(), colors }
    }
}

/// Find a theme by name (case-insensitive), custom themes first.
pub fn find_theme(name: &str, custom: &[CustomTheme]) -> Option<Theme> {
    custom
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
        .map(Theme::from)
        .or_else(|| builtin_themes().iter().find(|t| t.name.eq_ignore_ascii_case(name)).cloned())
}

/// Accept only #rgb/#rrggbb/#rrggbbaa so stored values cannot inject CSS.
fn safe_color(value: &str) -> Option<&str> {
    let hex = value.strip_prefix('#')?;
    (matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(value)
}

/// Relative luminance of a #rrggbb color (WCAG formula)
fn luminance(hex: &str) -> Option<f64> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() < 6 {
        return None;
    }
    let channel = |i: usize| -> Option<f64> {
        let v = u8::from_str_radix(&hex[i..i + 2], 16).ok()? as f64 / 255.0;
        Some(if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) })
    };
    Some(0.2126 * channel(0)? + 0.7152 * channel(2)? + 0.0722 * channel(4)?)
}

/// Dark or light text, whichever reads better on `background`
fn readable_on(background: &str) -> &'static str {
    match luminance(background) {
        Some(l) if l > 0.4 => "#1f2937",
        _ => "#ffffff",
    }
}

/// `:root { ... }` with the theme's colors as CSS variables
pub fn theme_css(theme: &Theme) -> String {
    let mut css = String::from(":root{");
    for (key, value) in &theme.colors {
        if let Some(color) = safe_color(value) {
            if key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                css.push_str(&format!("--color-{key}:{color};"));
            }
        }
    }
    let dark = theme.mode == "dark";
    // `neutral` is a surface color in dark themes, so text comes from the mode
    let content = if dark {
        "#e5e7eb".to_string()
    } else {
        theme.colors.get("neutral").and_then(|c| safe_color(c)).unwrap_or("#1f2937").to_string()
    };
    css.push_str(&format!("--color-base-content:{content};"));
    if let Some(primary) = theme.colors.get("primary").and_then(|c| safe_color(c)) {
        css.push_str(&format!("--color-primary-content:{};", readable_on(primary)));
    }
    css.push_str(if dark { "color-scheme:dark;" } else { "color-scheme:light;" });
    css.push('}');
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_all_builtin_themes_with_complete_colors() {
        let themes = builtin_themes();
        assert_eq!(themes.len(), 40);
        for theme in themes {
            assert_eq!(theme.colors.len(), 11, "{}", theme.name);
            assert!(theme.colors.values().all(|c| safe_color(c).is_some()), "{}", theme.name);
        }
    }

    #[test]
    fn finds_themes_case_insensitively() {
        assert_eq!(find_theme("light", &[]).unwrap().name, "Light");
        assert!(find_theme("nope", &[]).is_none());
    }

    #[test]
    fn css_uses_mode_for_text_and_rejects_non_colors() {
        let dracula = find_theme("Dracula", &[]).unwrap();
        let css = theme_css(&dracula);
        assert!(css.contains("--color-base-content:#e5e7eb;"), "{css}");
        assert!(css.contains("color-scheme:dark;"));

        let evil = Theme {
            name: "x".into(),
            mode: "light".into(),
            colors: [("primary".to_string(), "red;}body{display:none".to_string())].into_iter().collect(),
        };
        assert!(!theme_css(&evil).contains("display:none"));
    }

    #[test]
    fn primary_content_contrasts_with_primary() {
        assert_eq!(readable_on("#ffff00"), "#1f2937");
        assert_eq!(readable_on("#570df8"), "#ffffff");
    }
}
