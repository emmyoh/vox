use crate::page::Page;
use ahash::AHashMap;
use daggy::{petgraph::Direction, stable_dag::StableDag, NodeIndex, Walker};
use liquid::{to_object, Object, Parser};
use std::{env, error::Error, fs, path::PathBuf};

/// Information held in memory while performing a build.
pub struct Build {
    /// A Liquid template parser.
    pub template_parser: Parser,
    /// The Liquid contexts necessary to render templates in pages.
    pub contexts: Object,
    /// The locale information of the build, primarily used to render dates and times.
    pub locale: String,
    /// A directed acyclic graph (DAG) populated with pages and their children.
    pub dag: StableDag<Page, ()>,
}

impl Build {
    /// Render all pages in the DAG.
    ///
    /// # Returns
    ///
    /// A list of all nodes that were rendered.
    pub fn render_all(&mut self) -> Result<Vec<NodeIndex>, Box<dyn Error + Send + Sync>> {
        let mut rendered_indices = Vec::new();
        let root_indices = self.find_root_indices();
        for root_index in root_indices {
            let some_rendered_indices =
                self.render_recursively(root_index, self.contexts.clone())?;
            rendered_indices.extend(some_rendered_indices.iter());
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
    /// * `contexts` - The appropriate Liquid contexts to render the page with. This includes the context of the page's parent.
    ///
    /// # Returns
    ///
    /// A list of all nodes that were rendered.
    pub fn render_recursively(
        &mut self,
        root_index: NodeIndex,
        contexts: Object,
    ) -> Result<Vec<NodeIndex>, Box<dyn Error + Send + Sync>> {
        let mut rendered_indices = Vec::new();

        while let Some(child) = self.dag.children(root_index).walk_next(&self.dag) {
            let mut child_contexts = self.contexts.clone();
            let mut parent_pages: AHashMap<NodeIndex, Page> = AHashMap::new();
            let mut collection_pages: AHashMap<String, Vec<NodeIndex>> = AHashMap::new();
            // Find all parent pages of the child page.
            while let Some(parent) = self.dag.parents(child.1).walk_next(&self.dag) {
                let parent_page = &self.dag.graph()[parent.1];
                parent_pages.insert(parent.1, parent_page.clone());
            }
            for (parent_index, parent_page) in parent_pages.iter_mut() {
                let parent_path = fs::canonicalize(PathBuf::from(parent_page.directory.clone()))?;
                let path_difference =
                    parent_path.strip_prefix(fs::canonicalize(env::current_dir()?)?)?;
                // If the parent page is a layout page, render it and add it to the child's contexts.
                if path_difference.starts_with(PathBuf::from("layout")) {
                    let layout_object = {
                        let layout_page = self.dag.node_weight_mut(*parent_index).unwrap();
                        if layout_page.render(&contexts, &self.template_parser)? {
                            rendered_indices.push(*parent_index);
                        }
                        liquid_core::Value::Object(to_object(&parent_page)?)
                    };
                    child_contexts.insert("layout".into(), layout_object.clone());
                } else {
                    // If the parent page is a collection page, make note of it.
                    let path_components: Vec<String> = path_difference
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect();
                    let collection_name = path_components[0].clone();
                    if collection_pages.contains_key(&collection_name) {
                        collection_pages
                            .get_mut(&collection_name)
                            .unwrap()
                            .push(*parent_index);
                    } else {
                        collection_pages.insert(collection_name.clone(), vec![*parent_index]);
                    }
                }
            }
            // Render all collection pages and add them to the child's contexts.
            for (collection_name, collection) in collection_pages.iter_mut() {
                let collection_object = {
                    let mut collection_pages = Vec::new();
                    for page_index in collection.iter() {
                        let collection_page = self.dag.node_weight_mut(*page_index).unwrap();
                        if collection_page.render(&contexts, &self.template_parser)? {
                            rendered_indices.push(*page_index);
                        }
                        collection_pages.push(collection_page.clone());
                    }
                    liquid_core::Value::Object(to_object(&collection_pages)?)
                };
                child_contexts.insert(collection_name.clone().into(), collection_object.clone());
            }
            let some_rendered_indices = self.render_recursively(child.1, child_contexts)?;
            rendered_indices.extend(some_rendered_indices.iter());
        }
        rendered_indices.dedup();
        Ok(rendered_indices)
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
