use super::*;

/// Builds the backend-neutral per-frame output transaction.
///
/// Sample code mutates local caches, never host output memory directly. A
/// single ordered commit at the end of the sample gives backends a simple
/// store frontier, avoids redundant zero stores, and enables scalar promotion
/// and loop vectorization without changing Onda's zero-default semantics.
impl FunctionLowerer<'_> {
    pub(super) fn begin_audio_output_frame(
        &mut self,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let globals = self.runtime_globals.ok_or_else(|| {
            self.error(
                "audio output initialization requires runtime interface metadata",
                location,
            )
        })?;
        debug_assert!(self.audio_output_caches.is_empty());
        debug_assert!(self.audio_output_endpoint_caches.is_empty());
        debug_assert!(self.audio_output_array_caches.is_empty());

        let mut scalar_outputs = globals
            .outputs
            .iter()
            .map(|(name, (output, ty))| (name.clone(), *output, *ty))
            .collect::<Vec<_>>();
        scalar_outputs.sort_by_key(|(_, output, _)| output.raw());
        for (name, output, ty) in scalar_outputs {
            let cache = self.new_local(Some(format!("$output.{name}.current")), ty);
            self.assign_value(block, cache, zero_value(ty), location);
            self.audio_output_caches.insert(name, (cache, ty));
            self.audio_output_endpoint_caches
                .insert(output, (cache, ty));
        }

        let mut array_outputs = globals
            .output_arrays
            .iter()
            .map(|(name, (output, ty, len))| (name.clone(), *output, *ty, *len))
            .collect::<Vec<_>>();
        array_outputs.sort_by_key(|(_, output, _, _)| output.raw());
        for (name, output, ty, len) in array_outputs {
            let cache = self.new_array_local(Some(format!("$output.{name}.current")), ty, len);
            for element in 0..len {
                self.assign_place_value(
                    block,
                    Place {
                        base: PlaceBase::Local(cache),
                        projections: vec![Projection::Index {
                            index: Value::Constant(ScalarValue::I32(element as i32)),
                            bounds: BoundsMode::Unchecked,
                        }],
                    },
                    zero_value(ty),
                    location,
                );
            }
            self.audio_output_array_caches
                .insert(output, (cache, ty, len));
        }
        Ok(())
    }

    pub(super) fn commit_audio_output_frame(
        &mut self,
        block: &mut MirBlock,
        frame: Value,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let globals = self.runtime_globals.ok_or_else(|| {
            self.error(
                "audio output commit requires runtime interface metadata",
                location,
            )
        })?;
        let mut scalar_outputs = globals
            .outputs
            .iter()
            .map(|(name, (output, ty))| (name.clone(), *output, *ty))
            .collect::<Vec<_>>();
        scalar_outputs.sort_by_key(|(_, output, _)| output.raw());
        for (name, output, ty) in scalar_outputs {
            let (cache, cache_ty) = self.audio_output_caches[&name];
            debug_assert_eq!(cache_ty, ty);
            self.push_statement(
                block,
                StatementKind::OutputStore {
                    output,
                    element: None,
                    bounds: BoundsMode::Unchecked,
                    frame,
                    value: Value::Local(cache),
                },
                location,
            );
        }

        let mut array_outputs = globals.output_arrays.values().copied().collect::<Vec<_>>();
        array_outputs.sort_by_key(|(output, _, _)| output.raw());
        for (output, ty, len) in array_outputs {
            let (cache, cache_ty, cache_len) = self.audio_output_array_caches[&output];
            debug_assert_eq!((cache_ty, cache_len), (ty, len));
            for element in 0..len {
                let index = Value::Constant(ScalarValue::I32(element as i32));
                let value = self.load_place_value(
                    block,
                    ty,
                    &Place {
                        base: PlaceBase::Local(cache),
                        projections: vec![Projection::Index {
                            index,
                            bounds: BoundsMode::Unchecked,
                        }],
                    },
                    location,
                );
                self.push_statement(
                    block,
                    StatementKind::OutputStore {
                        output,
                        element: Some(index),
                        bounds: BoundsMode::Unchecked,
                        frame,
                        value,
                    },
                    location,
                );
            }
        }
        Ok(())
    }

    pub(super) fn clear_audio_output_caches(&mut self) {
        self.audio_output_caches.clear();
        self.audio_output_endpoint_caches.clear();
        self.audio_output_array_caches.clear();
    }
}
