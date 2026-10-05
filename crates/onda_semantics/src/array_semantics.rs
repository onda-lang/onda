//! Array compatibility and initializer meaning shared by checking and execution.
use onda_frontend::Expr;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ArrayShape<T> {
    pub(crate) elem_ty: Option<T>,
    pub(crate) len: Option<usize>,
}

impl<T> ArrayShape<T> {
    pub(crate) fn any() -> Self {
        Self {
            elem_ty: None,
            len: None,
        }
    }

    pub(crate) fn fixed(elem_ty: T, len: usize) -> Self {
        Self {
            elem_ty: Some(elem_ty),
            len: Some(len),
        }
    }

    pub(crate) fn elem(elem_ty: T) -> Self {
        Self {
            elem_ty: Some(elem_ty),
            len: None,
        }
    }
}

#[derive(PartialEq, Eq)]
pub(crate) enum ShapeCheck {
    Complete,
    Deferred,
}

#[derive(PartialEq, Eq)]
pub(crate) enum ShapeMismatch {
    Element,
    Length { expected: usize, actual: usize },
}

/// Missing source metadata defers a required check. Missing destination metadata
/// leaves that component unconstrained. Concrete consumers rerun this check.
pub(crate) fn check_array_shape<T: PartialEq>(
    actual: ArrayShape<T>,
    expected: ArrayShape<T>,
) -> Result<ShapeCheck, ShapeMismatch> {
    if let (Some(actual), Some(expected)) = (&actual.elem_ty, &expected.elem_ty) {
        if actual != expected {
            return Err(ShapeMismatch::Element);
        }
    }
    if let (Some(actual), Some(expected)) = (actual.len, expected.len) {
        if actual != expected {
            return Err(ShapeMismatch::Length { expected, actual });
        }
    }
    if (expected.elem_ty.is_some() && actual.elem_ty.is_none())
        || (expected.len.is_some() && actual.len.is_none())
    {
        Ok(ShapeCheck::Deferred)
    } else {
        Ok(ShapeCheck::Complete)
    }
}

pub(crate) enum ArrayInitializer<'a> {
    Zero,
    Elements(&'a [Expr]),
    Value(&'a Expr),
}

impl<'a> ArrayInitializer<'a> {
    pub(crate) fn new(init: Option<&'a [Expr]>, init_is_value: bool) -> Self {
        match init {
            None => Self::Zero,
            Some([value]) if init_is_value => Self::Value(value),
            Some(values) => Self::Elements(values),
        }
    }
}

/// References retain their access permissions. Literals, constructors and array
/// returns produce values even when their payload is backed by a const cache.
pub(crate) fn is_array_reference(expr: &Expr) -> bool {
    matches!(expr, Expr::Var { .. } | Expr::Slice { .. })
}
