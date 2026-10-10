//! Core of vw: the in-memory image model every decoder plugin produces, the
//! plugin interface, and conversion of that model into display memory.
//!
//! Pipeline: file bytes --[DecoderPlugin]--> [`VwImage`] --[`display`]--> [`DisplayImage`] (RGBA8)

pub mod display;
pub mod image;
pub mod plugin;

pub use display::{DisplayImage, DisplayParams};
pub use image::{ColorModel, Metadata, PixelData, VwImage};
pub use plugin::{DecodeError, DecoderPlugin, Registry};
