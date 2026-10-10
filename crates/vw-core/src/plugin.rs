use std::fmt;
use std::path::Path;
use std::sync::Arc;

use crate::image::{Metadata, VwImage};

#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    Unsupported(String),
    Invalid(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Io(e) => write!(f, "I/O error: {e}"),
            DecodeError::Unsupported(s) => write!(f, "unsupported: {s}"),
            DecodeError::Invalid(s) => write!(f, "invalid data: {s}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        DecodeError::Io(e)
    }
}

/// A file format decoder. Each supported format (PNG, BMP, JPEG, DICOM, NEF, ARW, ...)
/// is one plugin that turns file bytes into a [`VwImage`].
///
/// Plugins are currently linked statically and registered in [`Registry`]; the trait is
/// kept object-safe and free of generics so it can later sit behind a C ABI for
/// dynamically loaded plugins.
pub trait DecoderPlugin: Send + Sync {
    /// Short display name, e.g. "PNG".
    fn name(&self) -> &str;

    /// Lower-case file extensions without the dot, e.g. `["png"]`.
    fn extensions(&self) -> &[&str];

    /// Returns true when `header` (the first bytes of the file, at least 64 when
    /// the file is that long) looks like this format.
    fn probe(&self, header: &[u8]) -> bool;

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError>;

    /// Metadata without decoding pixels. The default decodes the whole image.
    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        self.decode(data).map(|img| img.metadata)
    }
}

/// The set of available decoder plugins.
#[derive(Clone, Default)]
pub struct Registry {
    plugins: Vec<Arc<dyn DecoderPlugin>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, plugin: Arc<dyn DecoderPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn plugins(&self) -> &[Arc<dyn DecoderPlugin>] {
        &self.plugins
    }

    /// True if some plugin claims the extension of `path` (cheap filter for file lists).
    pub fn supports_path(&self, path: &Path) -> bool {
        self.by_extension(path).is_some()
    }

    fn by_extension(&self, path: &Path) -> Option<&Arc<dyn DecoderPlugin>> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        self.plugins
            .iter()
            .find(|p| p.extensions().contains(&ext.as_str()))
    }

    /// Picks a plugin by content first, falling back to the file extension.
    pub fn find(&self, path: &Path, header: &[u8]) -> Option<&Arc<dyn DecoderPlugin>> {
        self.plugins
            .iter()
            .find(|p| p.probe(header))
            .or_else(|| self.by_extension(path))
    }

    pub fn decode_bytes(&self, path: &Path, data: &[u8]) -> Result<(VwImage, &str), DecodeError> {
        let plugin = self
            .find(path, data)
            .ok_or_else(|| DecodeError::Unsupported(format!("no plugin for {}", path.display())))?;
        Ok((plugin.decode(data)?, plugin.name()))
    }
}
