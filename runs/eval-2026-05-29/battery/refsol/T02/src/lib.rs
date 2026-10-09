//! Layered settings: an override struct merged on top of a base struct.

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub name: Option<String>,
    pub retries: Option<u32>,
    pub tag: Option<String>,
}

/// Merge `over` on top of `base`, returning the effective settings.
///
/// A field that is `None` in `over` means "not specified" and leaves the base
/// value alone. A field that is `Some` always wins — including `Some("")` and
/// `Some(0)`, which are explicit values rather than absence.
pub fn apply_overrides(base: Settings, over: Settings) -> Settings {
    Settings {
        name: over.name.or(base.name),
        retries: over.retries.or(base.retries),
        tag: over.tag.or(base.tag),
    }
}
