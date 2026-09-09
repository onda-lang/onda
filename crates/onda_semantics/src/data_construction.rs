//! Shared constructor argument selection and compile-time data construction.
use crate::*;
use onda_processor_abi::payload::PayloadDefault;

pub(crate) fn is_authored_struct_field(field: &TypedStructField) -> bool {
    !field.name.contains('.')
}

pub(crate) fn constructor_fields(
    fields: &[TypedStructField],
    args: &[onda_frontend::CallArg],
) -> Result<Vec<usize>, (String, SourceLoc)> {
    let mut supplied = HashSet::new();
    let mut positional = 0;
    args.iter()
        .map(|arg| {
            let index = if let Some(name) = &arg.name {
                fields.iter().position(|field| &field.name == name)
            } else {
                let index = positional;
                positional += 1;
                Some(index)
            }
            .filter(|index| *index < fields.len())
            .ok_or_else(|| {
                (
                    "constructor argument does not name a field".to_owned(),
                    arg.expr.loc(),
                )
            })?;
            if !supplied.insert(index) {
                return Err((
                    "constructor field supplied more than once".to_owned(),
                    arg.expr.loc(),
                ));
            }
            Ok(index)
        })
        .collect()
}

pub(crate) fn scalar_default(value: TypedConstValue) -> PayloadDefault {
    PayloadDefault::Scalar(match value {
        TypedConstValue::F32(value) => value.to_string(),
        TypedConstValue::F64(value) => value.to_string(),
        TypedConstValue::I32(value) => value.to_string(),
        TypedConstValue::I64(value) => value.to_string(),
        TypedConstValue::Bool(value) => value.to_string(),
    })
}

pub(crate) struct DefaultEvaluator<'a> {
    pub structs: &'a HashMap<String, Vec<TypedStructField>>,
    pub options: AnalysisOptions,
    pub errors: &'a mut Vec<Diagnostic>,
}
impl<'a> DefaultEvaluator<'a> {
    pub fn new(
        structs: &'a HashMap<String, Vec<TypedStructField>>,
        options: AnalysisOptions,
        errors: &'a mut Vec<Diagnostic>,
    ) -> Self {
        Self {
            structs,
            options,
            errors,
        }
    }
    fn error(&mut self, expr: Option<&Expr>, message: &str) -> Option<PayloadDefault> {
        push_semantic(
            DiagCtx::new(expr.map_or(SourceLoc::ZERO, Expr::loc)),
            self.errors,
            message.to_owned(),
        );
        None
    }
    fn scalar(&mut self, ty: PrimitiveType, expr: Option<&Expr>) -> Option<PayloadDefault> {
        let value = if let Some(expr) = expr {
            crate::processor_lowering::coerce_scalar_event_default(
                expr,
                ty,
                "data default",
                self.options,
                self.errors,
            )?
        } else {
            match ty {
                PrimitiveType::F32 => TypedConstValue::F32(0.0),
                PrimitiveType::F64 => TypedConstValue::F64(0.0),
                PrimitiveType::I32 => TypedConstValue::I32(0),
                PrimitiveType::I64 => TypedConstValue::I64(0),
                PrimitiveType::Bool => TypedConstValue::Bool(false),
            }
        };
        Some(scalar_default(value))
    }
    pub fn field(
        &mut self,
        field: &TypedStructField,
        expr: Option<&Expr>,
    ) -> Option<PayloadDefault> {
        let expr = match (field.integer_range, expr) {
            (Some(range), Some(expr @ Expr::Call { args, .. }))
                if typed_integer_range_from_expr(expr, Some(range.ty)) == Some(range) =>
            {
                args.first()
            }
            (_, expr) => expr,
        };
        let mut value = match &field.ty {
            TypedFieldType::Scalar(ty) => self.scalar(*ty, expr)?,
            TypedFieldType::Tuple(types) => {
                let values = match expr {
                    Some(Expr::Tuple { values, .. }) if values.len() == types.len() => Some(values),
                    None => None,
                    _ => {
                        return self.error(expr, "data default requires a matching constant tuple")
                    }
                };
                PayloadDefault::Aggregate(
                    types
                        .iter()
                        .enumerate()
                        .map(|(index, ty)| self.scalar(*ty, values.map(|values| &values[index])))
                        .collect::<Option<_>>()?,
                )
            }
            TypedFieldType::Struct | TypedFieldType::Array(_) => {
                return self.error(
                    expr,
                    "aggregate struct fields cannot have default expressions",
                )
            }
        };
        if let (Some(range), PayloadDefault::Scalar(text)) = (field.integer_range, &mut value) {
            let integer = text.parse::<i64>().ok()?;
            let normalized = if range.wrap {
                (i128::from(range.min)
                    + (i128::from(integer) - i128::from(range.min))
                        .rem_euclid(i128::from(range.max) - i128::from(range.min) + 1))
                    as i64
            } else {
                integer.clamp(range.min, range.max)
            };
            *text = normalized.to_string();
        }
        Some(value)
    }
}

pub(crate) fn populate_schema_defaults(
    ty: &mut onda_processor_abi::payload::PayloadType,
    evaluator: &mut DefaultEvaluator<'_>,
) {
    use onda_processor_abi::payload::PayloadType;
    match ty {
        PayloadType::Struct { name, fields } => {
            for field in fields {
                if let Some(source) = evaluator
                    .structs
                    .get(name)
                    .and_then(|fields| fields.iter().find(|source| source.name == field.name))
                    .cloned()
                {
                    if source.default.is_some() {
                        field.default = evaluator.field(&source, source.default.as_ref());
                    }
                }
                populate_schema_defaults(&mut field.ty, evaluator);
            }
        }
        PayloadType::Array { element, .. } | PayloadType::Slice { element } => {
            populate_schema_defaults(element, evaluator)
        }
        PayloadType::Scalar { .. } | PayloadType::Tuple { .. } => {}
    }
}
