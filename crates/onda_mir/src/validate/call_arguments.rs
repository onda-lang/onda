use super::*;

fn reference_access(mode: crate::PassingMode) -> crate::AccessMode {
    match mode {
        crate::PassingMode::ReadOnlyReference => crate::AccessMode::ReadOnly,
        crate::PassingMode::ReadWriteReference => crate::AccessMode::ReadWrite,
        crate::PassingMode::Value | crate::PassingMode::ResultReference => unreachable!(),
    }
}

impl Validator<'_> {
    pub(super) fn call_argument_matches(
        &self,
        function: &Function,
        argument: &CallArgument,
        parameter: &crate::FunctionParam,
    ) -> bool {
        if parameter.mode == crate::PassingMode::ResultReference {
            return matches!(argument, CallArgument::Place(place)
                if place.projections.is_empty()
                    && self.place_is_writable(function, place)
                    && self.place_type(function, place).is_some_and(|actual|
                        self.program.types_equivalent(actual, parameter.ty)));
        }
        match (parameter.mode, argument) {
            (crate::PassingMode::Value, CallArgument::Value(value)) => {
                self.value_matches_type(function, *value, parameter.ty)
            }
            (crate::PassingMode::Value, CallArgument::BufferSpan(span)) => {
                self.buffer_span_matches_type(function, *span, parameter.ty)
            }
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::Place(place),
            ) => self.place_type(function, place).is_some_and(|actual| {
                self.reference_type_matches(actual, parameter.ty)
                    && (!parameter.mode.is_writable_reference()
                        || self.place_is_writable(function, place))
            }),
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::SliceElement { slice, .. },
            ) => {
                let requested_access = reference_access(parameter.mode);
                self.value_slice_type(function, *slice).is_some_and(
                    |(slice_element, slice_access)| {
                        access_permits(slice_access, requested_access)
                            && match self.program.types.get(parameter.ty.index()) {
                                Some(Type::Scalar(expected)) => *expected == slice_element,
                                _ => false,
                            }
                    },
                )
            }
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::ArrayWindow {
                    array,
                    start,
                    bounds,
                },
            ) => {
                let requested_access = reference_access(parameter.mode);
                let Some(Type::Array {
                    element: expected_element,
                    len: required_len,
                }) = self.program.types.get(parameter.ty.index())
                else {
                    return false;
                };
                let Some(actual_ty) = self.place_type(function, array) else {
                    return false;
                };
                let Some(Type::Array {
                    element: actual_element,
                    len: actual_len,
                }) = self.program.types.get(actual_ty.index())
                else {
                    return false;
                };
                self.program
                    .types_equivalent(*actual_element, *expected_element)
                    && required_len <= actual_len
                    && access_permits(
                        if self.place_is_writable(function, array) {
                            crate::AccessMode::ReadWrite
                        } else {
                            crate::AccessMode::ReadOnly
                        },
                        requested_access,
                    )
                    && self.window_start_is_statically_valid(
                        *start,
                        *bounds,
                        *actual_len,
                        *required_len,
                    )
            }
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::SliceWindow { slice, .. },
            ) => {
                let requested_access = reference_access(parameter.mode);
                let Some(Type::Array { element, .. }) =
                    self.program.types.get(parameter.ty.index())
                else {
                    return false;
                };
                let Some(Type::Scalar(expected_element)) = self.program.types.get(element.index())
                else {
                    return false;
                };
                self.value_slice_type(function, *slice).is_some_and(
                    |(slice_element, slice_access)| {
                        slice_element == *expected_element
                            && access_permits(slice_access, requested_access)
                    },
                )
            }
            (_, CallArgument::Buffer(buffer)) => self.buffer_matches_type(*buffer, parameter.ty),
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::BufferParam(reference),
            ) => self.buffer_param_ref_matches_type(function, *reference, parameter.ty),
            _ => false,
        }
    }

    fn buffer_matches_type(&self, buffer: crate::BufferRef, expected: crate::TypeId) -> bool {
        let Some(first_buffer) = self.program.interface.buffers.get(buffer.index()) else {
            return false;
        };
        let matches = self.program.types.get(expected.index()).is_some_and(|ty| {
            matches!(
                ty,
                Type::Buffer {
                    element,
                    channels,
                    access,
                } if *element == first_buffer.element
                    && buffer_channels_accept(*channels, first_buffer.channels)
                    && access_permits(first_buffer.access, *access)
            )
        });
        matches
            && buffer.possible_indices().all(|index| {
                self.program
                    .interface
                    .buffers
                    .get(index)
                    .is_some_and(|candidate| {
                        candidate.element == first_buffer.element
                            && candidate.channels == first_buffer.channels
                            && candidate.access == first_buffer.access
                    })
            })
    }

    fn buffer_span_matches_type(
        &self,
        function: &Function,
        span: crate::BufferSpanRef,
        expected: crate::TypeId,
    ) -> bool {
        let Some(Type::BufferSpan {
            element: expected_element,
            channels: expected_channels,
            access: expected_access,
            len: expected_len,
        }) = self.program.types.get(expected.index())
        else {
            return false;
        };
        match span {
            crate::BufferSpanRef::Interface { first, len } => {
                if len != *expected_len {
                    return false;
                }
                let Some(source) = self.program.interface.buffers.get(first.index()) else {
                    return false;
                };
                source.element == *expected_element
                    && buffer_channels_accept(*expected_channels, source.channels)
                    && access_permits(source.access, *expected_access)
                    && (first.index()..first.index().saturating_add(len as usize)).all(|index| {
                        self.program
                            .interface
                            .buffers
                            .get(index)
                            .is_some_and(|candidate| {
                                candidate.element == source.element
                                    && candidate.channels == source.channels
                                    && candidate.access == source.access
                            })
                    })
            }
            crate::BufferSpanRef::Parameter { span, start, len } => {
                if len != *expected_len {
                    return false;
                }
                let Some(source) = function.params.get(span.index()) else {
                    return false;
                };
                let Some(Type::BufferSpan {
                    element,
                    channels,
                    access,
                    len: source_len,
                }) = self.program.types.get(source.ty.index())
                else {
                    return false;
                };
                start.checked_add(len).is_some_and(|end| end <= *source_len)
                    && element == expected_element
                    && buffer_channels_accept(*expected_channels, *channels)
                    && access_permits(*access, *expected_access)
            }
        }
    }

    fn reference_type_matches(&self, actual: crate::TypeId, expected: crate::TypeId) -> bool {
        match (
            self.program.types.get(actual.index()),
            self.program.types.get(expected.index()),
        ) {
            (
                Some(Type::Buffer {
                    element: actual_element,
                    channels: actual_channels,
                    access: actual_access,
                }),
                Some(Type::Buffer {
                    element: expected_element,
                    channels: expected_channels,
                    access: expected_access,
                }),
            ) => {
                actual_element == expected_element
                    && buffer_channels_accept(*expected_channels, *actual_channels)
                    && access_permits(*actual_access, *expected_access)
            }
            _ => self.program.types_equivalent(actual, expected),
        }
    }

    fn function_buffer_param(
        &self,
        function: &Function,
        parameter: crate::ParameterId,
    ) -> Option<(crate::ScalarType, crate::AccessMode)> {
        let parameter = function.params.get(parameter.index())?;
        match self.program.types.get(parameter.ty.index())? {
            Type::Buffer {
                element, access, ..
            } => Some((*element, *access)),
            _ => None,
        }
    }

    fn buffer_param_ref_matches_type(
        &self,
        function: &Function,
        reference: crate::BufferParamRef,
        expected: crate::TypeId,
    ) -> bool {
        let actual = function
            .params
            .get(reference.index())
            .and_then(|parameter| self.program.types.get(parameter.ty.index()));
        let (actual_element, actual_channels, actual_access) = match (reference, actual) {
            (
                crate::BufferParamRef::Direct(_),
                Some(Type::Buffer {
                    element,
                    channels,
                    access,
                }),
            )
            | (
                crate::BufferParamRef::ArrayElement { .. },
                Some(Type::BufferSpan {
                    element,
                    channels,
                    access,
                    ..
                }),
            ) => (*element, *channels, *access),
            _ => return false,
        };
        matches!(
            self.program.types.get(expected.index()),
            Some(Type::Buffer {
                element,
                channels,
                access,
            }) if *element == actual_element
                && buffer_channels_accept(*channels, actual_channels)
                && access_permits(actual_access, *access)
        )
    }

    pub(super) fn function_buffer_param_ref(
        &self,
        function: &Function,
        reference: crate::BufferParamRef,
    ) -> Option<(crate::ScalarType, crate::AccessMode)> {
        match reference {
            crate::BufferParamRef::Direct(parameter) => {
                self.function_buffer_param(function, parameter)
            }
            crate::BufferParamRef::ArrayElement { span, .. } => {
                let parameter = function.params.get(span.index())?;
                match self.program.types.get(parameter.ty.index())? {
                    Type::BufferSpan {
                        element, access, ..
                    } => Some((*element, *access)),
                    _ => None,
                }
            }
        }
    }

    pub(super) fn validate_buffer_param_ref(
        &mut self,
        function_id: crate::FunctionId,
        function: &Function,
        reference: crate::BufferParamRef,
        source: crate::SourceSpan,
    ) {
        if let crate::BufferParamRef::ArrayElement { selector, .. } = reference {
            self.validate_value(function_id, function, selector, source);
            self.require_i32_value(
                function_id,
                function,
                selector,
                source,
                "buffer-parameter collection selector",
            );
        }
    }
}
