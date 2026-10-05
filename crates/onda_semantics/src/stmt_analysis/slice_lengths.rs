use super::*;
use std::sync::{Arc, OnceLock};

/// Slice bounds are captured as metadata; only a fixed storage initializer
/// demands their values. Aliases share both the recipe and its cached proof.
#[derive(Debug, Clone)]
pub(crate) enum SliceLength {
    Known(usize),
    Deferred(Arc<DeferredSliceLength>),
}

#[derive(Debug)]
pub(crate) struct DeferredSliceLength {
    recipe: SliceLengthRecipe,
    resolved: OnceLock<Option<usize>>,
}

#[derive(Debug)]
enum SliceLengthRecipe {
    Slice {
        source: SliceLength,
        start: Option<Expr>,
        end: Option<Expr>,
        options: AnalysisOptions,
    },
    Join(SliceLength, SliceLength),
}

impl DeferredSliceLength {
    fn take_sources(&mut self) -> [SliceLength; 2] {
        let take = |source: &mut SliceLength| std::mem::replace(source, SliceLength::Known(0));
        match &mut self.recipe {
            SliceLengthRecipe::Slice { source, .. } => [take(source), SliceLength::Known(0)],
            SliceLengthRecipe::Join(left, right) => [take(left), take(right)],
        }
    }
}

impl Drop for DeferredSliceLength {
    fn drop(&mut self) {
        // A long alias chain must also release its metadata without recursion.
        let sources = self.take_sources();
        if sources
            .iter()
            .all(|source| matches!(source, SliceLength::Known(_)))
        {
            return;
        }
        let mut pending = Vec::from(sources);
        while let Some(source) = pending.pop() {
            if let SliceLength::Deferred(source) = source {
                if let Ok(mut source) = Arc::try_unwrap(source) {
                    pending.extend(source.take_sources());
                }
            }
        }
    }
}

impl SliceLength {
    pub(crate) fn known(&self) -> Option<usize> {
        match self {
            Self::Known(len) => Some(*len),
            Self::Deferred(length) => length.resolved.get().copied().flatten(),
        }
    }

    pub(crate) fn slice(self, start: Option<&Expr>, end: Option<&Expr>, env: ExprEnv<'_>) -> Self {
        Self::deferred(SliceLengthRecipe::Slice {
            source: self,
            start: start
                .map(|bound| crate::expr_validation::substitute_array_length_metadata(bound, env)),
            end: end
                .map(|bound| crate::expr_validation::substitute_array_length_metadata(bound, env)),
            options: env.declared_symbols.options,
        })
    }

    pub(crate) fn join(left: Option<&Self>, right: Option<&Self>) -> Option<Self> {
        let (left, right) = (left?, right?);
        if let (Some(left), Some(right)) = (left.known(), right.known()) {
            return (left == right).then_some(Self::Known(left));
        }
        if let (Self::Deferred(left), Self::Deferred(right)) = (left, right) {
            if Arc::ptr_eq(left, right) {
                return Some(Self::Deferred(left.clone()));
            }
        }
        Some(Self::deferred(SliceLengthRecipe::Join(
            left.clone(),
            right.clone(),
        )))
    }

    fn deferred(recipe: SliceLengthRecipe) -> Self {
        Self::Deferred(Arc::new(DeferredSliceLength {
            recipe,
            resolved: OnceLock::new(),
        }))
    }

    pub(crate) fn resolve(&self, symbols: &DeclaredSymbolMap) -> Option<usize> {
        match self {
            Self::Known(len) => return Some(*len),
            Self::Deferred(length) => {
                if let Some(len) = length.resolved.get() {
                    return *len;
                }
            }
        }
        enum Visit<'a> {
            Length(&'a SliceLength),
            Finish(&'a DeferredSliceLength),
        }
        let mut pending = vec![Visit::Length(self)];
        let mut lengths = Vec::new();
        while let Some(visit) = pending.pop() {
            match visit {
                Visit::Length(Self::Known(len)) => lengths.push(Some(*len)),
                Visit::Length(Self::Deferred(length)) => {
                    if let Some(len) = length.resolved.get() {
                        lengths.push(*len);
                        continue;
                    }
                    pending.push(Visit::Finish(length));
                    match &length.recipe {
                        SliceLengthRecipe::Slice { source, .. } => {
                            pending.push(Visit::Length(source));
                        }
                        SliceLengthRecipe::Join(left, right) => {
                            pending.push(Visit::Length(right));
                            pending.push(Visit::Length(left));
                        }
                    }
                }
                Visit::Finish(length) => {
                    let len = match &length.recipe {
                        SliceLengthRecipe::Slice {
                            start,
                            end,
                            options,
                            ..
                        } => {
                            let mut symbols = symbols.clone();
                            symbols.options = *options;
                            prove_static_slice_len(
                                lengths.pop().unwrap(),
                                start.as_ref(),
                                end.as_ref(),
                                &symbols,
                            )
                        }
                        SliceLengthRecipe::Join(_, _) => {
                            let right = lengths.pop().unwrap();
                            lengths.pop().unwrap().filter(|left| Some(*left) == right)
                        }
                    };
                    let _ = length.resolved.set(len);
                    lengths.push(len);
                }
            }
        }
        lengths.pop().unwrap()
    }
}

pub(crate) fn prove_static_slice_len(
    total_len: Option<usize>,
    start: Option<&Expr>,
    end: Option<&Expr>,
    symbols: &DeclaredSymbolMap,
) -> Option<usize> {
    prove_slice_len(total_len, start, end, |bound| {
        if let Some(integer) = crate::builtins::eval_const_slice_integer(
            bound,
            symbols.options,
            "constant slice bound",
            &mut Vec::new(),
        ) {
            return Some(integer);
        }
        let raw = symbols.constant_integer(bound, &mut Vec::new())?;
        crate::builtins::coerce_slice_integer(TypedConstValue::I64(raw))
    })
}

/// Share slice normalization while letting each consumer choose whether bound
/// values may be demanded or only syntax-known metadata may be inspected.
pub(crate) fn prove_slice_len(
    total_len: Option<usize>,
    start: Option<&Expr>,
    end: Option<&Expr>,
    mut integer: impl FnMut(&Expr) -> Option<i64>,
) -> Option<usize> {
    let total_len = total_len?;
    let start = match start {
        Some(bound) => Some(integer(bound)?),
        None => None,
    };
    let end = match end {
        Some(bound) => Some(integer(bound)?),
        None => None,
    };
    Some(normalize_slice_len(total_len, start, end))
}

pub(crate) fn normalize_slice_len(total_len: usize, start: Option<i64>, end: Option<i64>) -> usize {
    let start = start.map_or(0, |raw| normalize_slice_bound(raw, total_len));
    let end = end.map_or(total_len, |raw| normalize_slice_bound(raw, total_len));
    end.saturating_sub(start)
}

pub(crate) fn normalize_slice_bound(raw: i64, total_len: usize) -> usize {
    let total_len = total_len as i128;
    let raw = i128::from(raw);
    let adjusted = if raw < 0 { total_len + raw } else { raw };
    adjusted.clamp(0, total_len) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_slice_chains_resolve_and_drop_on_a_worker_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let mut length = SliceLength::Known(2);
                for _ in 0..20_000 {
                    length = SliceLength::deferred(SliceLengthRecipe::Slice {
                        source: length,
                        start: Some(Expr::int(0)),
                        end: None,
                        options: AnalysisOptions::default(),
                    });
                }
                assert_eq!(length.resolve(&DeclaredSymbolMap::new()), Some(2));
                assert_eq!(length.known(), Some(2));
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
