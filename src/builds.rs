use crate::page::Page;
use ahash::AHashMap;
use daggy::{
    petgraph::{
        dot::{Config, Dot},
        Direction,
    },
    stable_dag::StableDag,
    NodeIndex, Walker,
};
use layout::gv::DotParser;
use layout::gv::GraphBuilder;
use layout::{backends::svg::SVGWriter, core::color::Color, std_shapes::shapes::ShapeKind};
use liquid::{to_object, Object, Parser};
use liquid_core::to_value;
use miette::IntoDiagnostic;
use std::{env, fs, path::PathBuf};
use tracing::{debug, info, trace, warn};

/// Information held in memory while performing a build.
#[derive(Clone, Default)]
pub struct Build {
    /// A Liquid template parser.
    pub template_parser: Parser,
    /// The Liquid contexts necessary to render templates in pages.
    pub contexts: Object,
    /// The locale information of the build, primarily used to render dates and times.
    pub locale: String,
    /// A directed acyclic graph (DAG) populated with pages and their children.
    pub dag: StableDag<Page, EdgeType>,
}

/// The type of edge in the DAG.
#[derive(PartialEq, Clone, Debug)]
pub enum EdgeType {
    /// An edge between a layout and its parent page.
    Layout,
    /// An edge between a page and its parent collection page.
    Collection,
}

impl Build {
    /// Visualise the DAG.
    pub fn visualise_dag(&mut self) -> miette::Result<()> {
        let dag_graph = self.dag.graph();
        let dag_graphviz = Dot::with_attr_getters(
            dag_graph,
            &[Config::NodeNoLabel, Config::EdgeNoLabel],
            &|_graph, edge| format!("label = \"{:?}\"", edge.weight()),
            &|_graph, node| format!("label = \"{}\"", node.1.to_path_string()),
        );
        debug!("DAG: {:#?}", dag_graphviz);
        let mut parser = DotParser::new(&format!("{:?}", dag_graphviz));
        let tree = parser.process();
        if let Ok(tree) = tree {
            let mut gb = GraphBuilder::new();
            gb.visit_graph(&tree);
            let mut vg = gb.get();
            let mut svg = SVGWriter::new();
            for node_handle in vg.iter_nodes() {
                let node = vg.element_mut(node_handle);
                let old_shape = node.shape.clone();
                if let ShapeKind::Circle(label) = old_shape {
                    node.shape = ShapeKind::Box(label);
                    node.look.fill_color = Some(Color::fast("#FFDFBA"));
                }
            }
            vg.do_it(false, false, false, &mut svg);
            let content = svg.finalize();
            std::fs::create_dir_all("output").into_diagnostic()?;
            std::fs::write("output/dag.svg", content).into_diagnostic()?;
        } else {
            warn!("Unable to visualise the DAG.")
        }
        Ok(())
    }

    /// Render all pages in the DAG.
    ///
    /// # Returns
    ///
    /// A list of all nodes that were rendered.
    pub fn render_all(&mut self, visualise_dag: bool) -> miette::Result<Vec<NodeIndex>> {
        info!("Rendering all pages … ");
        if visualise_dag {
            self.visualise_dag()?;
        }
        let mut rendered_indices = Vec::new();
        let root_indices = self.find_root_indices();
        debug!("Root indices: {:?}", root_indices);
        for root_index in root_indices {
            self.render_recursively(root_index, &mut rendered_indices)?;
        }
        rendered_indices.dedup();
        Ok(rendered_indices)
    }

    /// Render a page and its child pages.
    ///
    /// # Arguments
    ///
    /// * `root_index` - The index of the page in the DAG.
    ///
    /// * `rendered_indices` - A list of all nodes that have already been rendered.
    ///
    /// # Returns
    ///
    /// A list of all nodes that were rendered.
    pub fn render_recursively(
        &mut self,
        root_index: NodeIndex,
        rendered_indices: &mut Vec<NodeIndex>,
    ) -> miette::Result<()> {
        // // If the page has already been rendered, return early to avoid rendering it again.
        // // Since a page can be a child of multiple pages, a render call can be made multiple times.
        // if rendered_indices.contains(&root_index) {
        //     debug!(
        //         "Page already rendered: `{}`. Skipping … ",
        //         self.dag.graph()[root_index].to_path_string()
        //     );
        //     // return Ok(());
        // }
        let current_directory =
            fs::canonicalize(env::current_dir().into_diagnostic()?).into_diagnostic()?;
        let root_page = self.dag.graph()[root_index].to_owned();
        let root_path = fs::canonicalize(root_page.to_path_string()).into_diagnostic()?;
        let root_path_difference = root_path
            .strip_prefix(&current_directory)
            .into_diagnostic()?;
        info!("Rendering page: {:?}", root_path_difference);
        debug!("{:#?}", root_page);
        let mut root_contexts = self.contexts.clone();
        if root_path_difference.starts_with(PathBuf::from("layouts/")) {
            trace!("Page is a layout page … ");
            let layout_object =
                liquid_core::Value::Object(to_object(&root_page).into_diagnostic()?);
            root_contexts.insert("layout".into(), layout_object.clone());
        } else {
            trace!("Page is not a layout page … ");
            let page_object = liquid_core::Value::Object(to_object(&root_page).into_diagnostic()?);
            root_contexts.insert("page".into(), page_object.clone());
        }
        let mut collection_pages: AHashMap<String, Vec<NodeIndex>> = AHashMap::new();
        // Find all parent pages of the root page.
        let parents = self
            .dag
            .parents(root_index)
            .iter(&self.dag)
            .collect::<Vec<_>>();
        debug!("Parents: {:?}", parents);
        // while let Some(parent) = self.dag.parents(root_index).walk_next(&self.dag) {
        for parent in parents {
            let parent_page = &self.dag.graph()[parent.1];
            let edge = self.dag.edge_weight(parent.0).unwrap();
            match edge {
                // If the parent page is using this page as a layout, add its context as `page`.
                EdgeType::Layout => {
                    info!(
                        "Page (`{}`) is a layout page (of `{}`) … ",
                        root_page.to_path_string(),
                        parent_page.to_path_string()
                    );
                    debug!("{:#?}", parent_page);
                    let parent_object =
                        liquid_core::Value::Object(to_object(&parent_page).into_diagnostic()?);
                    root_contexts.insert("page".into(), parent_object.clone());
                }
                // If the parent page is in a collection this page depends on, make note of it.
                EdgeType::Collection => {
                    let parent_path =
                        fs::canonicalize(PathBuf::from(parent_page.directory.clone()))
                            .into_diagnostic()?;
                    let parent_path_difference = parent_path
                        .strip_prefix(&current_directory)
                        .into_diagnostic()?;
                    let path_components: Vec<String> = parent_path_difference
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect();
                    let collection_name = path_components[0].clone();
                    info!(
                        "Parent page ({:?}) is in collection: {:?}",
                        parent_path_difference, collection_name
                    );
                    if collection_pages.contains_key(&collection_name) {
                        collection_pages
                            .get_mut(&collection_name)
                            .unwrap()
                            .push(parent.1);
                    } else {
                        collection_pages.insert(collection_name.clone(), vec![parent.1]);
                    }
                }
            }
        }
        // Add the collection pages to the root page's contexts.
        info!("Adding any collections to page's contexts … ");
        for (collection_name, collection) in collection_pages.iter_mut() {
            let collection_pages: Vec<liquid::Object> = collection
                .iter()
                .map(|page_index| {
                    to_object(&self.dag.node_weight_mut(*page_index).unwrap().to_owned())
                        .into_diagnostic()
                        .unwrap()
                })
                .collect();
            debug!("`{}` pages: {:#?}", collection_name, collection_pages);
            let collection_object = to_value(&collection_pages).into_diagnostic()?;
            // liquid_core::Value::Array(to_object(&collection_pages).into_diagnostic()?);
            root_contexts.insert(collection_name.clone().into(), collection_object.clone());
        }
        debug!(
            "Contexts for `{}`: {:#?}",
            root_page.to_path_string(),
            root_contexts
        );
        let root_page = self.dag.node_weight_mut(root_index).unwrap();
        if root_page.render(&root_contexts, &self.template_parser)? {
            rendered_indices.push(root_index);
            debug!(
                "After rendering `{}`: {:#?}",
                root_page.to_path_string(),
                root_page
            );
        }

        let children = self
            .dag
            .children(root_index)
            .iter(&self.dag)
            .collect::<Vec<_>>();
        debug!("Children: {:?}", children);
        // while let Some(child) = self.dag.children(root_index).walk_next(&self.dag) {
        for child in children {
            // let child_contexts = self.contexts.clone();
            // let child_page = &self.dag.graph()[child.1];
            // let child_path = fs::canonicalize(PathBuf::from(child_page.directory.clone())).into_diagnostic()?;
            // let child_path_difference = child_path.strip_prefix(&current_directory).into_diagnostic()?;
            // // If the child page is a layout page, its context is called `layout`.
            // // All other pages have their contexts named `page`.
            // if child_path_difference.starts_with(PathBuf::from("layout")) {
            //     let layout_object = liquid_core::Value::Object(to_object(&child_page).into_diagnostic()?);
            //     child_contexts.insert("layout".into(), layout_object.clone());
            // } else {
            //     let page_object = liquid_core::Value::Object(to_object(&child_page).into_diagnostic()?);
            //     child_contexts.insert("page".into(), page_object.clone());
            // }
            // // let mut parent_pages: AHashMap<NodeIndex, Page> = AHashMap::new();
            // let mut collection_pages: AHashMap<String, Vec<NodeIndex>> = AHashMap::new();
            // // Find all parent pages of the child page.
            // while let Some(parent) = self.dag.parents(child.1).walk_next(&self.dag) {
            //     let parent_page = &self.dag.graph()[parent.1];
            //     let edge = self.dag.edge_weight(parent.0).unwrap();
            //     match edge {
            //         // If the parent page is using this page as a layout, add its context as `page`.
            //         EdgeType::Layout => {
            //             let parent_object = liquid_core::Value::Object(to_object(&parent_page).into_diagnostic()?);
            //             child_contexts.insert("page".into(), parent_object.clone());
            //         }
            //         // If the parent page is in a collection this page depends on, make note of it.
            //         EdgeType::Collection => {
            //             let parent_path =
            //                 fs::canonicalize(PathBuf::from(parent_page.directory.clone())).into_diagnostic()?;
            //             let parent_path_difference =
            //                 parent_path.strip_prefix(&current_directory).into_diagnostic()?;
            //             let path_components: Vec<String> = parent_path_difference
            //                 .components()
            //                 .map(|c| c.as_os_str().to_string_lossy().to_string())
            //                 .collect();
            //             let collection_name = path_components[0].clone();
            //             if collection_pages.contains_key(&collection_name) {
            //                 collection_pages
            //                     .get_mut(&collection_name)
            //                     .unwrap()
            //                     .push(parent.1);
            //             } else {
            //                 collection_pages.insert(collection_name.clone(), vec![parent.1]);
            //             }
            //         }
            //     }
            //     // parent_pages.insert(parent.1, parent_page.clone());
            // }
            // // for (parent_index, parent_page) in parent_pages.iter_mut() {
            // //     let parent_path = fs::canonicalize(PathBuf::from(parent_page.directory.clone())).into_diagnostic()?;
            // //     let path_difference =
            // //         parent_path.strip_prefix(fs::canonicalize(env::current_dir().into_diagnostic()?).into_diagnostic()?).into_diagnostic()?;
            // //     // If the parent page is a layout page, render it and add it to the child's contexts.
            // //     // if path_difference.starts_with(PathBuf::from("layout")) {
            // //     //     let layout_object = {
            // //     //         let layout_page = self.dag.node_weight_mut(*parent_index).unwrap();
            // //     //         if layout_page.render(&contexts, &self.template_parser).into_diagnostic()? {
            // //     //             rendered_indices.push(*parent_index);
            // //     //         }
            // //     //         liquid_core::Value::Object(to_object(&parent_page).into_diagnostic()?)
            // //     //     };
            // //     //     child_contexts.insert("layout".into(), layout_object.clone());
            // //     // } else {
            // //         // If the parent page is a collection page, make note of it.
            // //         let path_components: Vec<String> = path_difference
            // //             .components()
            // //             .map(|c| c.as_os_str().to_string_lossy().to_string())
            // //             .collect();
            // //         let collection_name = path_components[0].clone();
            // //         if collection_pages.contains_key(&collection_name) {
            // //             collection_pages
            // //                 .get_mut(&collection_name)
            // //                 .unwrap()
            // //                 .push(*parent_index);
            // //         } else {
            // //             collection_pages.insert(collection_name.clone(), vec![*parent_index]);
            // //         }
            // //     // }
            // // }
            // // Add the collection pages to the child's contexts.
            // for (collection_name, collection) in collection_pages.iter_mut() {
            //     // let mut collection_pages = Vec::new();
            //     let collection_pages: Vec<Page> = collection
            //         .iter()
            //         .map(|page_index| self.dag.node_weight_mut(*page_index).unwrap().to_owned())
            //         .collect();
            //     // for page_index in collection.iter() {
            //     //     let collection_page = self.dag.node_weight_mut(*page_index).unwrap();
            //     //     collection_pages.push(collection_page.clone());
            //     // }
            //     let collection_object = liquid_core::Value::Object(to_object(&collection_pages).into_diagnostic()?);
            //     child_contexts.insert(collection_name.clone().into(), collection_object.clone());
            // }
            self.render_recursively(child.1, rendered_indices)?;
        }
        rendered_indices.dedup();
        Ok(())
    }

    /// Find the indices of all pages in the DAG that have no parent pages.
    ///
    /// # Returns
    ///
    /// A list of indices.
    pub fn find_root_indices(&self) -> Vec<NodeIndex> {
        self.dag.graph().externals(Direction::Incoming).collect()
    }
}
