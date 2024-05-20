use ahash::AHashMap;
use glob::glob;
use liquid::{object, Object};
use mimalloc::MiMalloc;
use std::{error::Error, fs};
use toml::Table;
use vox::{builds::Build, markdown_block::MarkdownBlock, math_block::MathBlock};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    Ok(())
}

fn build() -> Result<(), Box<dyn Error>> {
    let parser = create_liquid_parser()?;
    let global = get_global_context()?;
    // let build = Build {
    //     template_parser: create_liquid_parser,
    //     contexts: global.0,
    //     locale: global.1,
    // };
    Ok(())
}

fn get_global_context() -> Result<(Object, String), Box<dyn Error>> {
    let global_file = fs::read_to_string("global.toml")?;
    let global_context = global_file.parse::<Table>()?;
    let locale: String = global_context
        .get("locale")
        .unwrap_or(&toml::Value::String("en_US".to_string()))
        .to_string();
    Ok((
        object!({
            "global": global_context
        }),
        locale,
    ))
}

fn create_liquid_parser() -> Result<liquid::Parser, Box<dyn Error>> {
    let mut partials = liquid::partials::InMemorySource::new();
    for fs_entry in glob("snippets/**/*")? {
        let fs_entry = fs_entry?;
        if fs_entry.is_file() {
            partials.add(
                fs_entry
                    .clone()
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
                fs::read_to_string(fs_entry)?,
            );
        }
    }
    let partial_compiler = liquid::partials::EagerCompiler::new(partials);
    Ok(liquid::ParserBuilder::with_stdlib()
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
        .build()?)
}
