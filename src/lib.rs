#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![feature(doc_auto_cfg)]
#![warn(missing_docs)]

/// Operations relevant to the build process.
pub mod builds;

/// Date and time representations.
pub mod date;

/// A template block for Markdown.
pub mod markdown_block;

/// A template block for math.
pub mod math_block;

/// Logic pertaining to individual pages.
pub mod page;

/// Errors originating during the build process.
pub mod error;
