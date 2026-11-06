//! Forward root resource collections without executing their selection recipes.
use super::*;
use crate::processor_views::{
    ProcessorEnvironmentCapture, ProcessorEnvironmentShape, ProcessorResourceBinding,
    ProcessorResourceShape, ProcessorRootResourceRecipe, ProcessorViewCatalog,
};

pub(super) fn defer_root_resources(
    catalog: &mut ProcessorViewCatalog,
    blocks: &[Block],
    options: AnalysisOptions,
) {
    let mut ignored_errors = Vec::new();
    let buffers = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Buffers(buffers) => Some(buffers.as_slice()),
            _ => None,
        })
        .flat_map(|buffers| {
            crate::declaration_coercion::coerce_buffers(buffers, options, &mut ignored_errors)
        })
        .map(|buffer| (buffer.name.clone(), buffer))
        .collect::<HashMap<_, _>>();
    let mut roots = catalog.roots.keys().cloned().collect::<Vec<_>>();
    roots.sort();
    for name in roots {
        let source = catalog
            .roots
            .get_mut(&name)
            .expect("registered processor root");
        for (shape, capture) in source
            .shape
            .environments
            .iter_mut()
            .zip(&mut source.captures)
        {
            if !capture.resources.iter().any(needs_recipe) {
                continue;
            }
            let names = capture
                .resources
                .iter()
                .flat_map(Expr::walk)
                .filter_map(resource_name)
                .filter(|name| buffers.contains_key(*name))
                .map(str::to_owned)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let parameters = (0..names.len())
                .map(|index| format!("__onda_resource_{index}"))
                .collect::<Vec<_>>();
            let captures = names
                .iter()
                .map(|name| {
                    let buffer = &buffers[name];
                    ProcessorResourceShape {
                        elem_ty: buffer.elem_ty,
                        channels: buffer.channels.clone(),
                        len: buffer.is_array.then_some(buffer.array_len),
                        mapped: false,
                        mapped_len: None,
                        alternatives: None,
                        binding: None,
                    }
                })
                .collect::<Vec<_>>();
            let mut resources = capture.resources.clone();
            for resource in &mut resources {
                resource.visit_mut_postorder(|node| {
                    if let Expr::Var { name, .. }
                    | Expr::Index { base: name, .. }
                    | Expr::Slice { base: name, .. } = node
                    {
                        if let Some(index) = names.iter().position(|base| base == name) {
                            *name = format!("self.{}", parameters[index]);
                        }
                    }
                });
            }
            let globals = resources
                .iter()
                .flat_map(Expr::walk)
                .filter_map(resource_name)
                .filter(|name| !name.starts_with("self."))
                .map(|name| name.split('.').next().unwrap_or(name))
                .map(str::to_owned)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let recipe = ProcessorRootResourceRecipe {
                resources,
                parameters,
                captures: captures.clone(),
                globals,
            };
            // Expr equality ignores locations; retain spans for deferred diagnostics.
            let index = catalog
                .root_resource_recipes
                .iter()
                .position(|existing| *existing == recipe)
                .unwrap_or_else(|| {
                    let index = catalog.root_resource_recipes.len();
                    catalog.root_resource_recipes.push(recipe);
                    index
                });
            for (resource_index, resource) in shape.resources.iter_mut().enumerate() {
                // Recipes forward captured parameters, not interface identities.
                // Select their alternatives at use, including static siblings
                // rewritten into the same deferred environment.
                if resource.mapped {
                    resource.mapped = false;
                    resource.alternatives = resource.mapped_len.take();
                }
                resource.binding = Some(ProcessorResourceBinding::RootRecipe {
                    recipe: index,
                    resource_index,
                });
            }
            shape.outer = Some(Box::new(ProcessorEnvironmentShape {
                resource_sources: Vec::new(),
                state: None,
                owner_storage: None,
                owner: None,
                resources: captures,
                outer: None,
            }));
            capture.outer = Some(Box::new(ProcessorEnvironmentCapture {
                state_root: None,
                owner: None,
                coordinates: Vec::new(),
                resources: names.iter().map(Expr::var).collect(),
                outer: None,
            }));
        }
    }
}

fn resource_name(expression: &Expr) -> Option<&str> {
    match expression {
        Expr::Var { name, .. }
        | Expr::Index { base: name, .. }
        | Expr::Slice { base: name, .. } => Some(name),
        _ => None,
    }
}

fn needs_recipe(expression: &Expr) -> bool {
    expression.walk().any(|node| match node {
        Expr::Index { index, .. } => crate::try_constant_index_i64(index).is_none(),
        Expr::Slice { .. } => true,
        _ => false,
    })
}
