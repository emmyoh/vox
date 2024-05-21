use crate::{
    date::{locale_string_to_locale, Date},
    error::{DateNotFound, FrontmatterNotFound, InvalidCollectionsProperty},
};
use liquid::{Object, Parser};
use miette::NamedSource;
use serde::{Deserialize, Serialize};
use std::{error::Error, ffi::OsString, fmt, fs, path::PathBuf};
use toml::{value::Datetime, Table};

#[derive(PartialEq, Clone, Default, Debug, Serialize, Deserialize)]
/// Internal representation of a page.
pub struct Page {
    /// A page's contextual data, represented as TOML at the head of the file.
    pub data: Table,
    /// A page's contents following the frontmatter.
    pub content: String,
    /// Data representing the output path of a page.
    /// This is defined in a page's frontmatter.
    pub permalink: String,
    /// A page's date-time metadata, formatted per the RFC 3339 standard.
    /// This is defined in a page's frontmatter.
    pub date: Date,
    /// The collections a page depends on.
    /// This is defined in a page's frontmatter.
    pub collections: Option<Vec<String>>,
    /// The layout a page uses.
    /// This is defined in a page's frontmatter.
    pub layout: Option<String>,
    /// Path to the page, not including the page itself.
    pub directory: String,
    /// The page's base filename.
    pub name: String,
    /// The output path of a file; a processed `permalink` value.
    pub url: String,
    /// The rendered content of a page.
    pub rendered: String,
}

impl fmt::Display for Page {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:#?}")
    }
}

impl Page {
    /// Renders a page's content and URL.
    ///
    /// # Arguments
    ///
    /// * `contexts` - The Liquid contexts to render with.
    ///
    /// * `parser` - The Liquid parser to render with.
    ///
    /// # Returns
    ///
    /// Whether or not the page changed when rendering.
    pub fn render(
        &mut self,
        contexts: &Object,
        parser: &Parser,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let permalink_changed = self.render_url(&contexts, &parser)?;
        let rendered_content = parser.parse(&self.content)?.render(contexts)?;
        if !permalink_changed && rendered_content == self.rendered {
            return Ok(false);
        }
        self.rendered = rendered_content;
        Ok(true)
    }

    /// Render a page's URL from its permalink value.
    ///
    /// # Arguments
    ///
    /// * `contexts` - The Liquid contexts to render with.
    ///
    /// * `parser` - The Liquid parser to render with.
    ///
    /// # Returns
    ///
    /// Whether or not the page's URL changed when rendering.
    pub fn render_url(
        &mut self,
        contexts: &Object,
        parser: &Parser,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let expanded_permalink = match self.permalink.as_str() {
            "date" => {
                "/{{ page.data.collection }}/{{ page.date.year }}/{{ page.date.month }}/{{ page.date.day }}/{{ page.data.title }}.html".to_owned()
            }
            "pretty" => {
                "/{{ page.data.collection }}/{{ page.date.year }}/{{ page.date.month }}/{{ page.date.day }}/{{ page.data.title }}/index.html".to_owned()
            }
            "ordinal" => {
                "/{{ page.data.collection }}/{{ page.date.year }}/{{ page.date.y_day }}/{{ page.data.title }}.html"
                    .to_owned()
            }
            "weekdate" => {
                "/{{ page.data.collection }}/{{ page.date.year }}/W{{ page.date.week }}/{{ page.date.short_day }}/{{ page.data.title }}.html".to_owned()
            }
            "none" => {
                "/{{ page.data.collection }}/{{ page.data.title }}.html".to_owned()
            }
            _ => {
                self.permalink.to_owned()
            }
        };
        let rendered_permalink = parser.parse(&expanded_permalink)?.render(contexts)?;
        if rendered_permalink == self.url {
            return Ok(false);
        }
        self.url = rendered_permalink;
        Ok(true)
    }

    /// Separate a page's contents into the frontmatter and body.
    ///
    /// # Arguments
    ///
    /// * `contents` - The contents of the page.
    ///
    /// * `path` - The path to the page.
    ///
    /// # Returns
    ///
    /// A tuple where the first element is the frontmatter, and where the second element is the body.
    pub fn get_frontmatter_and_body(
        contents: String,
        path: PathBuf,
    ) -> Result<(String, String), Box<dyn Error + Send + Sync>> {
        let contents_clone = contents.clone();
        let mut lines = contents_clone.lines();
        let start_of_frontmatter = lines.position(|x| x == "---").ok_or(FrontmatterNotFound {
            src: NamedSource::new(path.to_string_lossy(), contents.clone()),
        })?;
        let end_of_frontmatter = lines.position(|x| x == "---").ok_or(FrontmatterNotFound {
            src: NamedSource::new(path.to_string_lossy(), contents.clone()),
        })?;
        let body = lines.collect::<Vec<&str>>().join("\n");
        let frontmatter = contents
            .lines()
            .map(|x| x.to_string())
            .collect::<Vec<String>>()[start_of_frontmatter + 1..end_of_frontmatter]
            .join("\n");
        Ok((frontmatter, body))
    }

    /// Create a representation of a page in memory.
    ///
    /// # Arguments
    ///
    /// * `contents` - The contents of the page.
    ///
    /// * `path` - The path to the page.
    ///
    /// * `locale` - The locale used to render dates and times.
    ///
    /// # Returns
    ///
    /// An instance of a page.
    pub fn new(
        contents: String,
        path: PathBuf,
        locale: String,
    ) -> Result<Page, Box<dyn Error + Send + Sync>> {
        let path = fs::canonicalize(path)?;
        let (frontmatter, body) = Self::get_frontmatter_and_body(contents.clone(), path.clone())?;
        let frontmatter_data = frontmatter.parse::<Table>()?;
        let frontmatter_data_clone = frontmatter_data.clone();
        let date_value: &Datetime = frontmatter_data_clone
            .get("date")
            .ok_or(DateNotFound {
                src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
            })?
            .as_datetime()
            .ok_or(DateNotFound {
                src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
            })?;
        let locale = locale_string_to_locale(locale);
        let layout = frontmatter_data_clone.get("layout").map(|p| p.to_string());
        let collections = match frontmatter_data_clone.get("collections") {
            Some(collections) => Some(
                collections
                    .as_array()
                    .ok_or(InvalidCollectionsProperty {
                        src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
                    })?
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .ok_or(InvalidCollectionsProperty {
                                src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
                            })
                            .unwrap()
                            .to_string()
                    })
                    .collect(),
            ),
            None => None,
        };
        Ok(Page {
            data: frontmatter_data,
            content: body,
            permalink: String::new(),
            date: Date::value_to_date(*date_value, locale),
            layout,
            collections,
            directory: path
                .parent()
                .unwrap_or(&PathBuf::new())
                .to_string_lossy()
                .to_string(),
            name: path
                .file_stem()
                .unwrap_or(&OsString::new())
                .to_string_lossy()
                .to_string(),
            url: String::new(),
            rendered: String::new(),
        })
    }
}
