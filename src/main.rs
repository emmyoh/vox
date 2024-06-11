use actix_files::NamedFile;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::{App, HttpServer};
use ahash::{AHashMap, AHashSet, HashSet, HashSetExt};
use chrono::{Locale, Utc};
use clap::{arg, crate_version};
use clap::{Parser, Subcommand};
use daggy::petgraph::algo::toposort;
use daggy::Walker;
use daggy::{stable_dag::StableDag, NodeIndex};
use glob::glob;
use liquid::{object, Object};
use miette::{Context, IntoDiagnostic};
use mimalloc::MiMalloc;
use notify_debouncer_full::{
    new_debouncer,
    notify::{RecursiveMode, Watcher},
};
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::mpsc::channel;
use std::{fs, path::PathBuf, time::Duration};
use syntect::highlighting::ThemeSet;
use syntect::html::css_for_theme_with_class_style;
use ticky::Stopwatch;
use tokio::time::sleep;
use toml::Table;
use tracing::{debug, error, info, trace, warn, Level};
use vox::builds::EdgeType;
use vox::date::{self, Date};
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
        /// An optional path to the site directory.
        #[arg(default_value = None)]
        path: Option<PathBuf>,
        /// Watch for changes.
        #[arg(short, long, default_value_t = false)]
        watch: bool,
        /// The level of log output; warnings, information, debugging messages, and trace logs.
        #[arg(short, long, action = clap::ArgAction::Count, default_value_t = 0)]
        verbosity: u8,
        /// Visualise the DAG.
        #[arg(short = 'd', long, default_value_t = false)]
        visualise_dag: bool,
        /// Generate stylesheet for syntax highlighting.
        #[arg(short = 's', long, default_value_t = false)]
        generate_syntax_css: bool,
    },
    /// Serve the site.
    Serve {
        /// An optional path to the site directory.
        #[arg(default_value = None)]
        path: Option<PathBuf>,
        /// Watch for changes.
        #[arg(short, long, default_value_t = false)]
        watch: bool,
        /// The port to serve the site on.
        #[arg(short, long, default_value_t = 80)]
        port: u16,
        /// The level of log output; warnings, information, debugging messages, and trace logs.
        #[arg(short, long, action = clap::ArgAction::Count, default_value_t = 0)]
        verbosity: u8,
        /// Visualise the DAG.
        #[arg(short = 'd', long, default_value_t = false)]
        visualise_dag: bool,
        /// Generate stylesheet for syntax highlighting.
        #[arg(short = 's', long, default_value_t = false)]
        generate_syntax_css: bool,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> miette::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Build {
            path,
            watch,
            verbosity,
            visualise_dag,
            generate_syntax_css,
        }) => {
            if let Some(path) = path {
                std::env::set_current_dir(path).into_diagnostic()?;
            }
            let verbosity_level = match verbosity {
                0 => Level::ERROR,
                1 => Level::WARN,
                2 => Level::INFO,
                3 => Level::DEBUG,
                4 => Level::TRACE,
                _ => Level::TRACE,
            };
            let mut subscriber_builder = tracing_subscriber::fmt()
                .with_env_filter("vox")
                .pretty()
                .with_max_level(verbosity_level)
                .with_file(false)
                .with_line_number(false);
            if verbosity >= 3 {
                subscriber_builder = subscriber_builder
                    .with_thread_ids(true)
                    .with_thread_names(true)
                    .with_file(true)
                    .with_line_number(true);
            }
            subscriber_builder.init();
            info!("Building … ");
            let build_loop = tokio::spawn(async move {
                loop {
                    let building = tokio::spawn(build(watch, visualise_dag, generate_syntax_css));
                    match building.await.unwrap() {
                        Ok(_) => {
                            if !watch {
                                break;
                            }
                        }
                        Err(err) => {
                            error!("Building failed: {:#?}", err);
                            info!("Retrying in 5 seconds … ");
                            sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                    }
                }
            });
            build_loop.await.into_diagnostic()?;
        }
        Some(Commands::Serve {
            path,
            watch,
            port,
            verbosity,
            visualise_dag,
            generate_syntax_css,
        }) => {
            if let Some(path) = path {
                std::env::set_current_dir(path).into_diagnostic()?;
            }
            let verbosity_level = match verbosity {
                0 => Level::ERROR,
                1 => Level::WARN,
                2 => Level::INFO,
                3 => Level::DEBUG,
                4 => Level::TRACE,
                _ => Level::TRACE,
            };
            let mut subscriber_builder = tracing_subscriber::fmt()
                .pretty()
                .with_max_level(verbosity_level)
                .with_file(false)
                .with_line_number(false);
            if verbosity >= 3 {
                subscriber_builder = subscriber_builder
                    .with_thread_ids(true)
                    .with_thread_names(true)
                    .with_file(true)
                    .with_line_number(true);
            }
            subscriber_builder.init();
            let build_loop = tokio::spawn(async move {
                loop {
                    let building = tokio::spawn(build(watch, visualise_dag, generate_syntax_css));
                    match building.await.unwrap() {
                        Ok(_) => {
                            if !watch {
                                break;
                            }
                        }
                        Err(err) => {
                            error!("Building failed: {:#?}", err);
                            info!("Retrying in 5 seconds … ");
                            sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                    }
                }
            });
            let serve_loop = tokio::spawn(async move {
                loop {
                    let serving = tokio::spawn(
                        HttpServer::new(|| {
                            let mut service = actix_files::Files::new("/", "output")
                                .prefer_utf8(true)
                                .use_hidden_files()
                                .use_etag(true)
                                .use_last_modified(true)
                                .show_files_listing()
                                .redirect_to_slash_directory();
                            service = service.index_file("index.html");
                            service = service.default_handler(|req: ServiceRequest| {
                                let (http_req, _payload) = req.into_parts();
                                async {
                                    let response = NamedFile::open("output/404.html")?
                                        .into_response(&http_req);
                                    Ok(ServiceResponse::new(http_req, response))
                                }
                            });
                            App::new().service(service)
                        })
                        .bind((Ipv4Addr::UNSPECIFIED, port))
                        .unwrap()
                        .run(),
                    );
                    println!("Serving on {}:{} … ", Ipv4Addr::UNSPECIFIED, port);
                    match serving.await.unwrap() {
                        Ok(_) => {}
                        Err(err) => {
                            error!("Serving failed: {:#?}", err);
                            info!("Retrying in 5 seconds … ");
                            sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                    }
                }
            });
            tokio::spawn(async move {
                match tokio::signal::ctrl_c().await {
                    Ok(()) => {
                        info!("Exiting … ");
                        std::process::exit(0);
                    }
                    Err(err) => {
                        error!("Unable to listen for shutdown signal: {}", err);
                        std::process::exit(0);
                    }
                }
            });
            build_loop.await.into_diagnostic()?;
            serve_loop.await.into_diagnostic()?;
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
    layouts: &mut AHashMap<PathBuf, HashSet<NodeIndex>>,
    collection_dependents: &mut AHashMap<String, HashSet<NodeIndex>>,
    collection_members: &mut AHashMap<String, HashSet<NodeIndex>>,
    locale: Locale,
) -> miette::Result<()> {
    let entry = fs::canonicalize(entry).into_diagnostic()?;
    let (page, index) = if !Page::is_layout_path(&entry)? {
        debug!("Inserting or updating page: {:?} … ", entry);
        let page = path_to_page(entry.clone(), locale)?;
        // If the page already exists in the DAG, update it. Otherwise, insert it.
        let index = if pages.contains_key(&entry) {
            debug!("Updating page: {:?} … ", entry);
            let index = pages[&entry];
            let node = dag.node_weight_mut(index).unwrap();
            *node = page.clone();
            index
        } else {
            debug!("Inserting page: {:?} … ", entry);
            let index = dag.add_node(page.clone());
            pages.insert(entry, index);
            index
        };
        (page, index)
    } else {
        debug!("Inserting layout: {:?} … ", entry);
        let index = layout_index.unwrap();
        let page = dag.graph()[layout_index.unwrap()].clone();
        // debug!("{:#?}", page);
        (page, index)
    };

    // A page's parents are pages in the collections it depends on. Its layout is a child.
    let layout = page.layout.clone();
    let collections = page.collections.clone();
    let depends = page.depends.clone();
    debug!("Layout used: {:?} … ", layout);
    debug!("Collections used: {:?} … ", depends);
    if let Some(layout) = layout {
        // Layouts are inserted multiple times, once for each page that uses them.
        let layout_path = fs::canonicalize(format!("layouts/{}.vox", layout))
            .into_diagnostic()
            .with_context(|| format!("Layout not found: `layouts/{}.vox`", layout))?;
        let children = dag.children(index).iter(dag).collect::<Vec<_>>();
        // If this page is being updated, the old layout should be replaced with the current one in the DAG.
        let old_layout = children
            .iter()
            .find(|child| *dag.edge_weight(child.0).unwrap() == EdgeType::Layout);
        if let Some(old_layout) = old_layout {
            trace!("Removing old layout … ");
            dag.remove_node(old_layout.1);
        }
        debug!("Inserting layout: {:?} … ", layout_path);
        let layout_page = path_to_page(layout_path.clone(), locale)?;
        let layout_index = dag.add_child(index, EdgeType::Layout, layout_page);
        if let Some(layouts) = layouts.get_mut(&layout_path) {
            layouts.insert(layout_index.1);
        } else {
            let mut new_set = HashSet::new();
            new_set.insert(layout_index.1);
            layouts.insert(layout_path.clone(), new_set);
        }
    }
    if let Some(collections) = collections {
        for collection in collections {
            if let Some(collection_members) = collection_members.get_mut(&collection) {
                collection_members.insert(index);
            } else {
                let mut new_set = HashSet::new();
                new_set.insert(index);
                collection_members.insert(collection.clone(), new_set);
            }
        }
    }
    if let Some(depends) = depends {
        for collection in depends {
            if let Some(collection_dependents) = collection_dependents.get_mut(&collection) {
                collection_dependents.insert(index);
            } else {
                let mut new_set = HashSet::new();
                new_set.insert(index);
                collection_dependents.insert(collection.clone(), new_set);
            }
        }
    }

    Ok(())
}

async fn build(watch: bool, visualise_dag: bool, generate_syntax_css: bool) -> miette::Result<()> {
    let parser = create_liquid_parser()?;
    let global = get_global_context()?;
    let mut dag = StableDag::new();
    let mut pages: AHashMap<PathBuf, NodeIndex> = AHashMap::new();
    let mut layouts: AHashMap<PathBuf, HashSet<NodeIndex>> = AHashMap::new();
    let mut collection_dependents: AHashMap<String, HashSet<NodeIndex>> = AHashMap::new();
    let mut collection_members: AHashMap<String, HashSet<NodeIndex>> = AHashMap::new();

    // Initial DAG construction.
    debug!("Constructing DAG … ");
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
            &mut collection_dependents,
            &mut collection_members,
            global.1,
        )?;
    }
    // We update the layouts with their parents and children once all other pages have been inserted.
    for (layout_path, layout_indices) in layouts.clone() {
        for layout_index in layout_indices {
            insert_or_update_page(
                layout_path.clone(),
                Some(layout_index),
                &mut dag,
                &mut pages,
                &mut layouts,
                &mut collection_dependents,
                &mut collection_members,
                global.1,
            )?;
        }
    }
    // We construct edges between collection members and dependents.
    for (collection, members) in collection_members {
        if let Some(dependents) = collection_dependents.get(&collection) {
            for member in members {
                for dependent in dependents {
                    dag.add_edge(member, *dependent, EdgeType::Collection)
                        .into_diagnostic()?;
                }
            }
        }
    }

    // Write the initial site to the output directory.
    info!("Performing initial build … ");
    let (_updated_pages, updated_dag) = generate_site(
        parser.clone(),
        global.0.clone(),
        global.1,
        dag,
        visualise_dag,
        generate_syntax_css,
    )
    .await?;
    dag = updated_dag;

    // Watch for changes to the site.
    if watch {
        let current_path = std::env::current_dir().into_diagnostic()?;
        let output_path = current_path.join("output");
        let git_path = current_path.join(".git");
        let (sender, receiver) = channel();
        let mut debouncer =
            new_debouncer(Duration::from_secs(1), None, sender).into_diagnostic()?;
        info!("Watching {:?} … ", current_path);
        debouncer
            .watcher()
            .watch(&current_path, RecursiveMode::Recursive)
            .into_diagnostic()?;

        let mut global_or_snippets_changed = false;
        loop {
            if let Ok(events) = receiver.recv().into_diagnostic()? {
                // Changes to the output directory or version control are irrelevant.
                if !events.iter().any(|event| {
                    event
                        .paths
                        .iter()
                        .any(|path| !path.starts_with(&output_path) && !path.starts_with(&git_path))
                }) {
                    continue;
                }
                global_or_snippets_changed = events.iter().any(|event| {
                    event.paths.iter().any(|path| {
                        path.strip_prefix(current_path.clone())
                            .unwrap_or(path)
                            .starts_with("global.toml")
                            || path
                                .strip_prefix(current_path.clone())
                                .unwrap_or(path)
                                .starts_with("snippets/")
                    })
                });
                trace!(
                    "Changes detected: {:#?} … ",
                    events
                        .into_iter()
                        .map(|event| event
                            .paths
                            .clone()
                            .into_iter()
                            .map(|path| {
                                path.strip_prefix(current_path.clone())
                                    .unwrap_or(&path)
                                    .to_path_buf()
                            })
                            .collect::<Vec<_>>())
                        .collect::<Vec<_>>()
                );
            }

            // 1. Build a new DAG.
            let parser = create_liquid_parser()?;
            let global = get_global_context()?;
            let mut new_dag = StableDag::new();
            let mut new_pages: AHashMap<PathBuf, NodeIndex> = AHashMap::new();
            let mut new_layouts: AHashMap<PathBuf, HashSet<NodeIndex>> = AHashMap::new();
            let mut new_collection_dependents: AHashMap<String, HashSet<NodeIndex>> =
                AHashMap::new();
            let mut new_collection_members: AHashMap<String, HashSet<NodeIndex>> = AHashMap::new();

            // New DAG construction.
            debug!("Constructing DAG … ");
            for entry in glob("**/*.vox").into_diagnostic()? {
                let entry = fs::canonicalize(entry.into_diagnostic()?).into_diagnostic()?;
                if Page::is_layout_path(&entry)? {
                    continue;
                }
                insert_or_update_page(
                    entry,
                    None,
                    &mut new_dag,
                    &mut new_pages,
                    &mut new_layouts,
                    &mut new_collection_dependents,
                    &mut new_collection_members,
                    global.1,
                )?;
            }
            for (layout_path, layout_indices) in new_layouts.clone() {
                for layout_index in layout_indices {
                    insert_or_update_page(
                        layout_path.clone(),
                        Some(layout_index),
                        &mut new_dag,
                        &mut new_pages,
                        &mut new_layouts,
                        &mut new_collection_dependents,
                        &mut new_collection_members,
                        global.1,
                    )?;
                }
            }
            for (collection, members) in new_collection_members {
                if let Some(dependents) = new_collection_dependents.get(&collection) {
                    for member in members {
                        for dependent in dependents {
                            new_dag
                                .add_edge(member, *dependent, EdgeType::Collection)
                                .into_diagnostic()?;
                        }
                    }
                }
            }

            // 2. Obtain the difference between the old and new DAGs; ie, calculate the set of added or modified nodes.
            //     - A node is modified if it has the same label, but its page is different (not comparing `url` or `rendered`).
            //         - If a node's page is the same (excluding `url` or `rendered`), it is unchanged.
            //     - A node is added if its label appears in the new DAG, but not the old one.
            //     - A node is removed if its label appears in the old DAG, but not the new one.

            let mut old_dag_pages = AHashMap::new();
            for (page_path, page_index) in &pages {
                let page = dag.node_weight(*page_index).unwrap();
                old_dag_pages.insert(page_path.clone(), page);
            }
            let mut new_dag_pages = AHashMap::new();
            for (page_path, page_index) in &new_pages {
                let page = new_dag.node_weight(*page_index).unwrap();
                new_dag_pages.insert(page_path.clone(), page);
            }
            let mut added_or_modified = AHashSet::new();
            let mut removed = AHashSet::new();
            let mut removed_output_paths = AHashSet::new();
            for (page_path, new_page) in new_dag_pages.iter() {
                if let Some(old_page) = old_dag_pages.get(page_path) {
                    // If the page has been modified, its index is noted.
                    if !new_page.is_equivalent(old_page) {
                        added_or_modified.insert(new_pages[page_path]);
                    }
                } else {
                    // If the page is new, its index is noted.
                    added_or_modified.insert(new_pages[page_path]);
                }
            }
            // The ancestors of modified or added layouts are themselves modified or added.
            for (layout_path, new_layout_indices) in &new_layouts {
                let new_layout = new_dag
                    .node_weight(*new_layout_indices.iter().last().unwrap())
                    .unwrap();
                if let Some(old_layout_indices) = layouts.get(layout_path) {
                    let old_layout = dag
                        .node_weight(*old_layout_indices.iter().last().unwrap())
                        .unwrap();
                    // Layout has been modified.
                    if !new_layout.is_equivalent(old_layout) {
                        for new_layout_index in new_layout_indices {
                            let ancestors =
                                Build::get_non_layout_ancestors(&new_dag, *new_layout_index)?;
                            for ancestor in ancestors {
                                added_or_modified.insert(ancestor);
                            }
                        }
                    }
                } else {
                    // Layout is new.
                    for new_layout_index in new_layout_indices {
                        let ancestors =
                            Build::get_non_layout_ancestors(&new_dag, *new_layout_index)?;
                        for ancestor in ancestors {
                            added_or_modified.insert(ancestor);
                        }
                    }
                }
            }
            // The ancestors of removed layouts are modified.
            for (layout_path, old_layout_indices) in &layouts {
                if new_layouts.get(layout_path).is_none() {
                    for old_layout_index in old_layout_indices {
                        let ancestors = Build::get_non_layout_ancestors(&dag, *old_layout_index)?;
                        let ancestor_paths = ancestors
                            .iter()
                            .map(|ancestor| {
                                PathBuf::from(dag.node_weight(*ancestor).unwrap().to_path_string())
                            })
                            .collect::<Vec<_>>();
                        for ancestor_path in ancestor_paths {
                            if let Some(ancestor_index) = new_pages.get(&ancestor_path) {
                                added_or_modified.insert(*ancestor_index);
                            }
                        }
                    }
                }
            }
            for (page_path, _old_page) in old_dag_pages.iter() {
                if new_dag_pages.get(page_path).is_none() {
                    // If the page has been removed, its index is noted.
                    removed.insert(pages[page_path]);
                    if let Some(old_page) = old_dag_pages.get(page_path) {
                        let output_path = if old_page.url.is_empty() {
                            let layout_url = get_layout_url(&pages[page_path], &dag);
                            layout_url.map(|layout_url| format!("output/{}", layout_url))
                        } else if !old_page.url.is_empty() {
                            Some(format!("output/{}", old_page.url))
                        } else {
                            None
                        };
                        if output_path.is_none() {
                            warn!("Page has no URL: {:#?} … ", old_page.to_path_string());
                            continue;
                        }
                        let output_path = output_path.unwrap();
                        removed_output_paths
                            .insert(fs::canonicalize(output_path.clone()).into_diagnostic()?);
                    }
                }
            }
            debug!("Removed pages: {:#?} … ", removed_output_paths);
            // No need to continue if nothing changed.
            if !global_or_snippets_changed
                && added_or_modified.is_empty()
                && removed.is_empty()
                && removed_output_paths.is_empty()
            {
                info!("Nothing changed. Aborting rebuild … ");
                continue;
            }

            // 3. Compute which pages need to be rendered, noting their node IDs.
            //     - All pages that were modified need to be re-rendered.
            //         - Their descendants in the new DAG also need to be rendered.
            //     - All pages that were added need to be rendered.
            //         - Their descendants in the new DAG also need to be rendered.
            //     - All pages that were removed need their descendants in the new DAG rendered.
            //         - Their old output also needs to be deleted.

            let mut pages_to_render = added_or_modified.clone();
            // If the global context or snippets have changed, all pages need to be re-rendered.
            if global_or_snippets_changed {
                pages_to_render.extend(new_pages.values());
                pages_to_render.extend(new_layouts.values().flatten());
            }
            for page_index in added_or_modified.clone() {
                let descendants = Build::get_descendants(&new_dag, page_index);
                for descendant in descendants {
                    pages_to_render.insert(descendant);
                }
            }
            for page_index in removed.clone() {
                let descendants = Build::get_descendants(&dag, page_index);
                let descendant_page_paths = descendants
                    .iter()
                    .map(|descendant| {
                        PathBuf::from(dag.node_weight(*descendant).unwrap().to_path_string())
                    })
                    .collect::<Vec<_>>();
                for descendant_page_path in descendant_page_paths {
                    if let Some(descendant_page_index) = new_pages.get(&descendant_page_path) {
                        pages_to_render.insert(*descendant_page_index);
                    }
                }
            }
            // Only the root pages need to be passed to the rendering code, as it will recursively render their descendants.
            for page_index in removed.clone() {
                let children = dag.children(page_index).iter(&dag).collect::<Vec<_>>();
                let child_page_paths = children
                    .iter()
                    .map(|child| PathBuf::from(dag.node_weight(child.1).unwrap().to_path_string()))
                    .collect::<Vec<_>>();
                for child_page_path in child_page_paths {
                    if let Some(child_page_index) = new_pages.get(&child_page_path) {
                        pages_to_render.insert(*child_page_index);
                    }
                }
            }

            // 4. Merge the DAGs.
            //     - In the new DAG, replace all pages not needing rendering with their rendered counterparts from the old DAG.

            for (page_path, page_index) in &new_pages {
                if !pages_to_render.contains(page_index) {
                    // Pages may be added, so it is necessary to check if the page already exists in the old DAG.
                    if let Some(old_page) = dag.node_weight(pages[page_path]) {
                        let new_page = new_dag.node_weight_mut(*page_index).unwrap();
                        new_page.url.clone_from(&old_page.url);
                        new_page.rendered.clone_from(&old_page.rendered);
                    }
                }
            }
            dag = new_dag;
            trace!("Merged DAGs … ");

            // 5. Render & output the appropriate pages.
            info!("Rebuilding … ");
            let mut timer = Stopwatch::start_new();
            let mut build = Build {
                template_parser: parser,
                contexts: global.0,
                locale: global.1,
                dag,
            };
            if visualise_dag {
                build.visualise_dag()?;
            }

            // Delete the output of removed pages.
            for removed_output_path in removed_output_paths {
                debug!("Removing {:?} … ", removed_output_path);
                tokio::fs::remove_file(removed_output_path)
                    .await
                    .into_diagnostic()?;
            }

            let mut rendered_pages = Vec::new();
            let render_order = toposort(&build.dag.graph(), None).unwrap_or_default();
            for page in render_order {
                if pages_to_render.contains(&page) {
                    build.render_page(page, false, &mut rendered_pages)?;
                }
            }

            for updated_page_index in rendered_pages.iter() {
                let updated_page = &build.dag.graph()[*updated_page_index];
                let output_path = get_output_path(updated_page, updated_page_index, &build);
                if output_path.is_none() {
                    warn!("Page has no URL: {:#?} … ", updated_page.to_path_string());
                    continue;
                }
                let output_path = output_path.unwrap();
                info!(
                    "Writing `{}` to `{}` … ",
                    updated_page.to_path_string(),
                    output_path
                );
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
            if generate_syntax_css {
                generate_syntax_stylesheets()?;
            }
            timer.stop();
            println!(
                "Generated {} pages in {:.2} seconds … ",
                rendered_pages.len(),
                timer.elapsed_s()
            );
            dag = build.dag;
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

fn get_output_path(page: &Page, page_index: &NodeIndex, build: &Build) -> Option<String> {
    // If a page has no URL, it may be a layout.
    // Layouts contain rendered content but must be written using their parent's URL.

    if page.url.is_empty() {
        let layout_url = get_layout_url(page_index, &build.dag);
        layout_url.map(|layout_url| format!("output/{}", layout_url))
    } else if !page.url.is_empty() {
        Some(format!("output/{}", page.url))
    } else {
        None
    }
}

async fn generate_site(
    template_parser: liquid::Parser,
    contexts: liquid::Object,
    locale: Locale,
    dag: StableDag<Page, EdgeType>,
    visualise_dag: bool,
    generate_syntax_css: bool,
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
        let output_path = get_output_path(updated_page, updated_page_index, &build);
        if output_path.is_none() {
            warn!("Page has no URL: {:#?} … ", updated_page.to_path_string());
            continue;
        }
        let output_path = output_path.unwrap();
        info!(
            "Writing `{}` to `{}` … ",
            updated_page.to_path_string(),
            output_path
        );
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
    if generate_syntax_css {
        generate_syntax_stylesheets()?;
    }
    timer.stop();
    println!(
        "Generated {} pages in {:.2} seconds … ",
        updated_pages.len(),
        timer.elapsed_s()
    );
    Ok((updated_pages, build.dag))
}

fn path_to_page(path: PathBuf, locale: Locale) -> miette::Result<Page> {
    Page::new(
        fs::read_to_string(path.clone()).into_diagnostic()?,
        path,
        locale,
    )
}

fn get_global_context() -> miette::Result<(Object, Locale)> {
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
    let locale = date::locale_string_to_locale(locale.clone());
    let current_date = Date::chrono_to_date(Utc::now(), locale);
    Ok((
        object!({
            "global": global_context,
            "meta": {
                "builder": "Vox",
                "version": crate_version!(),
                "date": current_date,
            }
        }),
        locale,
    ))
}

/// Generate stylesheets for syntax highlighting.
fn generate_syntax_stylesheets() -> miette::Result<()> {
    let css_path = PathBuf::from("output/css/");
    let dark_css_path = css_path.join("dark-code.css");
    let light_css_path = css_path.join("light-code.css");
    let code_css_path = css_path.join("code.css");
    std::fs::create_dir_all(css_path).into_diagnostic()?;

    let ts = ThemeSet::load_defaults();
    let dark_theme = &ts.themes["base16-ocean.dark"];
    let css_dark = css_for_theme_with_class_style(dark_theme, syntect::html::ClassStyle::Spaced)
        .into_diagnostic()?;
    std::fs::write(dark_css_path, css_dark).into_diagnostic()?;

    let light_theme = &ts.themes["base16-ocean.light"];
    let css_light = css_for_theme_with_class_style(light_theme, syntect::html::ClassStyle::Spaced)
        .into_diagnostic()?;
    std::fs::write(light_css_path, css_light).into_diagnostic()?;

    let css = r#"@import url("light-code.css") (prefers-color-scheme: light);@import url("dark-code.css") (prefers-color-scheme: dark);"#;
    std::fs::write(code_css_path, css).into_diagnostic()?;
    Ok(())
}
