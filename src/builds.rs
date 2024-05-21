use crate::page::Page;
use daggy::{petgraph::Direction, stable_dag::StableDag, NodeIndex, Walker};
use liquid::{to_object, Object, Parser};
use std::error::Error;

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
        let root_object = {
            let root_page = self.dag.node_weight_mut(root_index).unwrap();
            if root_page.render(&contexts, &self.template_parser)? {
                rendered_indices.push(root_index);
            }
            liquid_core::Value::Object(to_object(&root_page)?)
        };
        while let Some(child) = self.dag.children(root_index).walk_next(&self.dag) {
            let mut child_contexts = self.contexts.clone();
            child_contexts.insert("layout".into(), root_object.clone());
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
