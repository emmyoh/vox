use crate::{
    date::Date,
    error::{DateNotValid, FrontmatterNotFound, InvalidDependsProperty},
};
use chrono::Locale;
use liquid::{Object, Parser};
use miette::IntoDiagnostic;
use miette::NamedSource;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fmt, fs,
    path::{Path, PathBuf},
};
use toml::Table;

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
    pub date: Option<Date>,
    /// The collections a page belongs to.
    /// This is defined by a page's path, with one collection per path component, and collections including each successive path component as well.
    /// Example: `books/fantasy/page.vox` is in `books`, `fantasy`, and `books_fantasy`.
    pub collections: Option<Vec<String>>,
    /// The collections a page depends on.
    /// This is defined in a page's frontmatter.
    pub depends: Option<Vec<String>>,
    /// The layout a page uses.
    /// This is defined in a page's frontmatter.
    pub layout: Option<String>,
    /// Path to the page, not including the page itself.
    pub directory: String,
    /// The page's base filename.
    pub name: String,
    /// Whether or not a page is a layout.
    pub is_layout: bool,
    /// The output path of a file; a processed `permalink` value.
    pub url: String,
    /// The rendered content of a page.
    pub rendered: String,
}

impl fmt::Display for Page {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_path_string())
    }
}

impl Page {
    /// Determine if a page is a layout based on its path.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the page.
    ///
    /// # Returns
    ///
    /// Whether or not the page is a layout.
    pub fn is_layout_path<P: AsRef<Path>>(path: P) -> miette::Result<bool> {
        let current_directory =
            fs::canonicalize(std::env::current_dir().into_diagnostic()?).into_diagnostic()?;
        let page_path = fs::canonicalize(path).into_diagnostic()?;
        let path_difference = page_path
            .strip_prefix(&current_directory)
            .into_diagnostic()?;
        Ok(path_difference.starts_with("layouts/"))
    }

    /// Get the names of the collections a page belongs to based on its path.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the page.
    ///
    /// # Returns
    ///
    /// The names of the collections a page belongs to, or `None` if the page does not belong to a collection.
    pub fn get_collections_from_path<P: AsRef<Path>>(
        path: P,
    ) -> miette::Result<Option<Vec<String>>> {
        let current_directory =
            fs::canonicalize(std::env::current_dir().into_diagnostic()?).into_diagnostic()?;
        let page_path = fs::canonicalize(path).into_diagnostic()?;
        let path_difference = page_path
            .strip_prefix(&current_directory)
            .into_diagnostic()?;
        let path_components: Vec<String> = path_difference
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        let first_path_component = path_components[0].clone();
        if first_path_component == "layouts" {
            return Ok(None);
        }
        if Path::new(first_path_component.as_str()).is_file() {
            return Ok(None);
        }
        let mut results = Vec::new();
        let mut path_builder = Vec::new();
        for path_component in path_components {
            if Path::new(&path_builder.join("/"))
                .join(&path_component)
                .is_file()
            {
                break;
            }
            results.push(path_component.clone());
            path_builder.push(path_component.clone());
            let current_path = path_builder.join("_");
            if path_component != current_path {
                results.push(path_builder.join("_"))
            }
        }
        Ok(Some(results))
    }

    /// Determine if two pages are equivalent despite their rendered content.
    ///
    /// # Arguments
    ///
    /// * `lhs` - The first page to compare.
    ///
    /// * `rhs` - The second page to compare.
    ///
    /// # Returns
    ///
    /// Whether or not the two pages are equivalent.
    pub fn are_equivalent(lhs: &Page, rhs: &Page) -> bool {
        lhs.data == rhs.data
            && lhs.content == rhs.content
            && lhs.permalink == rhs.permalink
            && lhs.date == rhs.date
            && lhs.collections == rhs.collections
            && lhs.depends == rhs.depends
            && lhs.layout == rhs.layout
            && lhs.directory == rhs.directory
            && lhs.name == rhs.name
            && lhs.is_layout == rhs.is_layout
    }

    /// Determine if a page is equivalent to another page aside from rendered content.
    ///
    /// # Arguments
    ///
    /// * `other` - The page to compare to.
    ///
    /// # Returns
    ///
    /// Whether or not the two pages are equivalent.
    pub fn is_equivalent(&self, other: &Page) -> bool {
        Self::are_equivalent(self, other)
    }

    /// Determine if a page is a layout.
    ///
    /// # Returns
    ///
    /// Whether or not the page is a layout.
    pub fn is_layout(&self) -> miette::Result<bool> {
        // let current_directory =
        //     fs::canonicalize(std::env::current_dir().into_diagnostic()?).into_diagnostic()?;
        // let page_path = fs::canonicalize(self.to_path_string()).into_diagnostic()?;
        // let path_difference = page_path
        //     .strip_prefix(&current_directory)
        //     .into_diagnostic()?;
        // Ok(path_difference.starts_with("layouts"))
        Page::is_layout_path(self.to_path_string())
    }

    /// Get the names of the collections a page belongs to.
    ///
    /// # Returns
    ///
    /// The names of the collections a page belongs to, or `None` if the page does not belong to a collection.
    pub fn get_collections(&self) -> miette::Result<Option<Vec<String>>> {
        // let current_directory =
        //     fs::canonicalize(std::env::current_dir().into_diagnostic()?).into_diagnostic()?;
        // let page_path = fs::canonicalize(self.to_path_string()).into_diagnostic()?;
        // let path_difference = page_path
        //     .strip_prefix(&current_directory)
        //     .into_diagnostic()?;
        // let path_components: Vec<String> = path_difference
        //     .components()
        //     .map(|c| c.as_os_str().to_string_lossy().to_string())
        //     .collect();
        // Ok(Some(path_components[0].clone()))
        Page::get_collections_from_path(self.to_path_string())
    }

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
    pub fn render(&mut self, contexts: &Object, parser: &Parser) -> miette::Result<bool> {
        let permalink_changed = self.render_url(contexts, parser)?;
        let rendered_content = parser
            .parse(&self.content)
            .into_diagnostic()?
            .render(contexts)
            .into_diagnostic()?;
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
    pub fn render_url(&mut self, contexts: &Object, parser: &Parser) -> miette::Result<bool> {
        let expanded_permalink = match self.permalink.as_str() {
            "date" => {
                "{{ page.collections.last }}/{{ page.date.year }}/{{ page.date.month }}/{{ page.date.day }}/{{ page.data.title }}.html".to_owned()
            }
            "pretty" => {
                "{{ page.collections.last }}/{{ page.date.year }}/{{ page.date.month }}/{{ page.date.day }}/{{ page.data.title }}/index.html".to_owned()
            }
            "ordinal" => {
                "{{ page.collections.last }}/{{ page.date.year }}/{{ page.date.y_day }}/{{ page.data.title }}.html"
                    .to_owned()
            }
            "weekdate" => {
                "{{ page.collections.last }}/{{ page.date.year }}/W{{ page.date.week }}/{{ page.date.short_day }}/{{ page.data.title }}.html".to_owned()
            }
            "none" => {
                "{{ page.collections.last }}/{{ page.data.title }}.html".to_owned()
            }
            _ => {
                self.permalink.to_owned()
            }
        };
        let rendered_permalink = parser
            .parse(&expanded_permalink)
            .into_diagnostic()?
            .render(contexts)
            .into_diagnostic()?;
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
    ) -> miette::Result<(String, String)> {
        let contents_clone = contents.clone();
        let mut lines = contents_clone.lines();
        let start_of_frontmatter = lines
            .position(|x| x == "---")
            .ok_or(FrontmatterNotFound {
                src: NamedSource::new(path.to_string_lossy(), contents.clone()),
            })
            .into_diagnostic()?;
        let end_of_frontmatter = lines
            .position(|x| x == "---")
            .ok_or(FrontmatterNotFound {
                src: NamedSource::new(path.to_string_lossy(), contents.clone()),
            })
            .into_diagnostic()?;
        let body = lines.collect::<Vec<&str>>().join("\n");
        let frontmatter = contents
            .lines()
            .map(|x| x.to_string())
            .collect::<Vec<String>>()[start_of_frontmatter + 1..end_of_frontmatter + 1]
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
    pub fn new(contents: String, path: PathBuf, locale: Locale) -> miette::Result<Page> {
        let path = fs::canonicalize(path).into_diagnostic()?;
        let (frontmatter, body) = Self::get_frontmatter_and_body(contents.clone(), path.clone())?;
        let frontmatter_data = frontmatter.parse::<Table>().into_diagnostic()?;
        let frontmatter_data_clone = frontmatter_data.clone();
        // let date_value: &Datetime = frontmatter_data_clone
        //     .get("date")
        //     .ok_or(DateNotFound {
        //         src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
        //     })
        //     .into_diagnostic()?
        //     .as_datetime()
        //     .ok_or(DateNotValid {
        //         src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
        //     })
        //     .into_diagnostic()?;
        // let locale = locale_string_to_locale(locale);
        let date = if let Some(date) = frontmatter_data.get("date") {
            // if date.as_str().is_none() {
            //     return Err(DateNotFound {
            //         src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
            //     }
            //     .into());
            // }
            let date_value = date
                .as_datetime()
                .ok_or(DateNotValid {
                    src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
                })
                .into_diagnostic()?;
            Some(Date::value_to_date(*date_value, locale))
        } else {
            None
        };
        let layout = frontmatter_data_clone
            .get("layout")
            .map(|p| p.as_str().unwrap().to_string());
        let permalink = frontmatter_data_clone
            .get("permalink")
            .map(|p| p.as_str().unwrap().to_string());
        let depends = match frontmatter_data_clone.get("depends") {
            Some(depends) => Some(
                depends
                    .as_array()
                    .ok_or(InvalidDependsProperty {
                        src: NamedSource::new(path.to_string_lossy(), frontmatter.clone()),
                    })
                    .into_diagnostic()?
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .ok_or(InvalidDependsProperty {
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
            permalink: permalink.unwrap_or_default(),
            date,
            layout,
            depends,
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
            collections: Page::get_collections_from_path(path.clone())?,
            is_layout: Page::is_layout_path(path)?,
            url: String::new(),
            rendered: String::new(),
        })
    }

    /// Return the path to a page.
    ///
    /// # Returns
    ///
    /// The path to a page as a string.
    pub fn to_path_string(&self) -> String {
        format!("{}/{}.vox", self.directory, self.name)
    }
}
