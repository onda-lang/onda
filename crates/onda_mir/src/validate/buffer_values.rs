use super::*;

impl Validator<'_> {
    pub(super) fn validate_buffer_value(
        &mut self,
        id: FunctionId,
        function: &Function,
        argument: &CallArgument,
        ty: crate::TypeId,
        source: SourceSpan,
    ) {
        self.require_type(ty, Some(id), source);
        let mode = match self.program.types.get(ty.index()) {
            Some(Type::Buffer { .. }) => crate::PassingMode::ReadOnlyReference,
            Some(Type::BufferSpan { .. } | Type::BufferMap { .. }) => crate::PassingMode::Value,
            _ => {
                self.function_error(
                    id,
                    source,
                    "make_buffer requires a buffer or collection type",
                );
                return;
            }
        };
        let parameter = crate::FunctionParam {
            name: String::new(),
            ty,
            mode,
            integer_range: None,
        };
        if !self.call_argument_matches(function, argument, &parameter) {
            self.function_error(
                id,
                source,
                "make_buffer source does not match its logical type",
            );
        }
        match argument {
            CallArgument::Buffer(buffer) => {
                self.require_direct_buffer_capability(id, function, source, "make_buffer");
                self.require_buffer(id, function, *buffer, source);
            }
            CallArgument::BufferParam(reference) => {
                self.validate_buffer_param_ref(id, function, *reference, source)
            }
            CallArgument::Value(value) => self.validate_value(id, function, *value, source),
            CallArgument::Place(place) => self.validate_place(id, function, place, source),
            CallArgument::BufferSpan(span) => {
                self.validate_buffer_span_value(id, function, *span, source)
            }
            _ => self.function_error(id, source, "make_buffer source is not a buffer descriptor"),
        }
    }

    pub(super) fn validate_buffer_span_value(
        &mut self,
        id: FunctionId,
        function: &Function,
        span: crate::BufferSpanRef,
        source: SourceSpan,
    ) {
        if let crate::BufferSpanRef::Local {
            collection, start, ..
        } = span
        {
            self.require_local(id, function, collection, source);
            self.require_i32_value(
                id,
                function,
                start,
                source,
                "buffer collection window start",
            );
        }
    }

    pub(super) fn assignment_read_buffer_argument(
        &mut self,
        id: FunctionId,
        function: &Function,
        argument: &CallArgument,
        source: SourceSpan,
        state: &AssignmentState,
    ) {
        match argument {
            CallArgument::Value(value) => {
                self.assignment_read_value(id, function, *value, source, state)
            }
            CallArgument::Place(place) => {
                self.assignment_read_place(id, function, place, source, state)
            }
            CallArgument::Buffer(buffer) => {
                self.assignment_read_buffer_ref(id, function, *buffer, source, state)
            }
            CallArgument::BufferParam(reference) => {
                self.assignment_read_buffer_param_ref(id, function, *reference, source, state)
            }
            CallArgument::BufferSpan(crate::BufferSpanRef::Local {
                collection, start, ..
            }) => {
                self.assignment_read_value(id, function, Value::Local(*collection), source, state);
                self.assignment_read_value(id, function, *start, source, state);
            }
            _ => {}
        }
    }
}
