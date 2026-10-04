//! Layered settings: an override struct merged on top of a base struct.

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub name: Option<String>,
    pub retries: Option<u32>,
    pub tag: Option<String>,
}

/// Merge `over` on top of `base`, returning the effective settings.
pub fn apply_overrides(base: Settings, over: Settings) -> Settings {
    Settings {
        name: over.name,
        retries: over.retries,
        tag: over.tag,
    }
}
