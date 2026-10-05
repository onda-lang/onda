use super::*;
use std::cell::RefCell;

type PayloadIdentity = (PrimitiveType, *const Vec<crate::TypedConstValue>);

/// Validate cheap declaration properties individually and each shared payload once.
pub(super) fn validate_const_arrays(arrays: &[crate::TypedConstArray]) -> Vec<MirLoweringError> {
    let mut errors = Vec::new();
    let mut payload_types = HashMap::new();
    for array in arrays {
        let mut error = |message| errors.push(MirLoweringError::new(message, SourceLoc::ZERO));
        if array.len == 0 || array.len > i32::MAX as usize {
            error(format!(
                "constant array '{}' length must be between 1 and i32::MAX for MIR indexing",
                array.name
            ));
        }
        if array.values.len() != array.len {
            error(format!(
                "constant array '{}' declares {} elements but contains {} values",
                array.name,
                array.len,
                array.values.len()
            ));
        }
        let valid = payload_types
            .entry(payload_identity(array))
            .or_insert_with(|| {
                array
                    .values
                    .iter()
                    .all(|value| value.primitive_type() == array.elem_ty)
            });
        if !*valid {
            error(format!(
                "constant array '{}' contains a value that does not match element type {}",
                array.name,
                array.elem_ty.name()
            ));
        }
    }
    errors
}

fn payload_identity(array: &crate::TypedConstArray) -> PayloadIdentity {
    (array.elem_ty, std::sync::Arc::as_ptr(&array.values))
}

#[derive(Debug)]
struct Registry {
    data: Vec<onda_mir::ConstData>,
    ids: HashMap<String, onda_mir::ConstDataId>,
    payloads: HashMap<PayloadIdentity, onda_mir::ConstDataId>,
    validated: HashSet<onda_mir::ConstDataId>,
}

/// Descriptors are cheap to inspect; immutable data is registered only when a
/// load or view is actually lowered. All entry points and functions share IDs.
#[derive(Debug)]
pub(super) struct ConstDataCatalog<'a> {
    arrays: HashMap<&'a str, &'a crate::TypedConstArray>,
    registry: RefCell<Registry>,
    original_len: usize,
}

impl<'a> ConstDataCatalog<'a> {
    pub(super) fn new(program: &'a TypedProgram, data: Vec<onda_mir::ConstData>) -> Self {
        let original_len = data.len();
        let arrays: HashMap<_, _> = program
            .const_arrays
            .iter()
            .map(|array| (array.name.as_str(), array))
            .collect();
        let mut ids = HashMap::new();
        let mut payloads = HashMap::new();
        for (index, array) in data.iter().enumerate() {
            let id = onda_mir::ConstDataId::new(index as u32);
            ids.entry(array.name.clone()).or_insert(id);
            if let Some(source) = arrays.get(array.name.as_str()) {
                payloads.entry(payload_identity(source)).or_insert(id);
            }
        }
        Self {
            arrays,
            registry: RefCell::new(Registry {
                data,
                ids,
                payloads,
                validated: HashSet::new(),
            }),
            original_len,
        }
    }

    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.arrays.contains_key(name)
    }

    pub(super) fn length(&self, name: &str) -> Option<u32> {
        u32::try_from(self.arrays.get(name)?.len).ok()
    }

    pub(super) fn shape(
        &self,
        name: &str,
    ) -> Option<crate::array_semantics::ArrayShape<PrimitiveType>> {
        let array = self.arrays.get(name)?;
        Some(crate::array_semantics::ArrayShape::fixed(
            array.elem_ty,
            array.len,
        ))
    }

    pub(super) fn resolve(
        &self,
        name: &str,
        location: SourceLoc,
    ) -> Result<Option<(onda_mir::ConstDataId, PrimitiveType, u32)>, MirLoweringError> {
        let Some(array) = self.arrays.get(name) else {
            return Ok(None);
        };
        let len = u32::try_from(array.len).map_err(|_| {
            MirLoweringError::new(
                format!("constant array '{name}' length does not fit u32"),
                location,
            )
        })?;
        let mut registry = self.registry.borrow_mut();
        let Registry {
            data,
            ids,
            payloads,
            validated,
        } = &mut *registry;
        // Aliases share an Arc. Its identity avoids hashing or copying large
        // arrays, and the program keeps each payload alive for this catalog.
        let key = payload_identity(array);
        let existing = ids.get(name).or_else(|| payloads.get(&key)).copied();
        let id = if let Some(id) = existing {
            // Existing MIR is owned by the caller of standalone function
            // lowering. Validate its contents before reusing that identity.
            if !validated.contains(&id) {
                let existing = &data[id.index()];
                if existing.element != scalar_type(array.elem_ty)
                    || !typed_and_mir_scalar_values_exact_equal(&array.values, &existing.values)
                {
                    return Err(MirLoweringError::new(
                        format!(
                            "constant data '{name}' already exists in MIR with different contents"
                        ),
                        location,
                    ));
                }
            }
            id
        } else {
            let id = onda_mir::ConstDataId::new(data.len() as u32);
            data.push(onda_mir::ConstData {
                name: name.to_owned(),
                element: scalar_type(array.elem_ty),
                values: array.values.iter().copied().map(mir_scalar).collect(),
            });
            id
        };
        ids.insert(name.to_owned(), id);
        payloads.insert(key, id);
        validated.insert(id);
        Ok(Some((id, array.elem_ty, len)))
    }

    pub(super) fn take_data(&self, success: bool) -> Vec<onda_mir::ConstData> {
        let mut data = std::mem::take(&mut self.registry.borrow_mut().data);
        if !success {
            data.truncate(self.original_len);
        }
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program() -> TypedProgram {
        let source = "const Table: f32[2] = [7.0, 9.0]\nconst Alias = Table\nparams:\n  index: i32 = 1\nsample:\n  out1 = Table[index] + Alias[index]\n";
        crate::analyze(onda_frontend::parse_program(source).unwrap()).unwrap()
    }

    fn existing_data(program: &TypedProgram) -> onda_mir::ConstData {
        let array = program
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .unwrap();
        onda_mir::ConstData {
            name: array.name.clone(),
            element: scalar_type(array.elem_ty),
            values: array.values.iter().copied().map(mir_scalar).collect(),
        }
    }

    #[test]
    fn shared_const_payloads_keep_per_declaration_shapes_and_element_types() {
        let mut arrays = program().const_arrays;
        assert!(validate_const_arrays(&arrays).is_empty());
        let alias = arrays
            .iter_mut()
            .find(|array| array.name == "Alias")
            .unwrap();
        alias.len = 3;
        alias.elem_ty = PrimitiveType::I32;
        for _ in 0..2 {
            let errors = validate_const_arrays(&arrays);
            assert_eq!(errors.len(), 2, "{errors:?}");
            assert!(errors.iter().all(|error| error.message.contains("'Alias'")));
            assert!(errors
                .iter()
                .any(|error| error.message.contains("declares 3 elements")));
            assert!(errors
                .iter()
                .any(|error| error.message.contains("element type i32")));
            arrays.reverse();
        }
    }

    #[test]
    fn const_aliases_reuse_existing_mir_data_and_validate_once() {
        let program = program();
        let catalog = ConstDataCatalog::new(&program, vec![existing_data(&program)]);
        for name in ["Alias", "Table", "Alias"] {
            let (id, _, _) = catalog
                .resolve(name, SourceLoc::default())
                .unwrap()
                .unwrap();
            assert_eq!(id, onda_mir::ConstDataId::new(0));
        }
        assert_eq!(catalog.registry.borrow().validated.len(), 1);
        assert_eq!(catalog.take_data(true).len(), 1);
    }

    #[test]
    fn const_alias_reuse_rejects_conflicting_existing_data() {
        let program = program();
        let mut data = existing_data(&program);
        data.values[0] = onda_mir::ScalarValue::F32(8.0);
        let catalog = ConstDataCatalog::new(&program, vec![data]);
        assert!(catalog
            .resolve("Alias", SourceLoc::default())
            .unwrap_err()
            .message
            .contains("different contents"));
        assert!(catalog.registry.borrow().validated.is_empty());
        assert_eq!(catalog.take_data(false).len(), 1);
    }
}
