use super::*;
use onda_processor_abi::payload::{PayloadPlan, PayloadSchema};

impl Validator<'_> {
    pub(super) fn validate_message_schemas(&mut self) {
        for event in &self.program.interface.events {
            self.validate_message_schema(
                &event.name,
                &event.schema,
                event.params.iter().map(|p| (p.name.as_str(), p.ty)),
            );
        }
        for delegate in &self.program.interface.delegates {
            self.validate_message_schema(
                &delegate.name,
                &delegate.schema,
                delegate.params.iter().map(|p| (p.name.as_str(), p.ty)),
            );
        }
    }

    fn validate_message_schema<'a>(
        &mut self,
        name: &str,
        schema: &PayloadSchema,
        params: impl Iterator<Item = (&'a str, crate::TypeId)>,
    ) {
        let plan = match PayloadPlan::new(schema) {
            Ok(plan) => plan,
            Err(error) => {
                self.program_error(format!("message '{name}' has an invalid schema: {error}"));
                return;
            }
        };
        let params = params.collect::<Vec<_>>();
        let mut matches = params.len() == plan.abi_parameter_count();
        for (group, field) in plan.parameters().iter().zip(&schema.params) {
            if let Some(index) = group.length_parameter {
                matches &= params.get(index).is_some_and(|(path, ty)| {
                    *path == field.name
                        && self.program.types.get(ty.index())
                            == Some(&Type::Scalar(crate::ScalarType::I32))
                });
            }
            for index in group.tensors.clone() {
                let leaf = &plan.tensors()[index];
                let Some((path, ty)) = params.get(leaf.parameter) else {
                    continue;
                };
                let element = match self.program.types.get(ty.index()) {
                    Some(Type::Scalar(ty)) if !group.dynamic && leaf.shape.is_empty() => Some(*ty),
                    Some(Type::Array { element, len })
                        if !group.dynamic
                            && !leaf.shape.is_empty()
                            && *len as usize == leaf.elements =>
                    {
                        match self.program.types.get(element.index()) {
                            Some(Type::Scalar(ty)) => Some(*ty),
                            _ => None,
                        }
                    }
                    Some(Type::Slice {
                        element,
                        access: crate::AccessMode::ReadOnly,
                    }) if group.dynamic => Some(*element),
                    _ => None,
                };
                matches &= *path == leaf.path
                    && element.map(crate::payload::payload_scalar) == Some(leaf.encoding);
            }
        }
        if !matches {
            self.program_error(format!(
                "message '{name}' tensor parameters do not match its recursive schema"
            ));
        }
    }
}
