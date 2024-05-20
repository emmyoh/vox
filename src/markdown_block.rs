use comrak::markdown_to_html_with_plugins;
use comrak::plugins::syntect::SyntectAdapter;
use comrak::ComrakPlugins;
use comrak::ListStyleType;
use liquid_core::error::ResultLiquidReplaceExt;
use liquid_core::Language;
use liquid_core::Renderable;
use liquid_core::Result;
use liquid_core::Runtime;
use liquid_core::{BlockReflection, ParseBlock, TagBlock, TagTokenIter};
use std::io::Write;

/// Render Markdown as HTML
///
/// # Arguments
///
/// * `text_to_render` - The Markdown text to render into HTML
pub fn render_markdown(text_to_render: String) -> String {
    let mut options = comrak::Options::default();
    options.extension.strikethrough = true;
    options.extension.tagfilter = false;
    options.extension.table = true;
    options.extension.autolink = false;
    options.extension.tasklist = true;
    options.extension.superscript = false;
    options.extension.header_ids = Some(String::from("h-"));
    options.extension.footnotes = true;
    options.extension.description_lists = true;
    options.extension.front_matter_delimiter = None;
    options.extension.multiline_block_quotes = true;
    options.extension.math_dollars = true;
    options.extension.math_code = false;
    options.extension.shortcodes = true;
    options.parse.smart = true;
    options.parse.default_info_string = None;
    options.parse.relaxed_tasklist_matching = true;
    options.parse.relaxed_autolinks = true;
    options.render.hardbreaks = true;
    options.render.github_pre_lang = true;
    options.render.full_info_string = true;
    options.render.width = 80;
    options.render.unsafe_ = true;
    options.render.escape = false;
    options.render.list_style = ListStyleType::Dash;
    options.render.sourcepos = false;
    let mut plugins = ComrakPlugins::default();
    let syntax_highlighting_adapter = SyntectAdapter::new(Some("InspiredGitHub"));
    plugins.render.codefence_syntax_highlighter = Some(&syntax_highlighting_adapter);
    markdown_to_html_with_plugins(&text_to_render, &options, &plugins)
}

#[derive(Copy, Clone, Debug, Default)]
/// A Liquid template block containing Markdown.
/// The block begins with `{% markdown %}` and ends with `{% endmarkdown %}`.
pub struct MarkdownBlock;

impl MarkdownBlock {
    /// Provides a new instance of the Markdown tag parser.
    pub fn new() -> Self {
        Self
    }
}

impl BlockReflection for MarkdownBlock {
    fn start_tag(&self) -> &str {
        "markdown"
    }

    fn end_tag(&self) -> &str {
        "endmarkdown"
    }

    fn description(&self) -> &str {
        ""
    }
}

impl ParseBlock for MarkdownBlock {
    fn parse(
        &self,
        mut arguments: TagTokenIter<'_>,
        mut tokens: TagBlock<'_, '_>,
        _options: &Language,
    ) -> Result<Box<dyn Renderable>> {
        arguments.expect_nothing()?;

        let raw_content = tokens.escape_liquid(false)?.to_string();
        let content = render_markdown(raw_content);

        tokens.assert_empty();
        Ok(Box::new(Markdown { content }))
    }

    fn reflection(&self) -> &dyn BlockReflection {
        self
    }
}

#[derive(Clone, Debug)]
struct Markdown {
    content: String,
}

impl Renderable for Markdown {
    fn render_to(&self, writer: &mut dyn Write, _runtime: &dyn Runtime) -> Result<()> {
        write!(writer, "{}", self.content).replace("Failed to render")?;
        Ok(())
    }
}
