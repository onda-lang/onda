use super::*;

/// A logical SoA leaf, independent of native alignment and wire offsets.
#[derive(Debug)]
pub struct PayloadLeaf<'a> {
    pub path: String,
    pub encoding: ScalarEncoding,
    pub shape: Vec<usize>,
    pub integer_range: Option<&'a crate::IntegerRangeMetadata>,
}

impl PayloadType {
    /// The common recursive shape planner for internal data and host payloads.
    /// Fixed array extents remain axes rather than expanding element metadata.
    /// A slice's runtime outer axis is supplied by its enclosing message plan.
    pub fn leaves(&self) -> Result<Vec<PayloadLeaf<'_>>, PayloadError> {
        let mut leaves = Vec::new();
        let mut pending = vec![(self, String::new(), Vec::new())];
        while let Some((ty, path, shape)) = pending.pop() {
            match ty {
                Self::Scalar {
                    encoding,
                    integer_range,
                } => leaves.push(PayloadLeaf {
                    path,
                    encoding: *encoding,
                    shape,
                    integer_range: integer_range.as_ref(),
                }),
                Self::Tuple { elements } => {
                    if elements.is_empty()
                        || elements.iter().any(|ty| !matches!(ty, Self::Scalar { .. }))
                    {
                        return Err(PayloadError::InvalidSchema);
                    }
                    for (index, element) in elements.iter().enumerate().rev() {
                        pending.push((
                            element,
                            child_path(&path, &format!("__{index}")),
                            shape.clone(),
                        ));
                    }
                }
                Self::Struct { name, fields } => {
                    if name.is_empty() {
                        return Err(PayloadError::InvalidSchema);
                    }
                    layout::unique_fields(fields)?;
                    for field in fields.iter().rev() {
                        pending.push((&field.ty, child_path(&path, &field.name), shape.clone()));
                    }
                }
                Self::Array { element, len } => {
                    if *len == 0 || !layout::array_element(element) {
                        return Err(PayloadError::InvalidSchema);
                    }
                    let mut shape = shape;
                    shape.push(*len);
                    pending.push((element, path, shape));
                }
                Self::Slice { .. } => return Err(PayloadError::InvalidSchema),
            }
        }
        Ok(leaves)
    }
}

fn child_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}.{child}")
    }
}
