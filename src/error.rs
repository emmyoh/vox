use miette::{Diagnostic, NamedSource};
use thiserror::Error;

// #[derive(Error, Debug, Diagnostic)]
// /// Errors during building.
// pub enum BuildError {
//     #[error("Unable to render math.")]
//     #[diagnostic(
//         code(templates::math_not_valid),
//         url(docsrs),
//         help("Ensure your math is written in valid TeX math mode notation.")
//     )]
//     /// Frontmatter not found.
//     MathNotValid(#[from] LatexError),
// }

#[derive(Error, Debug, Diagnostic)]
#[error("Frontmatter not found.")]
#[diagnostic(
    code(page::frontmatter_not_found),
    url(docsrs),
    help("Please ensure that all pages have frontmatter before attempting builds.")
)]
/// Frontmatter not found.
pub struct FrontmatterNotFound {
    #[source_code]
    /// The page missing its frontmatter.
    pub src: NamedSource<String>,
}

#[derive(Error, Debug, Diagnostic)]
#[error("Page date not found.")]
#[diagnostic(
    code(page::date_not_found),
    url(docsrs),
    help("Please ensure that all pages have their date specified in their frontmatter before attempting builds.")
)]
/// Page date not found.
pub struct DateNotFound {
    #[source_code]
    /// The page missing its date.
    pub src: NamedSource<String>,
}

#[derive(Error, Debug, Diagnostic)]
#[error("Invalid list of dependent collections.")]
#[diagnostic(
    code(page::invalid_collections_property),
    url(docsrs),
    help("Please ensure that your `collections` property is a list of collections this page depends on.")
)]
/// Invalid list of dependent collections.
pub struct InvalidCollectionsProperty {
    #[source_code]
    /// The page with the invalid `collections` property.
    pub src: NamedSource<String>,
}
