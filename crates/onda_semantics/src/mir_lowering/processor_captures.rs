//! Symbolic capture selections survive nested branches without creating
//! branch-local descriptors. Physical references are formed at their use site.
use super::*;
use crate::processor_views::{ProcessorResourceShape, ProcessorStorageShape};

/// Choices contain distinct leaves and one selector, never nested capture trees.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ProcessorCapture<T> {
    Direct(T),
    Choice { tag: Value, alternatives: Vec<T> },
}

impl<T> ProcessorCapture<T> {
    fn alternatives(&self) -> &[T] {
        match self {
            Self::Direct(value) => std::slice::from_ref(value),
            Self::Choice { alternatives, .. } => alternatives,
        }
    }

    fn tag(&self) -> Option<Value> {
        match self {
            Self::Direct(_) => None,
            Self::Choice { tag, .. } => Some(*tag),
        }
    }

    pub(super) fn visit_leaves(&self, visit: &mut impl FnMut(&T)) {
        self.alternatives().iter().for_each(visit);
    }

    fn visit_leaves_mut(&mut self, visit: &mut impl FnMut(&mut T)) {
        match self {
            Self::Direct(value) => visit(value),
            Self::Choice { alternatives, .. } => alternatives.iter_mut().for_each(visit),
        }
    }

    pub(super) fn visit_values(&mut self, visit: &mut impl FnMut(&mut Value)) {
        if let Self::Choice { tag, .. } = self {
            visit(tag);
        }
    }
}

impl FunctionLowerer<'_> {
    /// Give captured branch-local roots private names before lexical scopes
    /// merge. Only binding metadata survives; descriptors still form at use.
    pub(super) fn preserve_processor_capture_roots(
        &mut self,
        bindings: &mut HashMap<String, Binding>,
        outer: &HashMap<String, Binding>,
        live: &[String],
    ) -> HashMap<String, Binding> {
        let mut pending = live
            .iter()
            .filter_map(|name| bindings.get(name))
            .flat_map(processor_capture_roots)
            .collect::<Vec<_>>();
        let mut roots = std::collections::BTreeSet::new();
        while let Some(root) = pending.pop() {
            if outer.contains_key(&root) || roots.contains(&root) {
                continue;
            }
            let Some(binding) = bindings.get(&root) else {
                continue;
            };
            pending.extend(processor_capture_roots(binding));
            roots.insert(root);
        }
        if roots.is_empty() {
            return HashMap::new();
        }
        let replacements = roots
            .into_iter()
            .map(|root| (root, self.fresh_data_name()))
            .collect::<HashMap<_, _>>();
        let mut preserved = HashMap::new();
        for (name, binding) in &*bindings {
            // Walk path ancestors once, rather than scanning all bindings for
            // each root. Nested roots may each need their own private alias.
            for end in std::iter::once(name.len()).chain(name.match_indices('.').map(|(i, _)| i)) {
                if let Some(root) = replacements.get(&name[..end]) {
                    preserved.insert(format!("{root}{}", &name[end..]), binding.clone());
                }
            }
        }
        for binding in preserved.values_mut() {
            rewrite_capture_roots(binding, &replacements);
        }
        for name in live {
            if let Some(binding) = bindings.get_mut(name) {
                rewrite_capture_roots(binding, &replacements);
            }
        }
        preserved
    }

    pub(super) fn publish_processor_capture<T: Clone>(
        &mut self,
        capture: &ProcessorCapture<T>,
        active: &mut MirBlock,
        inactive: &mut MirBlock,
        location: SourceLoc,
    ) -> ProcessorCapture<T> {
        match capture {
            ProcessorCapture::Direct(value) => ProcessorCapture::Direct(value.clone()),
            ProcessorCapture::Choice { tag, alternatives } => ProcessorCapture::Choice {
                tag: self.join_processor_coordinate(
                    *tag,
                    Value::Constant(ScalarValue::I32(0)),
                    active,
                    inactive,
                    location,
                ),
                alternatives: alternatives.clone(),
            },
        }
    }

    pub(super) fn join_processor_capture<T: Clone + PartialEq>(
        &mut self,
        a: &ProcessorCapture<T>,
        b: &ProcessorCapture<T>,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        location: SourceLoc,
    ) -> ProcessorCapture<T> {
        if a == b {
            return a.clone();
        }
        let mut alternatives = a.alternatives().to_vec();
        for value in b.alternatives() {
            if !alternatives.contains(value) {
                alternatives.push(value.clone());
            }
        }
        if alternatives.len() == 1 {
            return ProcessorCapture::Direct(alternatives.remove(0));
        }
        let selected = |this: &mut Self, capture: &ProcessorCapture<T>, block: &mut MirBlock| {
            let mapping = capture
                .alternatives()
                .iter()
                .map(|value| {
                    alternatives
                        .iter()
                        .position(|candidate| candidate == value)
                        .unwrap() as i32
                })
                .collect::<Vec<_>>();
            this.remap_processor_tag(capture.tag(), &mapping, block, location.clone())
        };
        let a = selected(self, a, then_block);
        let b = selected(self, b, else_block);
        ProcessorCapture::Choice {
            tag: self.join_processor_coordinate(a, b, then_block, else_block, location),
            alternatives,
        }
    }

    /// A tag mapping reads its input only on the path where that input exists.
    /// The subsequent join publishes just one fully initialized output tag.
    pub(super) fn remap_processor_tag(
        &mut self,
        tag: Option<Value>,
        mapping: &[i32],
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Value {
        let Some(tag) = tag else {
            return Value::Constant(ScalarValue::I32(mapping[0]));
        };
        if mapping
            .iter()
            .enumerate()
            .all(|(index, target)| index as i32 == *target)
        {
            return tag;
        }
        let local = self.new_local(None, PrimitiveType::I32);
        let alternatives = mapping
            .iter()
            .map(|index| {
                let mut branch = MirBlock::default();
                self.assign_value(
                    &mut branch,
                    local,
                    Value::Constant(ScalarValue::I32(*index)),
                    location.clone(),
                );
                branch
            })
            .collect();
        block.statements.extend(
            self.processor_choice_block(tag, alternatives, 0, location)
                .statements,
        );
        Value::Local(local)
    }

    pub(super) fn materialize_processor_storage_capture(
        &mut self,
        capture: &ProcessorCapture<String>,
        storage: &ProcessorStorageShape,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<String, MirLoweringError> {
        let root = self.materialize_processor_capture(
            capture.alternatives(),
            (capture.tag(), 0),
            block,
            location.clone(),
            &mut |this, a, b, then_block, else_block, location| {
                this.join_processor_storage_root(a, b, storage, then_block, else_block, location)
            },
        )?;
        // Owned column storage outlives a lexical arm, while the slice locals
        // made in that arm do not. Reconstruct those references at each use,
        // retaining the complete view metadata on the temporary owner.
        if storage.len.is_none() {
            return Ok(root);
        }
        let Some(Binding::StructArrayParameter {
            struct_name,
            processor,
            length,
            mut fields,
        }) = self.bindings.get(&root).cloned()
        else {
            return Ok(root);
        };
        if fields.is_empty()
            || !fields.iter().all(|(path, _, _)| {
                matches!(
                    self.bindings.get(&format!("{root}.{path}")),
                    Some(Binding::Array(..))
                )
            })
        {
            return Ok(root);
        }
        for (path, local, _) in &mut fields {
            let slice = self.lower_named_slice(
                &format!("{root}.{path}"),
                SliceSelection::default(),
                None,
                block,
                location.clone(),
            )?;
            let Value::Local(value) = slice.value else {
                unreachable!("materialized column reference")
            };
            *local = value;
        }
        let captured = self.fresh_data_name();
        let binding = Binding::StructArrayParameter {
            struct_name,
            processor,
            length,
            fields,
        };
        self.bind_joined_struct_array_leaves(&captured, &binding, location)?;
        self.bindings.insert(captured.clone(), binding);
        Ok(captured)
    }

    pub(super) fn materialize_processor_resource_capture(
        &mut self,
        capture: &ProcessorCapture<Expr>,
        shape: &ProcessorResourceShape,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Expr, MirLoweringError> {
        self.materialize_processor_capture(
            capture.alternatives(),
            (capture.tag(), 0),
            block,
            location,
            &mut |this, a, b, then_block, else_block, location| {
                let mut joined = this.join_processor_resource_expressions(
                    std::slice::from_ref(a),
                    std::slice::from_ref(b),
                    std::slice::from_ref(shape),
                    then_block,
                    else_block,
                    location,
                )?;
                Ok(joined.remove(0))
            },
        )
    }

    fn materialize_processor_capture<T: Clone>(
        &mut self,
        alternatives: &[T],
        selection: (Option<Value>, usize),
        block: &mut MirBlock,
        location: SourceLoc,
        join: &mut impl FnMut(
            &mut Self,
            &T,
            &T,
            &mut MirBlock,
            &mut MirBlock,
            SourceLoc,
        ) -> Result<T, MirLoweringError>,
    ) -> Result<T, MirLoweringError> {
        self.select_processor_capture(
            alternatives.len(),
            selection,
            block,
            location,
            &mut |_, index, _, _| Ok(alternatives[index].clone()),
            join,
        )
    }

    pub(super) fn select_processor_capture<T>(
        &mut self,
        count: usize,
        selection: (Option<Value>, usize),
        block: &mut MirBlock,
        location: SourceLoc,
        lower: &mut impl FnMut(
            &mut Self,
            usize,
            &mut MirBlock,
            SourceLoc,
        ) -> Result<T, MirLoweringError>,
        join: &mut impl FnMut(
            &mut Self,
            &T,
            &T,
            &mut MirBlock,
            &mut MirBlock,
            SourceLoc,
        ) -> Result<T, MirLoweringError>,
    ) -> Result<T, MirLoweringError> {
        let (tag, first) = selection;
        if count == 1 {
            return lower(self, first, block, location);
        }
        let mid = count / 2;
        let mut then_block = MirBlock::default();
        let a = self.select_processor_capture(
            mid,
            selection,
            &mut then_block,
            location.clone(),
            lower,
            join,
        )?;
        let mut else_block = MirBlock::default();
        let b = self.select_processor_capture(
            count - mid,
            (tag, first + mid),
            &mut else_block,
            location.clone(),
            lower,
            join,
        )?;
        let result = join(
            self,
            &a,
            &b,
            &mut then_block,
            &mut else_block,
            location.clone(),
        )?;
        let condition = self.compare_value(
            block,
            CompareOp::Less,
            tag.expect("choice has a tag"),
            Value::Constant(ScalarValue::I32((first + mid) as i32)),
            location.clone(),
        );
        self.push_statement(
            block,
            StatementKind::If {
                condition,
                then_block,
                else_block,
            },
            location,
        );
        Ok(result)
    }
}

fn rewrite_capture_roots(binding: &mut Binding, replacements: &HashMap<String, String>) {
    fn rewrite(name: &mut String, replacements: &HashMap<String, String>) {
        let end = std::iter::once(name.len())
            .chain(name.rmatch_indices('.').map(|(i, _)| i))
            .find(|end| replacements.contains_key(&name[..*end]));
        if let Some(end) = end {
            *name = format!("{}{}", replacements[&name[..end]], &name[end..]);
        }
    }
    let Some(processor) = binding.processor_view_mut() else {
        return;
    };
    processor.visit_environments_mut(&mut |frame| {
        for capture in [&mut frame.owner, &mut frame.state_root]
            .into_iter()
            .flatten()
        {
            capture.visit_leaves_mut(&mut |root| rewrite(root, replacements));
        }
        for (resource, shape) in frame.resources.iter_mut().zip(&frame.shape.resources) {
            if shape.binding.is_some() {
                continue;
            }
            let Some(resource) = resource else { continue };
            resource.visit_leaves_mut(&mut |expression| {
                expression.visit_mut(|node| {
                    if let Expr::Var { name, .. }
                    | Expr::Index { base: name, .. }
                    | Expr::Slice { base: name, .. } = node
                    {
                        rewrite(name, replacements);
                    }
                    true
                });
            });
        }
    });
}

pub(super) fn processor_capture_roots(binding: &Binding) -> Vec<String> {
    let mut result = Vec::new();
    let Some(processor) = binding.processor_view() else {
        return result;
    };
    processor.visit_environments(&mut |environment| {
        if let Some(owner) = &environment.owner {
            owner.visit_leaves(&mut |root| result.push(root.clone()));
        }
        if let Some(root) = &environment.state_root {
            root.visit_leaves(&mut |root| result.push(root.clone()));
        }
        for (resource, shape) in environment
            .resources
            .iter()
            .zip(&environment.shape.resources)
        {
            if shape.binding.is_some() {
                continue;
            }
            let Some(resource) = resource else { continue };
            resource.visit_leaves(&mut |resource| {
                for expression in resource.walk() {
                    match expression {
                        Expr::Var { name, .. }
                        | Expr::Index { base: name, .. }
                        | Expr::Slice { base: name, .. } => result.push(name.clone()),
                        _ => {}
                    }
                }
            });
        }
    });
    result
}
