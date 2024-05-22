use actix_files::NamedFile;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::{App, HttpServer};
use ahash::AHashMap;
use clap::{arg, crate_version};
use clap::{Parser, Subcommand};
use daggy::{stable_dag::StableDag, NodeIndex};
use glob::glob;
use liquid::{object, Object};
use mimalloc::MiMalloc;
use notify_debouncer_full::{
    new_debouncer,
    notify::{
        event::{ModifyKind, RemoveKind, RenameMode},
        EventKind, RecursiveMode, Watcher,
    },
};
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::mpsc::channel;
use std::{error::Error, fs, path::PathBuf, time::Duration};
use ticky::Stopwatch;
use toml::Table;
use tracing::info;
use vox::{builds::Build, page::Page, templates::create_liquid_parser};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}
#[derive(Subcommand)]
enum Commands {
    Build {
        #[arg(short, long, default_value_t = false)]
        watch: bool,
    },
    Serve {
        #[arg(short, long, default_value_t = true)]
        watch: bool,
        #[arg(short, long, default_value_t = 80)]
        port: u16,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    tracing_subscriber::fmt().init();
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Build { watch }) => {
            tokio::spawn(build(watch)).await??;
        }
        Some(Commands::Serve { watch, port }) => {
            tokio::spawn(build(watch)).await??;
            tokio::spawn(
                HttpServer::new(|| {
                    let mut service = actix_files::Files::new("/", "output")
                        .prefer_utf8(true)
                        .use_hidden_files()
                        .use_etag(true)
                        .use_last_modified(true)
                        .show_files_listing()
                        .redirect_to_slash_directory();
                    if Path::new("output/index.html").is_file() {
                        service = service.index_file("index.html");
                    }
                    if Path::new("output/404.html").is_file() {
                        service = service.default_handler(|req: ServiceRequest| {
                            let (http_req, _payload) = req.into_parts();
                            async {
                                let response =
                                    NamedFile::open("output/404.html")?.into_response(&http_req);
                                Ok(ServiceResponse::new(http_req, response))
                            }
                        });
                    };
                    App::new().service(service)
                })
                .bind((Ipv4Addr::UNSPECIFIED, port))?
                .run(),
            )
            .await??;
        }
        None => println!("Vox {}", crate_version!()),
    }
    Ok(())
}

fn insert_or_update_page(
    entry: PathBuf,
    dag: &mut StableDag<Page, ()>,
    pages: &mut AHashMap<PathBuf, NodeIndex>,
    locale: String,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let entry = fs::canonicalize(entry)?;
    let page = path_to_page(entry.clone(), locale.clone())?;
    // If the page already exists in the DAG, update it. Otherwise, insert it.
    let index = if pages.contains_key(&entry) {
        let index = pages[&entry];
        let node = dag.node_weight_mut(index).unwrap();
        *node = page.clone();
        index
    } else {
        let index = dag.add_node(page.clone());
        pages.insert(entry, index);
        index
    };

    // A page's parents are its layout and the collections it depends on.
    // The DAG indices of the parents must be found.
    let layout = page.layout.clone();
    let collections = page.collections.clone();
    let mut parents: Vec<NodeIndex> = Vec::new();
    if let Some(layout) = layout {
        let layout = fs::canonicalize(layout)?;
        if let Some(index) = pages.get(&layout) {
            parents.push(*index);
        } else {
            let page = path_to_page(layout.clone(), locale.clone())?;
            let index = dag.add_node(page);
            parents.push(index);
            pages.insert(layout, index);
        }
    }
    if let Some(collections) = collections {
        for collection in collections {
            let collection = fs::canonicalize(collection)?;
            for entry in glob(&format!("{}/**/*.vox", collection.to_string_lossy()))? {
                let entry = fs::canonicalize(entry?)?;
                if let Some(index) = pages.get(&entry) {
                    parents.push(*index);
                } else {
                    let page = path_to_page(entry.clone(), locale.clone())?;
                    let index = dag.add_node(page);
                    parents.push(index);
                    pages.insert(entry, index);
                }
            }
        }
    }

    // Now that the parents have been found, edges can be added to the DAG.
    for parent in parents {
        dag.add_edge(parent, index, ())?;
    }

    Ok(())
}

async fn build(watch: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
    let parser = create_liquid_parser()?;
    let global = get_global_context()?;
    let mut dag = StableDag::new();
    let mut pages: AHashMap<PathBuf, NodeIndex> = AHashMap::new();

    // Initial DAG construction.
    for entry in glob("**/*.vox")? {
        let entry = fs::canonicalize(entry?)?;
        if pages.contains_key(&entry) {
            continue;
        }
        insert_or_update_page(entry, &mut dag, &mut pages, global.1.clone())?;
    }

    // Write the initial site to the output directory.
    let (updated_pages, updated_dag) = tokio::spawn(async move {
        generate_site(parser.clone(), global.0.clone(), global.1.clone(), dag).await
    })
    .await??;
    dag = updated_dag;

    // Watch for changes to the site.
    if watch {
        let current_path = std::env::current_dir()?;
        let output_path = current_path.join("output");
        let git_path = current_path.join(".git");
        let (sender, receiver) = channel();
        let mut debouncer = new_debouncer(Duration::from_secs(1), None, sender)?;
        debouncer
            .watcher()
            .watch(&current_path, RecursiveMode::Recursive)?;
        // Changes to the output directory or version control are irrelevant.
        debouncer.watcher().unwatch(&git_path)?;
        debouncer.watcher().unwatch(&output_path)?;

        loop {
            if let Ok(events) = receiver.recv()? {
                for event in events {
                    match event.kind {
                        // If a new page is created, insert it into the DAG.
                        EventKind::Create(_) => {
                            let parser = create_liquid_parser()?;
                            let global = get_global_context()?;
                            let page_paths: Vec<&PathBuf> = event
                                .paths
                                .iter()
                                .filter(|path| {
                                    path.exists()
                                        && path.is_file()
                                        && path.extension().unwrap_or_default() == "vox"
                                })
                                .collect();
                            for path in page_paths {
                                insert_or_update_page(
                                    path.clone(),
                                    &mut dag,
                                    &mut pages,
                                    global.1.clone(),
                                )?;
                            }
                            let (updated_pages, updated_dag) = tokio::spawn(async move {
                                generate_site(
                                    parser.clone(),
                                    global.0.clone(),
                                    global.1.clone(),
                                    dag,
                                )
                                .await
                            })
                            .await??;
                            dag = updated_dag;
                        }
                        EventKind::Modify(modify_kind) => match modify_kind {
                            ModifyKind::Name(rename_mode) => match rename_mode {
                                RenameMode::Both => {
                                    let parser = create_liquid_parser()?;
                                    let global = get_global_context()?;
                                    let from_path = fs::canonicalize(event.paths[0].clone())?;
                                    let to_path = fs::canonicalize(event.paths[1].clone())?;
                                    // If the path is a file, update the page in the DAG.
                                    if to_path.is_file() {
                                        let index = pages[&from_path];
                                        pages.remove(&from_path);
                                        dag.remove_node(index);
                                        insert_or_update_page(
                                            to_path.clone(),
                                            &mut dag,
                                            &mut pages,
                                            global.1.clone(),
                                        )?;
                                    }
                                    // If the path is a directory, update all pages in the DAG.
                                    else if to_path.is_dir() {
                                        for (page_path, index) in pages.clone().into_iter() {
                                            if page_path.starts_with(&from_path) {
                                                let to_page_path = to_path
                                                    .join(page_path.strip_prefix(&from_path)?);
                                                pages.remove(&page_path);
                                                dag.remove_node(index);
                                                insert_or_update_page(
                                                    to_page_path,
                                                    &mut dag,
                                                    &mut pages,
                                                    global.1.clone(),
                                                )?;
                                            }
                                        }
                                    };
                                    let (updated_pages, updated_dag) = tokio::spawn(async move {
                                        generate_site(
                                            parser.clone(),
                                            global.0.clone(),
                                            global.1.clone(),
                                            dag,
                                        )
                                        .await
                                    })
                                    .await??;
                                    dag = updated_dag;
                                }
                                _ => continue,
                            },
                            // If a page is modified, update it in the DAG.
                            ModifyKind::Data(_) => {
                                let parser = create_liquid_parser()?;
                                let global = get_global_context()?;
                                let page_paths: Vec<&PathBuf> = event
                                    .paths
                                    .iter()
                                    .filter(|path| {
                                        path.exists()
                                            && path.is_file()
                                            && path.extension().unwrap_or_default() == "vox"
                                    })
                                    .collect();
                                for path in page_paths {
                                    insert_or_update_page(
                                        path.clone(),
                                        &mut dag,
                                        &mut pages,
                                        global.1.clone(),
                                    )?;
                                }
                                let (updated_pages, updated_dag) = tokio::spawn(async move {
                                    generate_site(
                                        parser.clone(),
                                        global.0.clone(),
                                        global.1.clone(),
                                        dag,
                                    )
                                    .await
                                })
                                .await??;
                                dag = updated_dag;
                            }
                            _ => continue,
                        },
                        EventKind::Remove(remove_kind) => match remove_kind {
                            // If a folder is removed, remove all pages in the folder from the DAG.
                            RemoveKind::Folder => {
                                let parser = create_liquid_parser()?;
                                let global = get_global_context()?;
                                let path = fs::canonicalize(event.paths[0].clone())?;
                                for (page_path, index) in pages.clone().into_iter() {
                                    if page_path.starts_with(&path) {
                                        pages.remove(&page_path);
                                        dag.remove_node(index);
                                    }
                                }
                                let (updated_pages, updated_dag) = tokio::spawn(async move {
                                    generate_site(
                                        parser.clone(),
                                        global.0.clone(),
                                        global.1.clone(),
                                        dag,
                                    )
                                    .await
                                })
                                .await??;
                                dag = updated_dag;
                            }
                            // If a file is removed, remove the page from the DAG.
                            RemoveKind::File => {
                                let parser = create_liquid_parser()?;
                                let global = get_global_context()?;
                                let page_paths: Vec<&PathBuf> = event
                                    .paths
                                    .iter()
                                    .filter(|path| {
                                        !path.exists()
                                            && path.is_file()
                                            && path.extension().unwrap_or_default() == "vox"
                                    })
                                    .collect();
                                for path in page_paths {
                                    let path = fs::canonicalize(path)?;
                                    if let Some(index) = pages.get(&path) {
                                        dag.remove_node(*index);
                                        pages.remove(&path);
                                    }
                                }
                                let (updated_pages, updated_dag) = tokio::spawn(async move {
                                    generate_site(
                                        parser.clone(),
                                        global.0.clone(),
                                        global.1.clone(),
                                        dag,
                                    )
                                    .await
                                })
                                .await??;
                                dag = updated_dag;
                            }
                            _ => continue,
                        },
                        _ => continue,
                    }
                }
            }
        }
    }

    Ok(())
}

async fn generate_site(
    template_parser: liquid::Parser,
    contexts: liquid::Object,
    locale: String,
    dag: StableDag<Page, ()>,
) -> Result<(Vec<NodeIndex>, StableDag<Page, ()>), Box<dyn Error + Send + Sync>> {
    let mut timer = Stopwatch::start_new();
    let mut build = Build {
        template_parser,
        contexts,
        locale,
        dag,
    };
    let updated_pages = build.render_all()?;
    for updated_page in updated_pages.iter() {
        let updated_page = &build.dag.graph()[*updated_page];
        let output_path = format!("output/{}", updated_page.url);
        tokio::fs::write(output_path, updated_page.rendered.clone()).await?;
    }
    timer.stop();
    info!(
        "Generated {} pages in {:.2} seconds … ",
        updated_pages.len(),
        timer.elapsed_s()
    );
    Ok((updated_pages, build.dag))
}

fn path_to_page(path: PathBuf, locale: String) -> Result<Page, Box<dyn Error + Send + Sync>> {
    Page::new(fs::read_to_string(path.clone())?, path, locale)
}

fn get_global_context() -> Result<(Object, String), Box<dyn Error + Send + Sync>> {
    let global_context = match fs::read_to_string("global.toml") {
        Ok(global_file) => global_file.parse::<Table>()?,
        Err(_) => "locale = 'en_US'".parse::<Table>()?,
    };
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
