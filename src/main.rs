use actix_files::NamedFile;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::{App, HttpServer};
use ahash::AHashMap;
use clap::{arg, crate_version};
use clap::{Parser, Subcommand};
use daggy::Walker;
use daggy::{stable_dag::StableDag, NodeIndex};
use glob::glob;
use liquid::{object, Object};
use miette::{Context, IntoDiagnostic};
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
use std::{fs, path::PathBuf, time::Duration};
use ticky::Stopwatch;
use toml::Table;
use tracing::{debug, info, warn, Level};
use vox::builds::EdgeType;
use vox::date::{self};
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
    /// Build the site.
    Build {
        /// Watch for changes (defaults to `false`).
        #[arg(short, long, default_value_t = false)]
        watch: bool,
        /// The level of log output; recoverable errors, warnings, information, debugging information, and trace information.
        #[arg(short, long, action = clap::ArgAction::Count, default_value_t = 0)]
        verbosity: u8,
        /// Whether to visualise the DAG (defaults to `false`).
        #[arg(short = 'd', long, default_value_t = false)]
        visualise_dag: bool,
    },
    /// Serve the site.
    Serve {
        /// Watch for changes (defaults to `true`).
        #[arg(short, long, default_value_t = true)]
        watch: bool,
        /// The port to serve the site on.
        #[arg(short, long, default_value_t = 80)]
        port: u16,
        /// The level of log output; recoverable errors, warnings, information, debugging information, and trace information.
        #[arg(short, long, action = clap::ArgAction::Count, default_value_t = 0)]
        verbosity: u8,
        /// Whether to visualise the DAG (defaults to `false`).
        #[arg(short = 'd', long, default_value_t = false)]
        visualise_dag: bool,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> miette::Result<()> {
    // miette::set_hook(Box::new(|_| {
    //     Box::new(
    //         miette::MietteHandlerOpts::new()
    //             .terminal_links(true)
    //             .unicode(true)
    //             .context_lines(3)
    //             .tab_width(4)
    //             .break_words(true)
    //             .with_cause_chain()
    //             .build(),
    //     )
    // }))?;
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Build {
            watch,
            verbosity,
            visualise_dag,
        }) => {
            let verbosity_level = match verbosity {
                0 => Level::ERROR,
                1 => Level::WARN,
                2 => Level::INFO,
                3 => Level::DEBUG,
                4 => Level::TRACE,
                _ => Level::TRACE,
            };
            tracing_subscriber::fmt()
                .pretty()
                .with_thread_ids(true)
                .with_thread_names(true)
                .with_file(true)
                .with_line_number(true)
                .with_max_level(verbosity_level)
                .init();
            info!("Building … ");
            tokio::spawn(build(watch, visualise_dag))
                .await
                .into_diagnostic()??;
        }
        Some(Commands::Serve {
            watch,
            port,
            verbosity,
            visualise_dag,
        }) => {
            let verbosity_level = match verbosity {
                0 => Level::ERROR,
                1 => Level::WARN,
                2 => Level::INFO,
                3 => Level::DEBUG,
                4 => Level::TRACE,
                _ => Level::TRACE,
            };
            tracing_subscriber::fmt()
                .pretty()
                .with_thread_ids(true)
                .with_thread_names(true)
                .with_file(true)
                .with_line_number(true)
                .with_max_level(verbosity_level)
                .init();
            println!("Serving on {}:{} … ", Ipv4Addr::UNSPECIFIED, port);
            tokio::spawn(build(watch, visualise_dag))
                .await
                .into_diagnostic()??;
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
                .bind((Ipv4Addr::UNSPECIFIED, port))
                .into_diagnostic()?
                .run(),
            )
            .await
            .into_diagnostic()?
            .into_diagnostic()?;
        }
        None => println!("Vox {}", crate_version!()),
    };
    Ok(())
}

fn insert_or_update_page(
    entry: PathBuf,
    layout_index: Option<NodeIndex>,
    dag: &mut StableDag<Page, EdgeType>,
    pages: &mut AHashMap<PathBuf, NodeIndex>,
    layouts: &mut AHashMap<NodeIndex, (PathBuf, PathBuf)>,
    locale: String,
) -> miette::Result<()> {
    let entry = fs::canonicalize(entry).into_diagnostic()?;
    let (page, index) = if !Page::is_layout_path(&entry)? {
        info!("Inserting or updating page: {:?} … ", entry);
        let page = path_to_page(entry.clone(), locale.clone())?;
        debug!("{:#?}", page);
        // If the page already exists in the DAG, update it. Otherwise, insert it.
        let index = if pages.contains_key(&entry) {
            info!("Updating page: {:?} … ", entry);
            let index = pages[&entry];
            let node = dag.node_weight_mut(index).unwrap();
            debug!("Old page: {:#?}", node);
            *node = page.clone();
            index
        } else {
            info!("Inserting page: {:?} … ", entry);
            let index = dag.add_node(page.clone());
            debug!("Noting index: {:?} … ", index);
            pages.insert(entry, index);
            debug!("Page inserted … ");
            index
        };
        (page, index)
    } else {
        info!("Inserting layout: {:?} … ", entry);
        let index = layout_index.unwrap();
        let page = dag.graph()[layout_index.unwrap()].clone();
        debug!("{:#?}", page);
        (page, index)
    };

    // A page's parents are pages in the collections it depends on. Its layout is a child.
    let layout = page.layout.clone();
    let collections = page.collections.clone();
    debug!("Layout: {:?} … ", layout);
    debug!("Collections: {:?} … ", collections);
    if let Some(layout) = layout {
        let layout_path = fs::canonicalize(format!("layouts/{}.vox", layout))
            .into_diagnostic()
            .with_context(|| format!("Layout not found: `layouts/{}.vox`", layout))?;
        // if pages.get(&layout_path).is_none() {
        //     info!("Inserting layout: {:?} … ", layout_path);
        //     let layout_page = path_to_page(layout_path.clone(), locale.clone())?;
        //     debug!("{:#?}", layout_page);
        //     let layout_index = dag.add_child(index, EdgeType::Layout, layout_page);
        //     pages.insert(layout_path, layout_index.1);
        // } else {
        //     info!(
        //         "Setting layout ({:?}) as child of {:?} … ",
        //         layout_path,
        //         page.to_path_string()
        //     );
        //     let layout_index = pages[&layout_path];
        //     dag.add_edge(index, layout_index, EdgeType::Layout)
        //         .into_diagnostic()?;
        // }
        // Layouts are inserted multiple times, once for each page that uses them.
        info!("Inserting layout: {:?} … ", layout_path);
        let layout_page = path_to_page(layout_path.clone(), locale.clone())?;
        debug!("{:#?}", layout_page);
        let layout_index = dag.add_child(index, EdgeType::Layout, layout_page);
        layouts.insert(layout_index.1, (page.to_path_string().into(), layout_path));
    }
    if let Some(collections) = collections {
        for collection in collections {
            let collection = fs::canonicalize(collection).into_diagnostic()?;
            for entry in
                glob(&format!("{}/**/*.vox", collection.to_string_lossy())).into_diagnostic()?
            {
                let entry = fs::canonicalize(entry.into_diagnostic()?).into_diagnostic()?;
                if pages.get(&entry).is_none() {
                    info!("Inserting collection page: {:?} … ", entry);
                    let collection_page = path_to_page(entry.clone(), locale.clone())?;
                    debug!("{:#?}", collection_page);
                    let collection_page_index =
                        dag.add_parent(index, EdgeType::Collection, collection_page);
                    pages.insert(entry, collection_page_index.1);
                } else {
                    info!(
                        "Setting collection page ({:?}) as parent of {:?} … ",
                        entry,
                        page.to_path_string()
                    );
                    let collection_page_index = pages[&entry];
                    dag.add_edge(collection_page_index, index, EdgeType::Collection)
                        .into_diagnostic()?;
                }
            }
        }
    }

    Ok(())
}

async fn build(watch: bool, visualise_dag: bool) -> miette::Result<()> {
    let parser = create_liquid_parser()?;
    let global = get_global_context()?;
    let mut dag = StableDag::new();
    let mut pages: AHashMap<PathBuf, NodeIndex> = AHashMap::new();
    let mut layouts: AHashMap<NodeIndex, (PathBuf, PathBuf)> = AHashMap::new();

    // Initial DAG construction.
    info!("Constructing DAG … ");
    for entry in glob("**/*.vox").into_diagnostic()? {
        let entry = fs::canonicalize(entry.into_diagnostic()?).into_diagnostic()?;
        // In the event that a layout has collection parents, we do not want it duplicated, so we avoid inserting it at first.
        if Page::is_layout_path(&entry)? {
            continue;
        }
        insert_or_update_page(
            entry,
            None,
            &mut dag,
            &mut pages,
            &mut layouts,
            global.1.clone(),
        )?;
    }
    // We update the layouts with their parents and children once all other pages have been inserted.
    for (layout, (_layout_parent_path, layout_path)) in layouts.clone() {
        insert_or_update_page(
            layout_path,
            Some(layout),
            &mut dag,
            &mut pages,
            &mut layouts,
            global.1.clone(),
        )?;
    }

    // Write the initial site to the output directory.
    info!("Performing initial build … ");
    let (_updated_pages, updated_dag) = tokio::spawn(async move {
        generate_site(
            parser.clone(),
            global.0.clone(),
            global.1.clone(),
            dag,
            visualise_dag,
        )
        .await
    })
    .await
    .into_diagnostic()??;
    dag = updated_dag;

    // Watch for changes to the site.
    info!("Watching for changes … ");
    if watch {
        let current_path = std::env::current_dir().into_diagnostic()?;
        let output_path = current_path.join("output");
        let git_path = current_path.join(".git");
        let (sender, receiver) = channel();
        let mut debouncer =
            new_debouncer(Duration::from_secs(1), None, sender).into_diagnostic()?;
        debouncer
            .watcher()
            .watch(&current_path, RecursiveMode::Recursive)
            .into_diagnostic()?;
        // Changes to the output directory or version control are irrelevant.
        debouncer.watcher().unwatch(&git_path).into_diagnostic()?;
        debouncer
            .watcher()
            .unwatch(&output_path)
            .into_diagnostic()?;

        loop {
            if let Ok(events) = receiver.recv().into_diagnostic()? {
                for event in events {
                    match event.kind {
                        // If a new page is created, insert it into the DAG.
                        EventKind::Create(_) => {
                            info!("New files created … ");
                            let parser = create_liquid_parser()?;
                            let global = get_global_context()?;
                            let page_paths: Vec<&PathBuf> = event
                                .paths
                                .iter()
                                .filter(|path| {
                                    path.exists()
                                        && path.is_file()
                                        && path.extension().unwrap_or_default() == "vox"
                                        && !Page::is_layout_path(path).unwrap()
                                })
                                .collect();
                            debug!("Pages created: {:?}", page_paths);
                            for path in page_paths {
                                insert_or_update_page(
                                    path.clone(),
                                    None,
                                    &mut dag,
                                    &mut pages,
                                    &mut layouts,
                                    global.1.clone(),
                                )?;
                            }
                            let (_updated_pages, updated_dag) = tokio::spawn(async move {
                                generate_site(
                                    parser.clone(),
                                    global.0.clone(),
                                    global.1.clone(),
                                    dag,
                                    visualise_dag,
                                )
                                .await
                            })
                            .await
                            .into_diagnostic()??;
                            dag = updated_dag;
                        }
                        EventKind::Modify(modify_kind) => match modify_kind {
                            ModifyKind::Name(rename_mode) => match rename_mode {
                                RenameMode::Both => {
                                    let parser = create_liquid_parser()?;
                                    let global = get_global_context()?;
                                    let from_path = fs::canonicalize(event.paths[0].clone())
                                        .into_diagnostic()?;
                                    let to_path = fs::canonicalize(event.paths[1].clone())
                                        .into_diagnostic()?;
                                    info!("Renaming occurred: {:?} → {:?}", from_path, to_path);
                                    // If the path is a file, update the page in the DAG.
                                    if to_path.is_file()
                                        && to_path.extension().unwrap_or_default() == "vox"
                                        && !Page::is_layout_path(&to_path)?
                                    {
                                        info!("Renaming page … ");
                                        let index = pages[&from_path];
                                        pages.remove(&from_path);
                                        dag.remove_node(index);
                                        insert_or_update_page(
                                            to_path.clone(),
                                            None,
                                            &mut dag,
                                            &mut pages,
                                            &mut layouts,
                                            global.1.clone(),
                                        )?;
                                    }
                                    // If the path is a directory, update all pages in the DAG.
                                    else if to_path.is_dir() {
                                        info!("Renaming directory … ");
                                        for (page_path, index) in pages.clone().into_iter() {
                                            if page_path.starts_with(&from_path) {
                                                let to_page_path = to_path.join(
                                                    page_path
                                                        .strip_prefix(&from_path)
                                                        .into_diagnostic()?,
                                                );
                                                pages.remove(&page_path);
                                                dag.remove_node(index);
                                                insert_or_update_page(
                                                    to_page_path,
                                                    None,
                                                    &mut dag,
                                                    &mut pages,
                                                    &mut layouts,
                                                    global.1.clone(),
                                                )?;
                                            }
                                        }
                                    };
                                    let (_updated_pages, updated_dag) = tokio::spawn(async move {
                                        generate_site(
                                            parser.clone(),
                                            global.0.clone(),
                                            global.1.clone(),
                                            dag,
                                            visualise_dag,
                                        )
                                        .await
                                    })
                                    .await
                                    .into_diagnostic()??;
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
                                            && !Page::is_layout_path(path).unwrap()
                                    })
                                    .collect();
                                info!("Pages were modified: {:#?}", page_paths);
                                for path in page_paths {
                                    insert_or_update_page(
                                        path.clone(),
                                        None,
                                        &mut dag,
                                        &mut pages,
                                        &mut layouts,
                                        global.1.clone(),
                                    )?;
                                }
                                let (_updated_pages, updated_dag) = tokio::spawn(async move {
                                    generate_site(
                                        parser.clone(),
                                        global.0.clone(),
                                        global.1.clone(),
                                        dag,
                                        visualise_dag,
                                    )
                                    .await
                                })
                                .await
                                .into_diagnostic()??;
                                dag = updated_dag;
                            }
                            _ => continue,
                        },
                        EventKind::Remove(remove_kind) => match remove_kind {
                            // If a folder is removed, remove all pages in the folder from the DAG.
                            RemoveKind::Folder => {
                                let parser = create_liquid_parser()?;
                                let global = get_global_context()?;
                                let path =
                                    fs::canonicalize(event.paths[0].clone()).into_diagnostic()?;
                                info!("Folder was removed: {:?}", path);
                                for (page_path, index) in pages.clone().into_iter() {
                                    if page_path.starts_with(&path) {
                                        pages.remove(&page_path);
                                        dag.remove_node(index);
                                    }
                                }
                                let (_updated_pages, updated_dag) = tokio::spawn(async move {
                                    generate_site(
                                        parser.clone(),
                                        global.0.clone(),
                                        global.1.clone(),
                                        dag,
                                        visualise_dag,
                                    )
                                    .await
                                })
                                .await
                                .into_diagnostic()??;
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
                                info!("Pages were removed: {:#?}", page_paths);
                                for path in page_paths {
                                    let path = fs::canonicalize(path).into_diagnostic()?;
                                    if let Some(index) = pages.get(&path) {
                                        dag.remove_node(*index);
                                        pages.remove(&path);
                                    }
                                }
                                let (_updated_pages, updated_dag) = tokio::spawn(async move {
                                    generate_site(
                                        parser.clone(),
                                        global.0.clone(),
                                        global.1.clone(),
                                        dag,
                                        visualise_dag,
                                    )
                                    .await
                                })
                                .await
                                .into_diagnostic()??;
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

fn get_layout_url(
    layout_node_index: &NodeIndex,
    dag: &StableDag<Page, EdgeType>,
) -> Option<String> {
    let layout_node = dag.graph()[*layout_node_index].clone();
    if !layout_node.url.is_empty() {
        return Some(layout_node.url);
    }

    let parents = dag
        .parents(*layout_node_index)
        .iter(dag)
        .collect::<Vec<_>>();
    let mut result = String::new();
    for parent in parents {
        if *dag.edge_weight(parent.0).unwrap() != EdgeType::Layout {
            continue;
        }
        result = get_layout_url(&parent.1, dag)?;
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

async fn generate_site(
    template_parser: liquid::Parser,
    contexts: liquid::Object,
    locale: String,
    dag: StableDag<Page, EdgeType>,
    visualise_dag: bool,
) -> miette::Result<(Vec<NodeIndex>, StableDag<Page, EdgeType>)> {
    let mut timer = Stopwatch::start_new();
    let mut build = Build {
        template_parser,
        contexts,
        locale,
        dag,
    };
    let updated_pages = build.render_all(visualise_dag)?;
    info!("{} pages were rendered … ", updated_pages.len());
    for updated_page_index in updated_pages.iter() {
        let updated_page = &build.dag.graph()[*updated_page_index];
        // If a page has no URL, it may be a layout.
        // Layouts contain rendered content but must be written using their parent's URL.
        let output_path = if updated_page.url.is_empty() {
            // let mut output_path = None;
            // let parents = build
            //     .dag
            //     .parents(*updated_page_index)
            //     .iter(&build.dag)
            //     .collect::<Vec<_>>();
            // for parent in parents {
            //     if *build.dag.edge_weight(parent.0).unwrap() != EdgeType::Layout {
            //         continue;
            //     }
            //     let parent = &build.dag.graph()[parent.1];
            //     if !parent.url.is_empty() {
            //         output_path = Some(format!("output/{}", parent.url));
            //         break;
            //     }
            // }
            let layout_url = get_layout_url(updated_page_index, &build.dag);
            layout_url.map(|layout_url| format!("output/{}", layout_url))
        } else if !updated_page.url.is_empty() {
            Some(format!("output/{}", updated_page.url))
        } else {
            None
        };
        if output_path.is_none() {
            warn!("Page has no URL: {:#?} … ", updated_page.to_path_string());
            continue;
        }
        let output_path = output_path.unwrap();
        info!("Writing to {} … ", output_path);
        tokio::fs::create_dir_all(
            Path::new(&output_path)
                .parent()
                .unwrap_or(Path::new(&output_path)),
        )
        .await
        .into_diagnostic()?;
        tokio::fs::write(output_path, updated_page.rendered.clone())
            .await
            .into_diagnostic()?;
    }
    timer.stop();
    println!(
        "Generated {} pages in {:.2} seconds … ",
        updated_pages.len(),
        timer.elapsed_s()
    );
    Ok((updated_pages, build.dag))
}

fn path_to_page(path: PathBuf, locale: String) -> miette::Result<Page> {
    Page::new(
        fs::read_to_string(path.clone()).into_diagnostic()?,
        path,
        locale,
    )
}

fn get_global_context() -> miette::Result<(Object, String)> {
    let global_context = match fs::read_to_string("global.toml") {
        Ok(global_file) => global_file.parse::<Table>().into_diagnostic()?,
        Err(_) => format!("locale = '{}'", date::default_locale_string())
            .parse::<Table>()
            .into_diagnostic()?,
    };
    let locale: String = global_context
        .get("locale")
        .unwrap_or(&toml::Value::String(date::default_locale_string()))
        .as_str()
        .unwrap_or(&date::default_locale_string())
        .to_string();
    Ok((
        object!({
            "global": global_context
        }),
        locale,
    ))
}
