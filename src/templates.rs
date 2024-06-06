use glob::glob;
use miette::IntoDiagnostic;
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};
use syntect::{highlighting::ThemeSet, html::css_for_theme};

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
            for entry in snippets_directory.flatten() {
                // if let Ok(entry) = entry {
                if entry.file_type().unwrap().is_file() {
                    let name = entry
                        .path()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string();
                    self.names.push(name);
                }
                // }
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

/// Generate stylesheets for syntax highlighting.
pub fn generate_syntax_css() -> miette::Result<()> {
    let css_path = PathBuf::from("output/css/");
    let dark_css_path = css_path.join("dark-code.css");
    let light_css_path = css_path.join("light-code.css");
    let code_css_path = css_path.join("code.css");
    std::fs::create_dir_all(css_path).into_diagnostic()?;

    let ts = ThemeSet::load_defaults();
    let dark_theme = &ts.themes["base16-ocean.dark"];
    let css_dark = css_for_theme(dark_theme);
    std::fs::write(dark_css_path, css_dark).into_diagnostic()?;

    let light_theme = &ts.themes["base16-ocean.light"];
    let css_light = css_for_theme(light_theme);
    std::fs::write(light_css_path, css_light).into_diagnostic()?;

    let css = r#"@import url("light-code.css") (prefers-color-scheme: light);@import url("dark-code.css") (prefers-color-scheme: dark);"#;
    std::fs::write(code_css_path, css).into_diagnostic()?;
    Ok(())
}
