//! Array bindings retain declaration metadata and share payloads until a read.
//! Aliases share local storage; explicit copies share only immutable payloads.
use super::*;
use std::cell::Ref;
use std::ops::Range;
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct ArrayBinding<'a> {
    storage: Rc<RefCell<ArrayStorage<'a>>>,
    range: Range<usize>,
}

struct ArrayStorage<'a> {
    source: ArraySource<'a>,
    range: Range<usize>,
}

#[derive(Clone)]
enum ArraySource<'a> {
    Value(ConstEvalArray),
    Declaration(ConstEvaluation<'a>),
}

impl<'a> ArrayBinding<'a> {
    fn new(source: ArraySource<'a>, range: Range<usize>) -> Self {
        Self {
            range: 0..range.len(),
            storage: Rc::new(RefCell::new(ArrayStorage { source, range })),
        }
    }

    pub(super) fn declaration(evaluation: ConstEvaluation<'a>) -> Option<Self> {
        let len = evaluation.entry.array?.len;
        Some(Self::new(ArraySource::Declaration(evaluation), 0..len))
    }

    pub(super) fn evaluation(&self) -> Option<ConstEvaluation<'a>> {
        match &self.storage.borrow().source {
            ArraySource::Declaration(evaluation) => Some(evaluation.clone()),
            ArraySource::Value(_) => None,
        }
    }

    pub(super) fn elem_ty(&self) -> PrimitiveType {
        match &self.storage.borrow().source {
            ArraySource::Value(array) => array.elem_ty,
            ArraySource::Declaration(evaluation) => evaluation.entry.array.unwrap().elem_ty,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.range.len()
    }

    fn snapshot(&self) -> ArrayStorage<'a> {
        let storage = self.storage.borrow();
        ArrayStorage {
            source: storage.source.clone(),
            range: storage.range.start + self.range.start..storage.range.start + self.range.end,
        }
    }

    /// A new owner captures the current contents without demanding a payload.
    pub(super) fn copy(&self) -> Self {
        let snapshot = self.snapshot();
        Self::new(snapshot.source, snapshot.range)
    }

    pub(super) fn values(&self) -> Option<Ref<'_, [TypedConstValue]>> {
        Ref::filter_map(self.storage.borrow(), |storage| {
            Some(&storage.values()?[self.range.clone()])
        })
        .ok()
    }

    pub(super) fn slice(mut self, start: usize, end: usize) -> Self {
        self.range = self.range.start + start..self.range.start + end;
        self
    }

    pub(super) fn materialize(&self) -> Option<ConstEvalArray> {
        self.snapshot().materialize()
    }

    pub(super) fn covers_storage(&self) -> bool {
        self.range == (0..self.storage.borrow().range.len())
    }

    pub(super) fn replace(
        &self,
        value: &Self,
        context: &str,
        loc: SourceLoc,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<()> {
        if !check_const_array_shape(
            value.elem_ty(),
            value.len(),
            ConstArrayExpectation::fixed(self.elem_ty(), self.len()),
            context,
            loc,
            errors,
        ) {
            return None;
        }
        // Capture first, including self-assignment, before borrowing the target.
        let snapshot = value.snapshot();
        let mut storage = self.storage.borrow_mut();
        if self.range == (0..storage.range.len()) {
            *storage = snapshot;
        } else {
            let values = snapshot.materialize()?;
            // The source slice is captured. Release its backing payload before
            // copy-on-write so self-copies do not clone the entire array.
            drop(snapshot);
            let array = storage.array_mut()?;
            std::sync::Arc::make_mut(&mut array.values)[self.range.clone()]
                .copy_from_slice(&values.values);
        }
        Some(())
    }

    pub(super) fn write(&self, index: usize, value: TypedConstValue) -> Option<()> {
        let mut storage = self.storage.borrow_mut();
        let array = storage.array_mut()?;
        std::sync::Arc::make_mut(&mut array.values)[self.range.start + index] = value;
        Some(())
    }
}

impl ArrayStorage<'_> {
    fn array_mut(&mut self) -> Option<&mut ConstEvalArray> {
        if !matches!(&self.source, ArraySource::Value(array) if self.range == (0..array.len())) {
            let array = self.materialize()?;
            self.range = 0..array.len();
            self.source = ArraySource::Value(array);
        }
        let ArraySource::Value(array) = &mut self.source else {
            unreachable!()
        };
        Some(array)
    }

    fn array(&self) -> Option<&ConstEvalArray> {
        match &self.source {
            ArraySource::Value(array) => Some(array),
            ArraySource::Declaration(evaluation) => {
                match evaluation.evaluated.get()?.as_ref().ok()? {
                    ResolvedConstValue::Array(array) => Some(array),
                    _ => None,
                }
            }
        }
    }

    fn values(&self) -> Option<&[TypedConstValue]> {
        Some(&self.array()?.values[self.range.clone()])
    }

    fn materialize(&self) -> Option<ConstEvalArray> {
        let array = self.array()?;
        Some(if self.range == (0..array.len()) {
            array.clone()
        } else {
            ConstEvalArray {
                elem_ty: array.elem_ty,
                values: self.values()?.to_vec().into(),
            }
        })
    }
}

impl From<ConstEvalArray> for ArrayBinding<'_> {
    fn from(array: ConstEvalArray) -> Self {
        let len = array.len();
        Self::new(ArraySource::Value(array), 0..len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn array() -> ArrayBinding<'static> {
        ConstEvalArray {
            elem_ty: PrimitiveType::I32,
            values: vec![1, 2, 3, 4]
                .into_iter()
                .map(TypedConstValue::I32)
                .collect::<Vec<_>>()
                .into(),
        }
        .into()
    }

    #[test]
    fn partial_self_copies_preserve_overlap_without_cloning_the_payload() {
        for (target, source, expected) in [
            (1..3, 0..2, [1, 1, 2, 4]),
            (0..2, 1..3, [2, 3, 3, 4]),
            (2..3, 0..1, [1, 2, 1, 4]),
        ] {
            let array = array();
            let payload = array.storage.borrow().array().unwrap().values.as_ptr();
            let target = array.clone().slice(target.start, target.end);
            let source = array.clone().slice(source.start, source.end);
            let mut errors = Vec::new();
            target
                .replace(&source, "test copy", SourceLoc::default(), &mut errors)
                .unwrap();
            assert!(errors.is_empty());
            assert_eq!(
                &*array.values().unwrap(),
                &expected.map(TypedConstValue::I32)
            );
            assert_eq!(
                array.storage.borrow().array().unwrap().values.as_ptr(),
                payload
            );
        }
    }

    #[test]
    fn partial_self_copies_keep_independent_snapshots_unchanged() {
        let array = array();
        let copy = array.copy();
        let target = array.clone().slice(1, 3);
        let source = array.clone().slice(0, 2);
        let mut errors = Vec::new();
        target
            .replace(&source, "test copy", SourceLoc::default(), &mut errors)
            .unwrap();
        assert!(errors.is_empty());
        assert_eq!(
            &*array.values().unwrap(),
            &[1, 1, 2, 4].map(TypedConstValue::I32)
        );
        assert_eq!(
            &*copy.values().unwrap(),
            &[1, 2, 3, 4].map(TypedConstValue::I32)
        );
    }
}
