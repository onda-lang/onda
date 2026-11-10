//! Bind deferred resource expressions in their declaration's lexical scope.
use super::CallTypeEnv;
use crate::processor_views::ProcessorOperation;
use crate::Stmt;

enum Recipe {
    Root(usize),
    Declaration(usize),
}

pub(crate) fn rewrite_root_resource_recipes(
    env: &CallTypeEnv,
    rewrite: impl FnMut(&mut Vec<Stmt>, &mut CallTypeEnv),
) {
    rewrite_resource_recipes(None, env, rewrite);
}

pub(super) fn rewrite_resource_recipes(
    function: Option<&str>,
    env: &CallTypeEnv,
    mut rewrite: impl FnMut(&mut Vec<Stmt>, &mut CallTypeEnv),
) {
    let (recipes, seed) = {
        let views = env.processor_views.borrow();
        if let Some(function) = function {
            let Some(operation) = views.operation(function).filter(|operation| {
                operation.kind == ProcessorOperation::Init && operation.projection.edges.is_empty()
            }) else {
                return;
            };
            let owner = &operation.projection.owner;
            let recipes = views
                .declarations()
                .iter()
                .enumerate()
                .filter(|(_, declaration)| {
                    declaration.owner_struct == *owner && !declaration.resources.is_empty()
                })
                .map(|(index, _)| Recipe::Declaration(index))
                .collect::<Vec<_>>();
            if recipes.is_empty() {
                return;
            }
            let mut seed = env.clone();
            for resource in views
                .lexical_resources
                .get(&operation.name)
                .into_iter()
                .flatten()
            {
                if resource.owner_struct != *owner {
                    continue;
                }
                let name = format!(
                    "self.{}",
                    views.owner_resources[owner][resource.resource_index]
                );
                if let Some(ty) = env.buffer_types.get(&resource.parameter) {
                    seed.buffer_types.insert(name.clone(), ty.clone());
                }
                if let Some(len) = env.buffer_array_lens.get(&resource.parameter) {
                    seed.buffer_array_lens.insert(name, *len);
                }
            }
            (recipes, seed)
        } else {
            if views.root_resource_recipes.is_empty() {
                return;
            }
            (
                (0..views.root_resource_recipes.len())
                    .map(Recipe::Root)
                    .collect(),
                env.clone(),
            )
        }
    };
    for recipe in recipes {
        let mut local_env = seed.clone();
        let expressions = {
            let views = env.processor_views.borrow();
            match recipe {
                Recipe::Root(index) => {
                    let recipe = &views.root_resource_recipes[index];
                    for (name, shape) in recipe.parameters.iter().zip(&recipe.captures) {
                        let name = format!("self.{name}");
                        local_env
                            .buffer_types
                            .insert(name.clone(), (shape.elem_ty, shape.channels.clone()));
                        if let Some(len) = shape.len {
                            local_env.buffer_array_lens.insert(name, len);
                        }
                    }
                    recipe.resources.clone()
                }
                Recipe::Declaration(index) => views.declarations()[index].resources.clone(),
            }
        };
        // The shared statement walker binds these expressions without adding
        // them to executable bodies or evaluating their selectors.
        let mut body = expressions
            .into_iter()
            .map(|expr| Stmt::Expr {
                loc: expr.loc().span(),
                expr,
            })
            .collect();
        rewrite(&mut body, &mut local_env);
        let expressions = body
            .into_iter()
            .map(|statement| {
                let Stmt::Expr { expr, .. } = statement else {
                    unreachable!("call binding preserves recipe expression statements")
                };
                expr
            })
            .collect();
        let mut views = env.processor_views.borrow_mut();
        match recipe {
            Recipe::Root(index) => views.root_resource_recipes[index].resources = expressions,
            Recipe::Declaration(index) => *views.declaration_resources_mut(index) = expressions,
        }
    }
}
