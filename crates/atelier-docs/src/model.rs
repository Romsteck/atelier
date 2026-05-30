//! Data model: types for docs entries, frontmatter, and metadata.

use serde::{Deserialize, Serialize};

/// Categories of documentation entries. The `overview` is a singleton per app; the others
/// can have many entries identified by a `name` (alphanumeric + `-`, `_`, `.`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocType {
    Overview,
    Screen,
    Feature,
    Component,
}

impl DocType {
    pub fn as_str(self) -> &'static str {
        match self {
            DocType::Overview => "overview",
            DocType::Screen => "screen",
            DocType::Feature => "feature",
            DocType::Component => "component",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "overview" => Some(DocType::Overview),
            "screen" => Some(DocType::Screen),
            "feature" => Some(DocType::Feature),
            "component" => Some(DocType::Component),
            _ => None,
        }
    }

    /// Subdirectory under `{app_id}/` where entries of this type live (None for overview, which
    /// lives at `{app_id}/overview.md`).
    pub fn subdir(self) -> Option<&'static str> {
        match self {
            DocType::Overview => None,
            DocType::Screen => Some("screens"),
            DocType::Feature => Some("features"),
            DocType::Component => Some("components"),
        }
    }
}

/// Scope of a feature. Only meaningful for `DocType::Feature`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Scope {
    /// Cross-screen feature.
    Global,
    /// Feature scoped to a specific screen (parent_screen).
    Screen { parent_screen: String },
}

impl Scope {
    /// Serialize for frontmatter: `global` or `screen:<name>`.
    pub fn to_frontmatter(&self) -> String {
        match self {
            Scope::Global => "global".to_string(),
            Scope::Screen { parent_screen } => format!("screen:{parent_screen}"),
        }
    }

    /// Parse from frontmatter form (`global` | `screen:<name>`).
    pub fn from_frontmatter(s: &str) -> Option<Self> {
        if s == "global" {
            Some(Scope::Global)
        } else if let Some(rest) = s.strip_prefix("screen:") {
            if rest.is_empty() {
                None
            } else {
                Some(Scope::Screen {
                    parent_screen: rest.to_string(),
                })
            }
        } else {
            None
        }
    }

    pub fn parent_screen(&self) -> Option<&str> {
        match self {
            Scope::Global => None,
            Scope::Screen { parent_screen } => Some(parent_screen.as_str()),
        }
    }
}

/// Parsed YAML frontmatter for an entry. All fields optional except title/summary which the
/// agent is expected to fill (validation is performed in `docs.completeness`, not at write).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Frontmatter {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    /// `global` or `screen:<name>` — only for features.
    #[serde(default)]
    pub scope: Option<String>,
    /// Convenience field; redundant with `scope=screen:<name>` but kept for clarity.
    #[serde(default)]
    pub parent_screen: Option<String>,
    /// Code references like `apps/home/src/screens/home.dart:1-120`.
    #[serde(default)]
    pub code_refs: Vec<String>,
    /// Cross-references to other entries: `feature:auth-login`, `screen:home`, `component:app-card`.
    #[serde(default)]
    pub links: Vec<String>,
    /// Mirror of whether `diagrams/{type}-{name}.mmd` exists. Maintained on write.
    #[serde(default)]
    pub diagram: bool,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Metadata for an app, stored at `{app_id}/meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    #[serde(default)]
    pub stack: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub logo: String,
    /// Schema version. v2 is the new structured layout.
    pub schema_version: u32,
}

impl Meta {
    pub fn new(app_id: &str) -> Self {
        Self {
            name: app_id.to_string(),
            stack: String::new(),
            description: String::new(),
            logo: String::new(),
            schema_version: super::SCHEMA_VERSION,
        }
    }
}

/// A full documentation entry (frontmatter + body markdown).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntry {
    pub app_id: String,
    pub doc_type: DocType,
    /// Always `"overview"` for the overview type.
    pub name: String,
    pub frontmatter: Frontmatter,
    /// Markdown body (without frontmatter).
    pub body: String,
}

/// The overview entry plus a compact index of all other entries — what `docs.overview` returns.
#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub app_id: String,
    pub meta: Meta,
    pub overview: Option<DocEntry>,
    pub index: OverviewIndex,
    pub stats: OverviewStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverviewIndex {
    pub screens: Vec<EntrySummary>,
    pub features: Vec<EntrySummary>,
    pub components: Vec<EntrySummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntrySummary {
    pub doc_type: DocType,
    pub name: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    /// Only present for features: `"global"` or `"screen:<name>"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_screen: Option<String>,
    pub has_diagram: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OverviewStats {
    pub screens: u32,
    pub features: u32,
    pub components: u32,
    pub with_diagram: u32,
    pub has_overview: bool,
}

/// Validate an `app_id`: alphanumeric + `-`, `_`. Forbids `/`, `..`, empty.
pub fn validate_app_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.contains('/')
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validate an entry `name`: alphanumeric + `-`, `_`, `.`. Forbids `/`, `..`, empty, leading `.`.
pub fn validate_entry_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 96
        && !s.contains('/')
        && !s.contains("..")
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Serialize a frontmatter into a YAML block (without surrounding `---`).
pub fn frontmatter_to_yaml(fm: &Frontmatter) -> String {
    serde_yaml::to_string(fm).unwrap_or_default()
}

/// Encode the frontmatter+body into a single `.md` payload with `---\n…\n---\n\n{body}`.
pub fn encode_entry(fm: &Frontmatter, body: &str) -> String {
    let yaml = frontmatter_to_yaml(fm);
    let body_trimmed = body.trim_start_matches('\n');
    format!("---\n{yaml}---\n\n{body_trimmed}")
}

/// Decode a `.md` file into (frontmatter, body). If no frontmatter is present, returns
/// (default, full content).
pub fn decode_entry(raw: &str) -> (Frontmatter, String) {
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let yaml = &rest[..end];
            let body = &rest[end + 5..];
            let fm: Frontmatter = serde_yaml::from_str(yaml).unwrap_or_default();
            return (fm, body.trim_start_matches('\n').to_string());
        }
        // Closing fence on the very last line.
        if let Some(end) = rest.rfind("\n---") {
            let yaml = &rest[..end];
            let body = &rest[end + 4..];
            let fm: Frontmatter = serde_yaml::from_str(yaml).unwrap_or_default();
            return (fm, body.trim_start_matches('\n').to_string());
        }
    }
    (Frontmatter::default(), raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_app_id_basic() {
        assert!(validate_app_id("home"));
        assert!(validate_app_id("my-app_1"));
        assert!(!validate_app_id(""));
        assert!(!validate_app_id("a/b"));
        assert!(!validate_app_id("../etc"));
        assert!(!validate_app_id("a b"));
    }

    #[test]
    fn validate_entry_name_basic() {
        assert!(validate_entry_name("home"));
        assert!(validate_entry_name("home.search"));
        assert!(validate_entry_name("auth-login"));
        assert!(!validate_entry_name(""));
        assert!(!validate_entry_name(".hidden"));
        assert!(!validate_entry_name("a/b"));
    }

    #[test]
    fn scope_roundtrip() {
        let g = Scope::Global;
        assert_eq!(g.to_frontmatter(), "global");
        assert_eq!(Scope::from_frontmatter("global"), Some(Scope::Global));
        let s = Scope::Screen {
            parent_screen: "home".into(),
        };
        assert_eq!(s.to_frontmatter(), "screen:home");
        assert_eq!(
            Scope::from_frontmatter("screen:home"),
            Some(Scope::Screen {
                parent_screen: "home".into()
            })
        );
        assert_eq!(Scope::from_frontmatter("screen:"), None);
        assert_eq!(Scope::from_frontmatter("nope"), None);
    }

    #[test]
    fn encode_decode_roundtrip() {
        let mut fm = Frontmatter::default();
        fm.title = Some("Home".into());
        fm.summary = Some("Liste des apps".into());
        fm.diagram = true;
        let body = "## Description\n\nUne page d'accueil.\n";
        let raw = encode_entry(&fm, body);
        let (fm2, body2) = decode_entry(&raw);
        assert_eq!(fm2.title.as_deref(), Some("Home"));
        assert_eq!(fm2.summary.as_deref(), Some("Liste des apps"));
        assert!(fm2.diagram);
        assert_eq!(body2.trim(), body.trim());
    }

    #[test]
    fn decode_no_frontmatter() {
        let raw = "Just some markdown\n\nWith no frontmatter.";
        let (fm, body) = decode_entry(raw);
        assert_eq!(fm.title, None);
        assert_eq!(body, raw);
    }
}
