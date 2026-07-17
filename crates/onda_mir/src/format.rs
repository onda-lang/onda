use std::fmt::Write;

use crate::*;

/// Formats a complete MIR program in a stable, human-readable form.
///
/// The format is intended for diagnostics, golden tests, and `--dump-mir`.
/// It is deliberately not a serialization format.
pub fn format_program(program: &Program) -> String {
    Formatter::new(program).format()
}

struct Formatter<'a> {
    program: &'a Program,
    output: String,
}

impl<'a> Formatter<'a> {
    fn new(program: &'a Program) -> Self {
        Self {
            program,
            output: String::new(),
        }
    }

    fn format(mut self) -> String {
        self.line(format_args!("mir {}", self.program.schema_version));
        self.line(format_args!(
            "config sample_rate={:?} block_size={}",
            self.program.config.sample_rate, self.program.config.block_size
        ));

        for (index, source) in self.program.source_files.iter().enumerate() {
            self.line(format_args!("source @file{index} {:?}", source.path));
        }
        for (index, ty) in self.program.types.iter().enumerate() {
            self.line(format_args!("type @t{index} = {}", self.format_type(ty)));
        }
        for (index, def) in self.program.structs.iter().enumerate() {
            self.line(format_args!("struct @s{index} {:?} {{", def.name));
            for (field_index, field) in def.fields.iter().enumerate() {
                self.line(format_args!(
                    "  field @f{field_index} {:?}: {}",
                    field.name,
                    type_id(field.ty)
                ));
            }
            self.line(format_args!("}}"));
        }

        self.format_interface();

        for (index, slot) in self.program.state.iter().enumerate() {
            self.line(format_args!(
                "state @state{index} {:?}: {} {}",
                slot.name,
                type_id(slot.ty),
                match slot.persistence {
                    StatePersistence::Snapshot => "snapshot",
                    StatePersistence::InstanceScratch => "instance_scratch",
                    StatePersistence::ControlMirror => "control_mirror",
                }
            ));
        }
        for (index, data) in self.program.const_data.iter().enumerate() {
            let values = data
                .values
                .iter()
                .copied()
                .map(format_scalar)
                .collect::<Vec<_>>()
                .join(", ");
            self.line(format_args!(
                "const_data @data{index} {:?}: {} = [{values}]",
                data.name,
                data.element.name()
            ));
        }

        self.line(format_args!(
            "entry init={} process={}",
            function_id(self.program.entry_points.init),
            function_id(self.program.entry_points.process)
        ));
        for (index, function) in self.program.functions.iter().enumerate() {
            self.format_function(FunctionId::new(index as u32), function);
        }

        self.output
    }

    fn format_interface(&mut self) {
        for (index, input) in self.program.interface.inputs.iter().enumerate() {
            let mut suffix = String::new();
            if let Some(default) = &input.default {
                write!(suffix, " default={}", format_constant(default)).expect("writing a string");
            }
            if let Some(range) = input.range {
                write!(
                    suffix,
                    " range={}..={}",
                    format_scalar(range.min),
                    format_scalar(range.max)
                )
                .expect("writing a string");
            }
            self.line(format_args!(
                "input @in{index} {:?}: {}{suffix}",
                input.name,
                type_id(input.ty)
            ));
        }
        for (index, output) in self.program.interface.outputs.iter().enumerate() {
            self.line(format_args!(
                "output @out{index} {:?}: {}",
                output.name,
                type_id(output.ty)
            ));
        }
        for (index, output) in self.program.interface.control_outputs.iter().enumerate() {
            self.line(format_args!(
                "control_output @kout{index} {:?}: {} mirror=@state{}",
                output.name,
                type_id(output.ty),
                output.mirror.raw()
            ));
        }
        for (index, param) in self.program.interface.params.iter().enumerate() {
            let mut suffix = format!(" default={}", format_constant(&param.default));
            if let Some(range) = param.range {
                write!(
                    suffix,
                    " range={}..={}",
                    format_scalar(range.min),
                    format_scalar(range.max)
                )
                .expect("writing a string");
            }
            self.line(format_args!(
                "param @param{index} {:?}: {}{suffix}",
                param.name,
                type_id(param.ty)
            ));
        }
        for (index, buffer) in self.program.interface.buffers.iter().enumerate() {
            self.line(format_args!(
                "buffer @buffer{index} {:?}: {} channels={} access={}",
                buffer.name,
                buffer.element.name(),
                format_channels(buffer.channels),
                format_access(buffer.access)
            ));
        }
        for (index, event) in self.program.interface.events.iter().enumerate() {
            self.line(format_args!(
                "event @event{index} {:?} handler={} {{",
                event.name,
                function_id(event.handler)
            ));
            for (param_index, param) in event.params.iter().enumerate() {
                let default = param
                    .default
                    .as_ref()
                    .map(|value| format!(" default={}", format_constant(value)))
                    .unwrap_or_default();
                self.line(format_args!(
                    "  event_param @event_param{param_index} {:?}: {}{default}",
                    param.name,
                    type_id(param.ty)
                ));
            }
            self.line(format_args!("}}"));
        }
    }

    fn format_function(&mut self, id: FunctionId, function: &Function) {
        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                format!(
                    "@p{index} {:?}: {} {}",
                    param.name,
                    type_id(param.ty),
                    format_passing_mode(param.mode)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let results = function
            .results
            .iter()
            .copied()
            .map(type_id)
            .collect::<Vec<_>>()
            .join(", ");
        let source = format_source(function.source);
        self.line(format_args!(
            "fn {} {:?} {} origin={} inline={} ({params}) -> ({results}){source} {{",
            function_id(id),
            function.name,
            format_function_kind(function.kind),
            format_function_origin(function.attributes.origin),
            format_inline_hint(function.attributes.inline),
        ));
        for (index, local) in function.locals.iter().enumerate() {
            let name = local
                .name
                .as_ref()
                .map(|name| format!(" {:?}", name))
                .unwrap_or_default();
            self.line(format_args!(
                "  local %{index}{name}: {}",
                type_id(local.ty)
            ));
        }
        self.format_block(&function.body, 1);
        self.line(format_args!("}}"));
    }

    fn format_block(&mut self, block: &Block, indent: usize) {
        for statement in &block.statements {
            self.format_statement(statement, indent);
        }
    }

    fn format_statement(&mut self, statement: &Statement, indent: usize) {
        let pad = "  ".repeat(indent);
        let source = format_source(statement.source);
        match &statement.kind {
            StatementKind::Assign { destination, value } => self.line(format_args!(
                "{pad}{} = {}{source}",
                format_place(destination),
                format_rvalue(value)
            )),
            StatementKind::Call {
                results,
                function,
                args,
            } => {
                let results = results
                    .iter()
                    .map(|id| local_id(*id))
                    .collect::<Vec<_>>()
                    .join(", ");
                let args = args
                    .iter()
                    .map(format_call_argument)
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(format_args!(
                    "{pad}({results}) = call {}({args}){source}",
                    function_id(*function)
                ));
            }
            StatementKind::OutputStore {
                output,
                element,
                bounds,
                frame,
                value,
            } => self.line(format_args!(
                "{pad}store_output @out{}{}[{}] {}, {}{source}",
                output.raw(),
                format_optional_index(element.as_ref(), *bounds),
                format_value(*frame),
                format_bounds(*bounds),
                format_value(*value)
            )),
            StatementKind::ControlOutputStore {
                output,
                element,
                bounds,
                value,
            } => self.line(format_args!(
                "{pad}store_control_output @kout{}{} {}, {}{source}",
                output.raw(),
                format_optional_index(element.as_ref(), *bounds),
                format_bounds(*bounds),
                format_value(*value)
            )),
            StatementKind::BufferStore {
                buffer,
                channel,
                index,
                value,
                bounds,
            } => self.line(format_args!(
                "{pad}store_buffer @buffer{}{}[{}] {}, {}{source}",
                buffer.raw(),
                channel
                    .map(|value| format!("[{}]", format_value(value)))
                    .unwrap_or_default(),
                format_value(*index),
                format_bounds(*bounds),
                format_value(*value)
            )),
            StatementKind::BufferParamStore {
                parameter,
                channel,
                index,
                value,
                bounds,
            } => self.line(format_args!(
                "{pad}store_buffer_param @param{}{}[{}] {}, {}{source}",
                parameter.raw(),
                channel
                    .map(|value| format!("[{}]", format_value(value)))
                    .unwrap_or_default(),
                format_value(*index),
                format_bounds(*bounds),
                format_value(*value)
            )),
            StatementKind::SliceStore {
                slice,
                index,
                value,
                bounds,
            } => self.line(format_args!(
                "{pad}store_slice {}[{}] {}, {}{source}",
                format_value(*slice),
                format_value(*index),
                format_bounds(*bounds),
                format_value(*value)
            )),
            StatementKind::SliceFill { destination, value } => self.line(format_args!(
                "{pad}slice_fill {}, {}{source}",
                format_value(*destination),
                format_value(*value)
            )),
            StatementKind::SliceCopy {
                destination,
                source: copy_source,
            } => self.line(format_args!(
                "{pad}slice_copy {}, {}{source}",
                format_value(*destination),
                format_value(*copy_source)
            )),
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                self.line(format_args!(
                    "{pad}if {}{source} {{",
                    format_value(*condition)
                ));
                self.format_block(then_block, indent + 1);
                self.line(format_args!("{pad}}} else {{"));
                self.format_block(else_block, indent + 1);
                self.line(format_args!("{pad}}}"));
            }
            StatementKind::Loop { body } => {
                self.line(format_args!("{pad}loop{source} {{"));
                self.format_block(body, indent + 1);
                self.line(format_args!("{pad}}}"));
            }
            StatementKind::Break => self.line(format_args!("{pad}break{source}")),
            StatementKind::Continue => self.line(format_args!("{pad}continue{source}")),
            StatementKind::Return { values } => {
                let values = values
                    .iter()
                    .copied()
                    .map(format_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(format_args!("{pad}return ({values}){source}"));
            }
        }
    }

    fn format_type(&self, ty: &Type) -> String {
        match ty {
            Type::Scalar(scalar) => scalar.name().to_owned(),
            Type::Tuple(elements) => format!(
                "({})",
                elements
                    .iter()
                    .copied()
                    .map(type_id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Type::Array { element, len } => format!("[{}; {len}]", type_id(*element)),
            Type::Struct(id) => format!("@s{}", id.raw()),
            Type::Slice { element, access } => {
                format!("slice<{}, {}>", element.name(), format_access(*access))
            }
            Type::Buffer {
                element,
                channels,
                access,
            } => format!(
                "buffer<{}, {}, {}>",
                element.name(),
                format_channels(*channels),
                format_access(*access)
            ),
        }
    }

    fn line(&mut self, args: std::fmt::Arguments<'_>) {
        self.output.write_fmt(args).expect("writing a string");
        self.output.push('\n');
    }
}

fn type_id(id: TypeId) -> String {
    format!("@t{}", id.raw())
}

fn function_id(id: FunctionId) -> String {
    format!("@fn{}", id.raw())
}

fn local_id(id: LocalId) -> String {
    format!("%{}", id.raw())
}

fn format_scalar(value: ScalarValue) -> String {
    match value {
        ScalarValue::F32(value) => format!("f32({value:?})"),
        ScalarValue::F64(value) => format!("f64({value:?})"),
        ScalarValue::I32(value) => format!("i32({value})"),
        ScalarValue::I64(value) => format!("i64({value})"),
        ScalarValue::Bool(value) => format!("bool({value})"),
    }
}

fn format_constant(value: &ConstantValue) -> String {
    match value {
        ConstantValue::Scalar(value) => format_scalar(*value),
        ConstantValue::Aggregate(values) => format!(
            "[{}]",
            values
                .iter()
                .map(format_constant)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn format_value(value: Value) -> String {
    match value {
        Value::Local(id) => local_id(id),
        Value::Constant(value) => format_scalar(value),
    }
}

fn format_place(place: &Place) -> String {
    let mut result = match place.base {
        PlaceBase::Local(id) => local_id(id),
        PlaceBase::Parameter(id) => format!("@p{}", id.raw()),
        PlaceBase::State(id) => format!("@state{}", id.raw()),
        PlaceBase::Param(id) => format!("@param{}", id.raw()),
        PlaceBase::EventParam(id) => format!("@event_param{}", id.raw()),
    };
    for projection in &place.projections {
        match projection {
            Projection::Field(id) => write!(result, ".@f{}", id.raw()),
            Projection::Index { index, bounds } => write!(
                result,
                "[{}] {}",
                format_value(*index),
                format_bounds(*bounds)
            ),
        }
        .expect("writing a string");
    }
    result
}

fn format_rvalue(value: &Rvalue) -> String {
    match value {
        Rvalue::Use(value) => format_value(*value),
        Rvalue::Load(place) => format!("load {}", format_place(place)),
        Rvalue::Unary { op, operand } => {
            format!("{} {}", format_unary(*op), format_value(*operand))
        }
        Rvalue::Binary { op, lhs, rhs } => format!(
            "{} {}, {}",
            format_binary(*op),
            format_value(*lhs),
            format_value(*rhs)
        ),
        Rvalue::Compare { op, lhs, rhs } => format!(
            "{} {}, {}",
            format_compare(*op),
            format_value(*lhs),
            format_value(*rhs)
        ),
        Rvalue::Cast { value, to } => {
            format!("cast {} to {}", format_value(*value), to.name())
        }
        Rvalue::Intrinsic { intrinsic, args } => format!(
            "intrinsic {}({})",
            format_intrinsic(*intrinsic),
            args.iter()
                .copied()
                .map(format_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Rvalue::ProcessFrame { offset } => {
            format!("process_frame {}", format_value(*offset))
        }
        Rvalue::InputLoad {
            input,
            element,
            bounds,
            frame,
        } => format!(
            "load_input @in{}{}[{}] {}",
            input.raw(),
            format_optional_index(element.as_ref(), *bounds),
            format_value(*frame),
            format_bounds(*bounds)
        ),
        Rvalue::OutputLoad {
            output,
            element,
            bounds,
            frame,
        } => format!(
            "load_output @out{}{}[{}] {}",
            output.raw(),
            format_optional_index(element.as_ref(), *bounds),
            format_value(*frame),
            format_bounds(*bounds)
        ),
        Rvalue::BufferLoad {
            buffer,
            channel,
            index,
            bounds,
        } => format!(
            "load_buffer @buffer{}{}[{}] {}",
            buffer.raw(),
            channel
                .map(|value| format!("[{}]", format_value(value)))
                .unwrap_or_default(),
            format_value(*index),
            format_bounds(*bounds)
        ),
        Rvalue::BufferParamLoad {
            parameter,
            channel,
            index,
            bounds,
        } => format!(
            "load_buffer_param @param{}{}[{}] {}",
            parameter.raw(),
            channel
                .map(|value| format!("[{}]", format_value(value)))
                .unwrap_or_default(),
            format_value(*index),
            format_bounds(*bounds)
        ),
        Rvalue::BufferLen(id) => format!("buffer_len @buffer{}", id.raw()),
        Rvalue::BufferChannels(id) => format!("buffer_channels @buffer{}", id.raw()),
        Rvalue::BufferSampleRate(id) => format!("buffer_sample_rate @buffer{}", id.raw()),
        Rvalue::BufferParamLen(id) => format!("buffer_len @param{}", id.raw()),
        Rvalue::BufferParamChannels(id) => format!("buffer_channels @param{}", id.raw()),
        Rvalue::BufferParamSampleRate(id) => {
            format!("buffer_sample_rate @param{}", id.raw())
        }
        Rvalue::ConstDataLoad {
            data,
            index,
            bounds,
        } => format!(
            "load_const_data @data{}[{}] {}",
            data.raw(),
            format_value(*index),
            format_bounds(*bounds)
        ),
        Rvalue::MakeSlice {
            source,
            start,
            len,
            bounds,
            access,
        } => format!(
            "make_slice {} start={} len={} bounds={} access={}",
            format_slice_source(source),
            format_value(*start),
            format_value(*len),
            format_bounds(*bounds),
            format_access(*access)
        ),
        Rvalue::SliceLoad {
            slice,
            index,
            bounds,
        } => format!(
            "load_slice {}[{}] {}",
            format_value(*slice),
            format_value(*index),
            format_bounds(*bounds)
        ),
        Rvalue::SliceLen(value) => format!("slice_len {}", format_value(*value)),
    }
}

fn format_slice_source(source: &SliceSource) -> String {
    match source {
        SliceSource::Place(place) => format_place(place),
        SliceSource::Buffer { buffer, channel } => format!(
            "@buffer{}{}",
            buffer.raw(),
            channel
                .map(|value| format!("[{}]", format_value(value)))
                .unwrap_or_default()
        ),
        SliceSource::BufferParam { parameter, channel } => format!(
            "@param{}{}",
            parameter.raw(),
            channel
                .map(|value| format!("[{}]", format_value(value)))
                .unwrap_or_default()
        ),
        SliceSource::ConstData(id) => format!("@data{}", id.raw()),
    }
}

fn format_call_argument(argument: &CallArgument) -> String {
    match argument {
        CallArgument::Value(value) => format_value(*value),
        CallArgument::Place(place) => format!("place {}", format_place(place)),
        CallArgument::SliceElement {
            slice,
            index,
            bounds,
        } => format!(
            "slice_element {}[{}] {}",
            format_value(*slice),
            format_value(*index),
            format_bounds(*bounds)
        ),
        CallArgument::ArrayWindow {
            array,
            start,
            bounds,
        } => format!(
            "array_window {}[{}..] {}",
            format_place(array),
            format_value(*start),
            format_bounds(*bounds)
        ),
        CallArgument::SliceWindow {
            slice,
            start,
            bounds,
        } => format!(
            "slice_window {}[{}..] {}",
            format_value(*slice),
            format_value(*start),
            format_bounds(*bounds)
        ),
        CallArgument::Buffer(id) => format!("@buffer{}", id.raw()),
    }
}

fn format_optional_index(value: Option<&Value>, bounds: BoundsMode) -> String {
    value
        .map(|value| format!("[{}] {}", format_value(*value), format_bounds(bounds)))
        .unwrap_or_default()
}

fn format_access(access: AccessMode) -> &'static str {
    match access {
        AccessMode::ReadOnly => "readonly",
        AccessMode::ReadWrite => "readwrite",
    }
}

fn format_channels(channels: BufferChannels) -> String {
    match channels {
        BufferChannels::Mono => "mono".to_owned(),
        BufferChannels::Static(count) => count.to_string(),
        BufferChannels::Dynamic => "dynamic".to_owned(),
    }
}

fn format_bounds(bounds: BoundsMode) -> &'static str {
    match bounds {
        BoundsMode::Clamp => "clamp",
        BoundsMode::Trap => "trap",
        BoundsMode::Unchecked => "unchecked",
    }
}

fn format_passing_mode(mode: PassingMode) -> &'static str {
    match mode {
        PassingMode::Value => "value",
        PassingMode::ReadOnlyReference => "readonly_ref",
        PassingMode::ReadWriteReference => "readwrite_ref",
    }
}

fn format_function_kind(kind: FunctionKind) -> String {
    match kind {
        FunctionKind::Init => "init".to_owned(),
        FunctionKind::Process => "process".to_owned(),
        FunctionKind::Event(id) => format!("event(@event{})", id.raw()),
        FunctionKind::User => "user".to_owned(),
    }
}

fn format_function_origin(origin: FunctionOrigin) -> &'static str {
    match origin {
        FunctionOrigin::Source => "source",
        FunctionOrigin::CompilerGenerated => "compiler_generated",
    }
}

fn format_inline_hint(hint: InlineHint) -> &'static str {
    match hint {
        InlineHint::Auto => "auto",
        InlineHint::Always => "always",
        InlineHint::Never => "never",
    }
}

fn format_unary(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Negate => "neg",
        UnaryOp::LogicalNot => "not",
        UnaryOp::BitNot => "bit_not",
    }
}

fn format_binary(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Subtract => "sub",
        BinaryOp::Multiply => "mul",
        BinaryOp::Divide => "div",
        BinaryOp::Remainder => "rem",
        BinaryOp::BitAnd => "bit_and",
        BinaryOp::BitOr => "bit_or",
        BinaryOp::BitXor => "bit_xor",
        BinaryOp::ShiftLeft => "shl",
        BinaryOp::ShiftRight => "shr",
    }
}

fn format_compare(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Equal => "eq",
        CompareOp::NotEqual => "ne",
        CompareOp::Less => "lt",
        CompareOp::LessEqual => "le",
        CompareOp::Greater => "gt",
        CompareOp::GreaterEqual => "ge",
    }
}

fn format_intrinsic(intrinsic: Intrinsic) -> &'static str {
    match intrinsic {
        Intrinsic::Sin => "sin",
        Intrinsic::Cos => "cos",
        Intrinsic::Tan => "tan",
        Intrinsic::Tanh => "tanh",
        Intrinsic::Atan => "atan",
        Intrinsic::Atan2 => "atan2",
        Intrinsic::Exp => "exp",
        Intrinsic::Log => "log",
        Intrinsic::Sqrt => "sqrt",
        Intrinsic::Pow => "pow",
        Intrinsic::Abs => "abs",
        Intrinsic::Floor => "floor",
        Intrinsic::Ceil => "ceil",
        Intrinsic::Round => "round",
        Intrinsic::Trunc => "trunc",
        Intrinsic::Min => "min",
        Intrinsic::Max => "max",
        Intrinsic::Fma => "fma",
    }
}

fn format_source(source: SourceSpan) -> String {
    if source.is_unknown() {
        return String::new();
    }
    let file = source
        .file
        .map(|id| format!("@file{}:", id.raw()))
        .unwrap_or_default();
    format!(
        " @ {file}{}:{}..{}:{}",
        source.line, source.column, source.end_line, source.end_column
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_function(name: &str, kind: FunctionKind) -> Function {
        Function {
            name: name.to_owned(),
            kind,
            attributes: FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        }
    }

    #[test]
    fn formats_program_deterministically() {
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.push(Type::Scalar(ScalarType::F32));
        program.types.push(Type::Scalar(ScalarType::I32));
        program.source_files.push(SourceFile {
            path: "quoted\"path.onda".to_owned(),
        });
        program
            .functions
            .push(empty_function("init", FunctionKind::Init));
        let mut process = empty_function("process", FunctionKind::Process);
        process.params = process_function_params(TypeId::new(1));
        program.functions.push(process);

        let expected = concat!(
            "mir 5\n",
            "config sample_rate=48000.0 block_size=64\n",
            "source @file0 \"quoted\\\"path.onda\"\n",
            "type @t0 = f32\n",
            "type @t1 = i32\n",
            "entry init=@fn0 process=@fn1\n",
            "fn @fn0 \"init\" init origin=source inline=auto () -> () {\n",
            "}\n",
            "fn @fn1 \"process\" process origin=source inline=auto (@p0 \"start_frame\": @t1 value, @p1 \"frames\": @t1 value, @p2 \"flags\": @t1 value) -> () {\n",
            "}\n",
        );
        assert_eq!(format_program(&program), expected);
        assert_eq!(format_program(&program), format_program(&program));
    }

    #[test]
    fn formats_structured_statements() {
        let mut function = empty_function("choose", FunctionKind::User);
        function.params.push(FunctionParam {
            name: "condition".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::Value,
        });
        function.results.push(TypeId::new(1));
        function.locals.push(Local {
            name: Some("result".to_owned()),
            ty: TypeId::new(1),
        });
        function.body.statements.push(Statement {
            kind: StatementKind::If {
                condition: Value::Constant(ScalarValue::Bool(true)),
                then_block: Block {
                    statements: vec![Statement {
                        kind: StatementKind::Assign {
                            destination: Place::local(LocalId::new(0)),
                            value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
                        },
                        source: SourceSpan::UNKNOWN,
                    }],
                },
                else_block: Block::default(),
            },
            source: SourceSpan::UNKNOWN,
        });
        function.body.statements.push(Statement {
            kind: StatementKind::Return {
                values: vec![Value::Local(LocalId::new(0))],
            },
            source: SourceSpan::UNKNOWN,
        });

        let mut program = Program::new(
            CompileConfig {
                sample_rate: 44_100.0,
                block_size: 128,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.extend([
            Type::Scalar(ScalarType::Bool),
            Type::Scalar(ScalarType::F32),
        ]);
        program.functions.extend([
            empty_function("init", FunctionKind::Init),
            empty_function("process", FunctionKind::Process),
            function,
        ]);

        let dump = format_program(&program);
        assert!(dump.contains(
            "fn @fn2 \"choose\" user origin=source inline=auto (@p0 \"condition\": @t0 value) -> (@t1)"
        ));
        assert!(dump.contains("  if bool(true) {\n    %0 = f32(1.0)\n  } else {\n  }"));
        assert!(dump.contains("  return (%0)"));
    }
}
