use glob::glob;
use miette::IntoDiagnostic;
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use crate::{markdown_block::MarkdownBlock, math_block::MathBlock};

#[derive(Eq, PartialEq, PartialOrd, Clone, Default, Debug)]
/// A source of snippets for Liquid templates.
pub struct SnippetSource {
    names: Vec<String>,
}

impl SnippetSource {
    /// Create a new `SnippetSource`.
    ///
    /// # Returns
    ///
    /// A new `SnippetSource` with an empty list of snippets.
    pub fn new() -> Self {
        Self { names: Vec::new() }
    }

    /// Update the list of snippets.
    pub fn update_list(&mut self) {
        self.names.clear();
        if let Ok(snippets_directory) = std::fs::read_dir("snippets") {
            for entry in snippets_directory {
                if let Ok(entry) = entry {
                    if entry.file_type().unwrap().is_file() {
                        let name = entry
                            .path()
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .to_string();
                        self.names.push(name);
                    }
                }
            }
        }
    }
}

impl liquid::partials::PartialSource for SnippetSource {
    fn contains(&self, name: &str) -> bool {
        Path::new(&format!("snippets/{}", name)).exists()
            || glob(&format!("snippets/{}.*", name))
                .unwrap()
                .next()
                .is_some()
    }

    fn names(&self) -> Vec<&str> {
        self.names.iter().map(|s| s.as_str()).collect()
    }

    fn try_get(&self, name: &str) -> Option<Cow<str>> {
        let path = PathBuf::from(format!("snippets/{}", name));
        // if path.exists() {
        //     return Some(std::fs::read_to_string(path).unwrap().into());
        // }
        // let path = glob(&format!("snippets/{}.*", name))
        //     .unwrap()
        //     .next()
        //     .unwrap()
        //     .unwrap();
        Some(std::fs::read_to_string(path).unwrap().into())
    }
}

/// Create a Liquid parser with custom tags and filters.
///
/// # Returns
///
/// A Liquid parser with custom tags and filters.
pub fn create_liquid_parser() -> miette::Result<liquid::Parser> {
    let mut partials = SnippetSource::new();
    partials.update_list();
    let partial_compiler = liquid::partials::EagerCompiler::new(partials);
    liquid::ParserBuilder::with_stdlib()
        .tag(liquid_lib::jekyll::IncludeTag)
        .filter(liquid_lib::jekyll::ArrayToSentenceString)
        .filter(liquid_lib::jekyll::Pop)
        .filter(liquid_lib::jekyll::Push)
        .filter(liquid_lib::jekyll::Shift)
        .filter(liquid_lib::jekyll::Slugify)
        .filter(liquid_lib::jekyll::Unshift)
        .filter(liquid_lib::shopify::Pluralize)
        .filter(liquid_lib::extra::DateInTz)
        .block(MathBlock)
        .block(MarkdownBlock)
        .partials(partial_compiler)
        .build()
        .into_diagnostic()
}
