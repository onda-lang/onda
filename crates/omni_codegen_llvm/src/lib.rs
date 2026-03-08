use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use omni_frontend::{AssignTarget, CallArg, Diagnostic, Expr, PrimitiveType, Stmt};
use omni_semantics::{
    TypedArrayInfo, TypedBufferChannels, TypedBufferDecl, TypedConstValue, TypedEventParamType,
    TypedFnParam, TypedProgram, TypedValueRange,
};

#[cfg(feature = "llvm-orc")]
mod orc_backend;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExecutionBackend {
    Auto,
    OrcJit,
}

#[derive(Debug, Clone, Copy)]
pub struct CompileOptions {
    pub backend: ExecutionBackend,
    pub sample_rate: f32,
    pub block_size: usize,
    pub fast_math: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: 512,
            fast_math: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JitProgram {
    pub typed: Arc<TypedProgram>,
    sample_rate: f32,
    block_size: usize,
    inputs: Arc<Vec<DeclaredIo>>,
    outputs: Arc<Vec<DeclaredIo>>,
    params: Arc<Vec<DeclaredIo>>,
    events: Arc<Vec<DeclaredEvent>>,
    buffers: Arc<Vec<DeclaredBuffer>>,
    input_index: Arc<HashMap<String, usize>>,
    output_index: Arc<HashMap<String, usize>>,
    param_index: Arc<HashMap<String, usize>>,
    event_index: Arc<HashMap<String, usize>>,
    buffer_index: Arc<HashMap<String, usize>>,
    #[cfg(feature = "llvm-orc")]
    compiled: Arc<orc_backend::OrcProcess>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeState {
    pub(crate) state_words: Vec<u64>,
    pub(crate) state_size_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct DeclaredIo {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    slot_offset: usize,
    byte_offset: usize,
    default: Option<TypedConstValue>,
    range: Option<TypedValueRange>,
}

impl DeclaredIo {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn elem_ty(&self) -> PrimitiveType {
        self.elem_ty
    }

    pub fn array_len(&self) -> usize {
        self.array_len
    }

    pub fn slot_offset(&self) -> usize {
        self.slot_offset
    }

    pub fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    pub fn default(&self) -> Option<TypedConstValue> {
        self.default
    }

    pub fn default_as_f64(&self) -> Option<f64> {
        self.default.map(typed_const_to_f64)
    }

    pub fn has_range(&self) -> bool {
        self.range.is_some()
    }

    pub fn range(&self) -> Option<TypedValueRange> {
        self.range
    }

    pub fn range_min_as_f64(&self) -> Option<f64> {
        self.range.map(|r| typed_const_to_f64(r.min))
    }

    pub fn range_max_as_f64(&self) -> Option<f64> {
        self.range.map(|r| typed_const_to_f64(r.max))
    }

    pub fn type_repr(&self) -> String {
        if self.array_len == 1 {
            primitive_type_name(self.elem_ty).to_owned()
        } else {
            format!("{}[{}]", primitive_type_name(self.elem_ty), self.array_len)
        }
    }

    pub fn byte_size(&self) -> usize {
        primitive_type_bytes(self.elem_ty).saturating_mul(self.array_len)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DeclaredBufferChannels {
    Mono,
    Static(usize),
    Dynamic,
}

#[derive(Debug, Clone)]
pub struct DeclaredBuffer {
    name: String,
    elem_ty: PrimitiveType,
    channels: DeclaredBufferChannels,
    may_write: bool,
}

impl DeclaredBuffer {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn elem_ty(&self) -> PrimitiveType {
        self.elem_ty
    }

    pub fn channels(&self) -> DeclaredBufferChannels {
        self.channels
    }

    pub fn may_write(&self) -> bool {
        self.may_write
    }

    pub fn type_repr(&self) -> String {
        let elem = primitive_type_name(self.elem_ty);
        match self.channels {
            DeclaredBufferChannels::Mono => format!("buffer[{elem}]"),
            DeclaredBufferChannels::Static(ch) => format!("buffer[{elem}[{ch}]]"),
            DeclaredBufferChannels::Dynamic => format!("buffer[{elem}[]]"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeclaredEvent {
    name: String,
    params: Vec<DeclaredEventParam>,
    payload_bytes: Option<usize>,
}

impl DeclaredEvent {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn params(&self) -> &[DeclaredEventParam] {
        &self.params
    }

    pub fn payload_bytes(&self) -> Option<usize> {
        self.payload_bytes
    }
}

#[derive(Debug, Clone)]
pub struct DeclaredEventParam {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    is_slice: bool,
    byte_offset: usize,
}

impl DeclaredEventParam {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn elem_ty(&self) -> PrimitiveType {
        self.elem_ty
    }

    pub fn array_len(&self) -> usize {
        self.array_len
    }

    pub fn is_slice(&self) -> bool {
        self.is_slice
    }

    pub fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    pub fn type_repr(&self) -> String {
        if self.is_slice {
            return format!("{}[]", primitive_type_name(self.elem_ty));
        }
        if self.array_len == 1 {
            primitive_type_name(self.elem_ty).to_owned()
        } else {
            format!("{}[{}]", primitive_type_name(self.elem_ty), self.array_len)
        }
    }

    pub fn byte_size(&self) -> Option<usize> {
        if self.is_slice {
            return None;
        }
        Some(primitive_type_bytes(self.elem_ty).saturating_mul(self.array_len))
    }
}

fn primitive_type_name(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

fn primitive_type_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn append_typed_const_bytes(out: &mut Vec<u8>, value: TypedConstValue, ty: PrimitiveType) {
    match (ty, value) {
        (PrimitiveType::F32, TypedConstValue::F32(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::F64, TypedConstValue::F64(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::I32, TypedConstValue::I32(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::I64, TypedConstValue::I64(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::Bool, TypedConstValue::Bool(v)) => out.push(if v { 1 } else { 0 }),
        (PrimitiveType::F32, other) => {
            let v = typed_const_to_f64(other) as f32;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::F64, other) => {
            let v = typed_const_to_f64(other);
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::I32, other) => {
            let v = typed_const_to_f64(other) as i32;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::I64, other) => {
            let v = typed_const_to_f64(other) as i64;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::Bool, other) => {
            out.push(if typed_const_to_f64(other) != 0.0 {
                1
            } else {
                0
            });
        }
    }
}

fn typed_const_to_f64(value: TypedConstValue) -> f64 {
    match value {
        TypedConstValue::F32(v) => v as f64,
        TypedConstValue::F64(v) => v,
        TypedConstValue::I32(v) => v as f64,
        TypedConstValue::I64(v) => v as f64,
        TypedConstValue::Bool(v) => {
            if v {
                1.0
            } else {
                0.0
            }
        }
    }
}

fn build_declared_port_ios(
    flat: &[String],
    types: &HashMap<String, PrimitiveType>,
    arrays: &HashMap<String, TypedArrayInfo>,
    defaults: &HashMap<String, TypedConstValue>,
    ranges: &HashMap<String, TypedValueRange>,
) -> Vec<DeclaredIo> {
    let arrays_by_offset = arrays
        .iter()
        .map(|(name, info)| (info.offset, (name, info)))
        .collect::<HashMap<_, _>>();
    let mut out = Vec::new();
    let mut slot = 0usize;
    let mut byte_offset = 0usize;
    while slot < flat.len() {
        if let Some((name, info)) = arrays_by_offset.get(&slot) {
            out.push(DeclaredIo {
                name: (*name).clone(),
                elem_ty: info.elem_ty,
                array_len: info.len,
                slot_offset: slot,
                byte_offset,
                default: None,
                range: None,
            });
            byte_offset = byte_offset
                .saturating_add(primitive_type_bytes(info.elem_ty).saturating_mul(info.len));
            slot += info.len;
            continue;
        }
        let name = flat[slot].clone();
        let ty = *types.get(&name).unwrap_or(&PrimitiveType::F32);
        let default = defaults.get(&name).copied();
        let range = ranges.get(&name).copied();
        out.push(DeclaredIo {
            name,
            elem_ty: ty,
            array_len: 1,
            slot_offset: slot,
            byte_offset,
            default,
            range,
        });
        byte_offset = byte_offset.saturating_add(primitive_type_bytes(ty));
        slot += 1;
    }
    out
}

fn build_declared_param_ios(typed: &TypedProgram) -> Vec<DeclaredIo> {
    let arrays_by_offset = typed
        .param_arrays
        .iter()
        .map(|(name, info)| (info.offset, (name, info)))
        .collect::<HashMap<_, _>>();
    let mut out = Vec::new();
    let mut slot = 0usize;
    let mut byte_offset = 0usize;
    while slot < typed.params.len() {
        if let Some((name, info)) = arrays_by_offset.get(&slot) {
            out.push(DeclaredIo {
                name: (*name).clone(),
                elem_ty: info.elem_ty,
                array_len: info.len,
                slot_offset: slot,
                byte_offset,
                default: None,
                range: None,
            });
            byte_offset = byte_offset
                .saturating_add(primitive_type_bytes(info.elem_ty).saturating_mul(info.len));
            slot += info.len;
            continue;
        }
        let param = &typed.params[slot];
        out.push(DeclaredIo {
            name: param.name.clone(),
            elem_ty: param.ty,
            array_len: 1,
            slot_offset: slot,
            byte_offset,
            default: Some(param.default),
            range: param.range,
        });
        byte_offset = byte_offset.saturating_add(primitive_type_bytes(param.ty));
        slot += 1;
    }
    out
}

fn build_name_to_index(entries: &[DeclaredIo]) -> HashMap<String, usize> {
    entries
        .iter()
        .enumerate()
        .map(|(idx, e)| (e.name.clone(), idx))
        .collect()
}

fn build_event_name_to_index(entries: &[DeclaredEvent]) -> HashMap<String, usize> {
    entries
        .iter()
        .enumerate()
        .map(|(idx, e)| (e.name.clone(), idx))
        .collect()
}

fn build_declared_buffers(typed: &TypedProgram) -> Vec<DeclaredBuffer> {
    let written_top_level_buffers = infer_written_top_level_buffers(typed);
    typed
        .buffers
        .iter()
        .map(|b: &TypedBufferDecl| DeclaredBuffer {
            name: b.name.clone(),
            elem_ty: b.elem_ty,
            channels: match b.channels {
                TypedBufferChannels::Mono => DeclaredBufferChannels::Mono,
                TypedBufferChannels::Static(ch) => DeclaredBufferChannels::Static(ch),
                TypedBufferChannels::Dynamic => DeclaredBufferChannels::Dynamic,
            },
            may_write: written_top_level_buffers.contains(&b.name),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefBufferWriteSummary {
    buffer_param_writes: Vec<bool>,
    global_buffer_writes: HashSet<String>,
}

fn infer_written_top_level_buffers(typed: &TypedProgram) -> HashSet<String> {
    let top_level_buffers = typed
        .buffers
        .iter()
        .map(|b| b.name.clone())
        .collect::<HashSet<_>>();
    if top_level_buffers.is_empty() {
        return HashSet::new();
    }

    let def_index_by_name = typed
        .defs
        .iter()
        .enumerate()
        .map(|(idx, def)| (def.name.as_str(), idx))
        .collect::<HashMap<_, _>>();

    let def_buffer_param_positions = typed
        .defs
        .iter()
        .map(|def| {
            def.param_kinds
                .iter()
                .enumerate()
                .filter_map(|(param_idx, param_kind)| match param_kind {
                    TypedFnParam::Buffer { .. } => Some(param_idx),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let def_buffer_param_slot_by_name = typed
        .defs
        .iter()
        .zip(def_buffer_param_positions.iter())
        .map(|(def, positions)| {
            positions
                .iter()
                .enumerate()
                .filter_map(|(slot_idx, param_idx)| {
                    def.params
                        .get(*param_idx)
                        .cloned()
                        .map(|name| (name, slot_idx))
                })
                .collect::<HashMap<_, _>>()
        })
        .collect::<Vec<_>>();

    let mut summaries = def_buffer_param_positions
        .iter()
        .map(|positions| DefBufferWriteSummary {
            buffer_param_writes: vec![false; positions.len()],
            global_buffer_writes: HashSet::new(),
        })
        .collect::<Vec<_>>();

    let mut changed = true;
    while changed {
        changed = false;
        for (def_idx, def) in typed.defs.iter().enumerate() {
            let mut next = DefBufferWriteSummary {
                buffer_param_writes: vec![false; def_buffer_param_positions[def_idx].len()],
                global_buffer_writes: HashSet::new(),
            };
            for stmt in &def.body {
                collect_stmt_buffer_write_usage(
                    stmt,
                    &top_level_buffers,
                    &def_index_by_name,
                    typed,
                    &def_buffer_param_positions,
                    &def_buffer_param_slot_by_name[def_idx],
                    &summaries,
                    &mut next.buffer_param_writes,
                    &mut next.global_buffer_writes,
                );
            }
            if summaries[def_idx] != next {
                summaries[def_idx] = next;
                changed = true;
            }
        }
    }

    let mut written = HashSet::<String>::new();
    let no_param_slots = HashMap::<String, usize>::new();
    for stmt in &typed.init {
        collect_stmt_buffer_write_usage(
            stmt,
            &top_level_buffers,
            &def_index_by_name,
            typed,
            &def_buffer_param_positions,
            &no_param_slots,
            &summaries,
            &mut [],
            &mut written,
        );
    }
    for stmt in &typed.block_pre {
        collect_stmt_buffer_write_usage(
            stmt,
            &top_level_buffers,
            &def_index_by_name,
            typed,
            &def_buffer_param_positions,
            &no_param_slots,
            &summaries,
            &mut [],
            &mut written,
        );
    }
    for stmt in &typed.sample {
        collect_stmt_buffer_write_usage(
            stmt,
            &top_level_buffers,
            &def_index_by_name,
            typed,
            &def_buffer_param_positions,
            &no_param_slots,
            &summaries,
            &mut [],
            &mut written,
        );
    }
    for stmt in &typed.block_post {
        collect_stmt_buffer_write_usage(
            stmt,
            &top_level_buffers,
            &def_index_by_name,
            typed,
            &def_buffer_param_positions,
            &no_param_slots,
            &summaries,
            &mut [],
            &mut written,
        );
    }
    for event in &typed.events {
        for stmt in &event.body {
            collect_stmt_buffer_write_usage(
                stmt,
                &top_level_buffers,
                &def_index_by_name,
                typed,
                &def_buffer_param_positions,
                &no_param_slots,
                &summaries,
                &mut [],
                &mut written,
            );
        }
    }
    written
}

fn collect_stmt_buffer_write_usage(
    stmt: &Stmt,
    top_level_buffers: &HashSet<String>,
    def_index_by_name: &HashMap<&str, usize>,
    typed: &TypedProgram,
    def_buffer_param_positions: &[Vec<usize>],
    param_slot_by_name: &HashMap<String, usize>,
    summaries: &[DefBufferWriteSummary],
    param_writes: &mut [bool],
    global_writes: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { base, index } = target {
                mark_buffer_symbol_write(
                    base,
                    top_level_buffers,
                    param_slot_by_name,
                    param_writes,
                    global_writes,
                );
                collect_expr_buffer_write_usage(
                    index,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
            collect_expr_buffer_write_usage(
                expr,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_expr_buffer_write_usage(
                expr,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr_buffer_write_usage(
                cond,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
            for inner in then_branch {
                collect_stmt_buffer_write_usage(
                    inner,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
            for inner in else_branch {
                collect_stmt_buffer_write_usage(
                    inner,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                collect_expr_buffer_write_usage(
                    step,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
            collect_expr_buffer_write_usage(
                start,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
            collect_expr_buffer_write_usage(
                end,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
            for inner in body {
                collect_stmt_buffer_write_usage(
                    inner,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_expr_buffer_write_usage(
                cond,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
            for inner in body {
                collect_stmt_buffer_write_usage(
                    inner,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn collect_expr_buffer_write_usage(
    expr: &Expr,
    top_level_buffers: &HashSet<String>,
    def_index_by_name: &HashMap<&str, usize>,
    typed: &TypedProgram,
    def_buffer_param_positions: &[Vec<usize>],
    param_slot_by_name: &HashMap<String, usize>,
    summaries: &[DefBufferWriteSummary],
    param_writes: &mut [bool],
    global_writes: &mut HashSet<String>,
) {
    match expr {
        Expr::ArrayLiteral(items) => {
            for item in items {
                collect_expr_buffer_write_usage(
                    item,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
        }
        Expr::Index { index, .. } => {
            collect_expr_buffer_write_usage(
                index,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_expr_buffer_write_usage(
                    start,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
            if let Some(end) = end {
                collect_expr_buffer_write_usage(
                    end,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
        }
        Expr::ArrayCtor { spec, init } => {
            collect_expr_buffer_write_usage(
                &spec.size,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
            if let Some(init) = init {
                for item in init {
                    collect_expr_buffer_write_usage(
                        item,
                        top_level_buffers,
                        def_index_by_name,
                        typed,
                        def_buffer_param_positions,
                        param_slot_by_name,
                        summaries,
                        param_writes,
                        global_writes,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_expr_buffer_write_usage(
                lhs,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
            collect_expr_buffer_write_usage(
                rhs,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_buffer_write_usage(
                    arg,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            apply_user_call_buffer_write_usage(
                name,
                args,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
            for arg in args {
                collect_expr_buffer_write_usage(
                    &arg.expr,
                    top_level_buffers,
                    def_index_by_name,
                    typed,
                    def_buffer_param_positions,
                    param_slot_by_name,
                    summaries,
                    param_writes,
                    global_writes,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } | Expr::UnaryBitNot { expr } => {
            collect_expr_buffer_write_usage(
                expr,
                top_level_buffers,
                def_index_by_name,
                typed,
                def_buffer_param_positions,
                param_slot_by_name,
                summaries,
                param_writes,
                global_writes,
            );
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn apply_user_call_buffer_write_usage(
    name: &str,
    args: &[CallArg],
    top_level_buffers: &HashSet<String>,
    def_index_by_name: &HashMap<&str, usize>,
    typed: &TypedProgram,
    def_buffer_param_positions: &[Vec<usize>],
    param_slot_by_name: &HashMap<String, usize>,
    summaries: &[DefBufferWriteSummary],
    param_writes: &mut [bool],
    global_writes: &mut HashSet<String>,
) {
    if name == "unsafe_write" || name == "__omni_buffer_write2" {
        if let Some(first_arg) = args.first() {
            if let Expr::Var(base) = &first_arg.expr {
                mark_buffer_symbol_write(
                    base,
                    top_level_buffers,
                    param_slot_by_name,
                    param_writes,
                    global_writes,
                );
            }
        }
    } else if let Some(base) = parse_unsafe_write_instance_base_for_metadata(name) {
        mark_buffer_symbol_write(
            base,
            top_level_buffers,
            param_slot_by_name,
            param_writes,
            global_writes,
        );
    }

    let Some(&callee_idx) = def_index_by_name.get(name) else {
        return;
    };
    let callee = &typed.defs[callee_idx];
    let callee_summary = &summaries[callee_idx];
    let bound_args = bind_call_args_to_params(&callee.params, args);

    for (slot_idx, param_idx) in def_buffer_param_positions[callee_idx].iter().enumerate() {
        if !callee_summary
            .buffer_param_writes
            .get(slot_idx)
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        let Some(Some(arg_expr)) = bound_args.get(*param_idx) else {
            continue;
        };
        if let Expr::Var(base) = arg_expr {
            mark_buffer_symbol_write(
                base,
                top_level_buffers,
                param_slot_by_name,
                param_writes,
                global_writes,
            );
        }
    }

    global_writes.extend(callee_summary.global_buffer_writes.iter().cloned());
}

fn bind_call_args_to_params<'a>(params: &[String], args: &'a [CallArg]) -> Vec<Option<&'a Expr>> {
    let mut bound = vec![None; params.len()];
    let mut next_positional = 0usize;
    for arg in args {
        if let Some(name) = &arg.name {
            if let Some(param_idx) = params.iter().position(|p| p == name) {
                bound[param_idx] = Some(&arg.expr);
            }
            continue;
        }
        while next_positional < bound.len() && bound[next_positional].is_some() {
            next_positional = next_positional.saturating_add(1);
        }
        if next_positional >= bound.len() {
            continue;
        }
        bound[next_positional] = Some(&arg.expr);
        next_positional = next_positional.saturating_add(1);
    }
    bound
}

fn mark_buffer_symbol_write(
    base: &str,
    top_level_buffers: &HashSet<String>,
    param_slot_by_name: &HashMap<String, usize>,
    param_writes: &mut [bool],
    global_writes: &mut HashSet<String>,
) {
    if top_level_buffers.contains(base) {
        global_writes.insert(base.to_owned());
    }
    if let Some(slot_idx) = param_slot_by_name.get(base) {
        if let Some(slot) = param_writes.get_mut(*slot_idx) {
            *slot = true;
        }
    }
}

fn parse_unsafe_write_instance_base_for_metadata(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "unsafe_write" {
        return None;
    }
    Some(base)
}

fn build_declared_events(typed: &TypedProgram) -> Vec<DeclaredEvent> {
    typed
        .events
        .iter()
        .map(|event| {
            let mut params = Vec::<DeclaredEventParam>::new();
            let mut byte_offset = 0usize;
            let mut payload_bytes = Some(0usize);
            for param in &event.params {
                match param.ty {
                    TypedEventParamType::Scalar(elem_ty) => {
                        params.push(DeclaredEventParam {
                            name: param.name.clone(),
                            elem_ty,
                            array_len: 1,
                            is_slice: false,
                            byte_offset,
                        });
                        byte_offset = byte_offset.saturating_add(primitive_type_bytes(elem_ty));
                        if let Some(total) = payload_bytes.as_mut() {
                            *total = total.saturating_add(primitive_type_bytes(elem_ty));
                        }
                    }
                    TypedEventParamType::Array { elem, len } => {
                        params.push(DeclaredEventParam {
                            name: param.name.clone(),
                            elem_ty: elem,
                            array_len: len,
                            is_slice: false,
                            byte_offset,
                        });
                        let bytes = primitive_type_bytes(elem).saturating_mul(len);
                        byte_offset = byte_offset.saturating_add(bytes);
                        if let Some(total) = payload_bytes.as_mut() {
                            *total = total.saturating_add(bytes);
                        }
                    }
                    TypedEventParamType::Slice { elem } => {
                        params.push(DeclaredEventParam {
                            name: param.name.clone(),
                            elem_ty: elem,
                            array_len: 0,
                            is_slice: true,
                            byte_offset,
                        });
                        payload_bytes = None;
                    }
                }
            }
            DeclaredEvent {
                name: event.name.clone(),
                params,
                payload_bytes,
            }
        })
        .collect::<Vec<_>>()
}

fn validate_event_payload<'a>(desc: &'a DeclaredEvent, payload: &[u8]) -> Result<(), Diagnostic> {
    if let Some(expected) = desc.payload_bytes() {
        if payload.len() != expected {
            return Err(Diagnostic::runtime(
                format!(
                    "event '{}' expects {} payload bytes, got {}",
                    desc.name(),
                    expected,
                    payload.len()
                ),
                0,
                0,
            ));
        }
        return Ok(());
    }

    let mut offset = 0usize;
    for param in desc.params() {
        if param.is_slice() {
            if payload.len().saturating_sub(offset) < std::mem::size_of::<i32>() {
                return Err(Diagnostic::runtime(
                    format!(
                        "event '{}' payload is truncated before slice parameter '{}'",
                        desc.name(),
                        param.name()
                    ),
                    0,
                    0,
                ));
            }
            let len_bytes: [u8; 4] = payload[offset..offset + 4]
                .try_into()
                .expect("slice len bytes");
            let len = i32::from_ne_bytes(len_bytes);
            if len < 0 {
                return Err(Diagnostic::runtime(
                    format!(
                        "event '{}' slice parameter '{}' has negative length {}",
                        desc.name(),
                        param.name(),
                        len
                    ),
                    0,
                    0,
                ));
            }
            let len = len as usize;
            let data_bytes = primitive_type_bytes(param.elem_ty()).saturating_mul(len);
            offset = offset.saturating_add(4);
            if payload.len().saturating_sub(offset) < data_bytes {
                return Err(Diagnostic::runtime(
                    format!(
                        "event '{}' payload is truncated in slice parameter '{}'; expected {} element bytes after length prefix",
                        desc.name(),
                        param.name(),
                        data_bytes
                    ),
                    0,
                    0,
                ));
            }
            offset = offset.saturating_add(data_bytes);
        } else {
            let bytes = param.byte_size().unwrap_or(0);
            if payload.len().saturating_sub(offset) < bytes {
                return Err(Diagnostic::runtime(
                    format!(
                        "event '{}' payload is truncated in parameter '{}'; expected {} bytes",
                        desc.name(),
                        param.name(),
                        bytes
                    ),
                    0,
                    0,
                ));
            }
            offset = offset.saturating_add(bytes);
        }
    }

    if offset != payload.len() {
        return Err(Diagnostic::runtime(
            format!(
                "event '{}' expects {} payload bytes for its dynamic layout, got {}",
                desc.name(),
                offset,
                payload.len()
            ),
            0,
            0,
        ));
    }

    Ok(())
}

pub fn lower_and_jit(typed: TypedProgram) -> Result<JitProgram, Vec<Diagnostic>> {
    lower_and_jit_with_options(typed, CompileOptions::default())
}

pub fn lower_and_jit_with_options(
    typed: TypedProgram,
    options: CompileOptions,
) -> Result<JitProgram, Vec<Diagnostic>> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(vec![Diagnostic::internal(
            "compile option 'sample_rate' must be finite and greater than zero",
        )]);
    }
    if options.block_size == 0 {
        return Err(vec![Diagnostic::internal(
            "compile option 'block_size' must be greater than zero",
        )]);
    }
    match options.backend {
        ExecutionBackend::Auto | ExecutionBackend::OrcJit => build_orc_program(
            typed,
            options.sample_rate,
            options.block_size,
            options.fast_math,
        ),
    }
}

pub fn lower_to_llvm_ir_with_options(
    typed: TypedProgram,
    options: CompileOptions,
) -> Result<String, Vec<Diagnostic>> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(vec![Diagnostic::internal(
            "compile option 'sample_rate' must be finite and greater than zero",
        )]);
    }
    if options.block_size == 0 {
        return Err(vec![Diagnostic::internal(
            "compile option 'block_size' must be greater than zero",
        )]);
    }
    match options.backend {
        ExecutionBackend::Auto | ExecutionBackend::OrcJit => emit_orc_ir(
            typed,
            options.sample_rate,
            options.block_size,
            options.fast_math,
        ),
    }
}

#[cfg(feature = "llvm-orc")]
fn build_orc_program(
    typed: TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<JitProgram, Vec<Diagnostic>> {
    let empty_defaults = HashMap::<String, TypedConstValue>::new();
    let empty_ranges = HashMap::<String, TypedValueRange>::new();
    let inputs = build_declared_port_ios(
        &typed.ins,
        &typed.in_types,
        &typed.in_arrays,
        &typed.in_defaults,
        &typed.in_ranges,
    );
    let outputs = build_declared_port_ios(
        &typed.outs,
        &typed.out_types,
        &typed.out_arrays,
        &empty_defaults,
        &empty_ranges,
    );
    let params = build_declared_param_ios(&typed);
    let events = build_declared_events(&typed);
    let buffers = build_declared_buffers(&typed);
    let compiled = orc_backend::compile_orc(&typed, sample_rate, block_size, fast_math)
        .map_err(|d| vec![d])?;
    Ok(JitProgram {
        typed: Arc::new(typed),
        sample_rate,
        block_size,
        input_index: Arc::new(build_name_to_index(&inputs)),
        output_index: Arc::new(build_name_to_index(&outputs)),
        param_index: Arc::new(build_name_to_index(&params)),
        event_index: Arc::new(build_event_name_to_index(&events)),
        buffer_index: Arc::new(
            buffers
                .iter()
                .enumerate()
                .map(|(idx, b)| (b.name.clone(), idx))
                .collect(),
        ),
        inputs: Arc::new(inputs),
        outputs: Arc::new(outputs),
        params: Arc::new(params),
        events: Arc::new(events),
        buffers: Arc::new(buffers),
        compiled: Arc::new(compiled),
    })
}

#[cfg(feature = "llvm-orc")]
fn emit_orc_ir(
    typed: TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<String, Vec<Diagnostic>> {
    orc_backend::emit_optimized_ir(&typed, sample_rate, block_size, fast_math).map_err(|d| vec![d])
}

#[cfg(not(feature = "llvm-orc"))]
fn emit_orc_ir(
    _typed: TypedProgram,
    _sample_rate: f32,
    _block_size: usize,
    _fast_math: bool,
) -> Result<String, Vec<Diagnostic>> {
    Err(vec![Diagnostic::internal(
        "ORC backend is required but omni_codegen_llvm was built without 'llvm-orc' feature",
    )])
}

#[cfg(not(feature = "llvm-orc"))]
fn build_orc_program(
    _typed: TypedProgram,
    _sample_rate: f32,
    _block_size: usize,
    _fast_math: bool,
) -> Result<JitProgram, Vec<Diagnostic>> {
    Err(vec![Diagnostic::internal(
        "ORC backend is required but omni_codegen_llvm was built without 'llvm-orc' feature",
    )])
}

impl JitProgram {
    pub fn required_in_channels(&self) -> usize {
        self.typed.ins.len()
    }

    pub fn required_out_channels(&self) -> usize {
        self.typed.outs.len()
    }

    pub fn default_param_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for param in &self.typed.params {
            append_typed_const_bytes(&mut out, param.default, param.ty);
        }
        out
    }

    pub fn inputs(&self) -> &[DeclaredIo] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[DeclaredIo] {
        &self.outputs
    }

    pub fn params(&self) -> &[DeclaredIo] {
        &self.params
    }

    pub fn buffers(&self) -> &[DeclaredBuffer] {
        &self.buffers
    }

    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }

    pub fn param_count(&self) -> usize {
        self.params.len()
    }

    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    pub fn input_name(&self, index: usize) -> Option<&str> {
        self.inputs.get(index).map(DeclaredIo::name)
    }

    pub fn output_name(&self, index: usize) -> Option<&str> {
        self.outputs.get(index).map(DeclaredIo::name)
    }

    pub fn param_name(&self, index: usize) -> Option<&str> {
        self.params.get(index).map(DeclaredIo::name)
    }

    pub fn buffer_name(&self, index: usize) -> Option<&str> {
        self.buffers.get(index).map(DeclaredBuffer::name)
    }

    pub fn event_name(&self, index: usize) -> Option<&str> {
        self.events.get(index).map(DeclaredEvent::name)
    }

    pub fn input_index(&self, name: &str) -> Option<usize> {
        self.input_index.get(name).copied()
    }

    pub fn output_index(&self, name: &str) -> Option<usize> {
        self.output_index.get(name).copied()
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.param_index.get(name).copied()
    }

    pub fn buffer_index(&self, name: &str) -> Option<usize> {
        self.buffer_index.get(name).copied()
    }

    pub fn event_index(&self, name: &str) -> Option<usize> {
        self.event_index.get(name).copied()
    }

    pub fn input_type(&self, index: usize) -> Option<String> {
        self.inputs.get(index).map(DeclaredIo::type_repr)
    }

    pub fn output_type(&self, index: usize) -> Option<String> {
        self.outputs.get(index).map(DeclaredIo::type_repr)
    }

    pub fn param_type(&self, index: usize) -> Option<String> {
        self.params.get(index).map(DeclaredIo::type_repr)
    }

    pub fn buffer_type(&self, index: usize) -> Option<String> {
        self.buffers.get(index).map(DeclaredBuffer::type_repr)
    }

    pub fn event_payload_bytes(&self, index: usize) -> Option<usize> {
        self.events
            .get(index)
            .and_then(DeclaredEvent::payload_bytes)
    }

    pub fn input_type_bytes(&self, index: usize) -> Option<usize> {
        self.inputs.get(index).map(DeclaredIo::byte_size)
    }

    pub fn output_type_bytes(&self, index: usize) -> Option<usize> {
        self.outputs.get(index).map(DeclaredIo::byte_size)
    }

    pub fn param_type_bytes(&self, index: usize) -> Option<usize> {
        self.params.get(index).map(DeclaredIo::byte_size)
    }

    pub fn param_descriptor(&self, index: usize) -> Option<&DeclaredIo> {
        self.params.get(index)
    }

    pub fn param_slot_count(&self) -> usize {
        self.typed.params.len()
    }

    pub fn param_byte_size(&self) -> usize {
        self.params.iter().map(DeclaredIo::byte_size).sum()
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn event_descriptor(&self, index: usize) -> Option<&DeclaredEvent> {
        self.events.get(index)
    }

    pub fn initialize_state(&self, params: &[u8]) -> Result<RuntimeState, Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return initialize_state_orc(&self.compiled, params);
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = params;
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn process_bound(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        frames: usize,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return process_bound_orc(
                &self.compiled,
                state,
                params,
                frames,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                frames,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn sync_proc_buffer_refs_for_process_bound(
        &self,
        state: &mut RuntimeState,
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return sync_proc_buffer_refs_for_process_bound_orc(
                &self.compiled,
                state,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (state, buffer_ptrs, buffer_frames, buffer_channels);
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub unsafe fn process_bound_unchecked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        frames: u32,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            process_bound_orc_unchecked(
                &self.compiled,
                state,
                params,
                frames,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
            return Ok(());
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                frames,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn trigger_event_by_index(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
    ) -> Result<(), Diagnostic> {
        let Some(desc) = self.event_descriptor(event_index) else {
            return Ok(());
        };
        validate_event_payload(desc, payload)?;
        #[cfg(feature = "llvm-orc")]
        {
            return trigger_event_orc(
                &self.compiled,
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub unsafe fn trigger_event_by_index_unchecked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
    ) -> Result<(), Diagnostic> {
        if self.event_descriptor(event_index).is_none() {
            return Ok(());
        }
        #[cfg(feature = "llvm-orc")]
        {
            trigger_event_orc_unchecked(
                &self.compiled,
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
            return Ok(());
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }
}

#[cfg(feature = "llvm-orc")]
fn process_bound_orc(
    compiled: &orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    frames: usize,
    in_ptrs: &[*const u8],
    out_ptrs: &[*mut u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
) -> Result<(), Diagnostic> {
    let frames = u32::try_from(frames).map_err(|_| {
        Diagnostic::runtime(
            "frame count does not fit u32 for ORC process entrypoint",
            0,
            0,
        )
    })?;
    if in_ptrs.len() != compiled.in_channels() {
        return Err(Diagnostic::runtime(
            format!(
                "runtime input channel pointer count {} does not match compiled program ({})",
                in_ptrs.len(),
                compiled.in_channels()
            ),
            0,
            0,
        ));
    }
    if out_ptrs.len() != compiled.out_channels() {
        return Err(Diagnostic::runtime(
            format!(
                "runtime output channel pointer count {} does not match compiled program ({})",
                out_ptrs.len(),
                compiled.out_channels()
            ),
            0,
            0,
        ));
    }
    if buffer_ptrs.len() != compiled.buffer_count()
        || buffer_frames.len() != compiled.buffer_count()
        || buffer_channels.len() != compiled.buffer_count()
    {
        return Err(Diagnostic::runtime(
            format!(
                "runtime buffer metadata count mismatch: ptrs={}, frames={}, chans={}, expected={}",
                buffer_ptrs.len(),
                buffer_frames.len(),
                buffer_channels.len(),
                compiled.buffer_count()
            ),
            0,
            0,
        ));
    }

    let expected_param_bytes = compiled.param_size_bytes();
    if state.state_size_bytes != compiled.state_size_bytes() {
        return Err(Diagnostic::runtime(
            "runtime state buffer size does not match compiled program state layout",
            0,
            0,
        ));
    }
    if params.len() != expected_param_bytes {
        return Err(Diagnostic::runtime(
            format!(
                "runtime parameter byte count {} does not match compiled program ({expected_param_bytes})",
                params.len()
            ),
            0,
            0,
        ));
    }
    let required_words = (state.state_size_bytes + 7) / 8;
    if state.state_words.len() < required_words {
        return Err(Diagnostic::runtime(
            "runtime state backing storage is smaller than required by compiled program",
            0,
            0,
        ));
    }

    compiled.run(
        in_ptrs.as_ptr(),
        out_ptrs.as_ptr(),
        frames,
        params.as_ptr(),
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs.as_ptr(),
        buffer_frames.as_ptr(),
        buffer_channels.as_ptr(),
    );

    Ok(())
}

#[cfg(feature = "llvm-orc")]
fn sync_proc_buffer_refs_for_process_bound_orc(
    compiled: &orc_backend::OrcProcess,
    state: &mut RuntimeState,
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
) -> Result<(), Diagnostic> {
    if buffer_ptrs.len() != compiled.buffer_count()
        || buffer_frames.len() != compiled.buffer_count()
        || buffer_channels.len() != compiled.buffer_count()
    {
        return Err(Diagnostic::runtime(
            format!(
                "runtime buffer metadata count mismatch: ptrs={}, frames={}, chans={}, expected={}",
                buffer_ptrs.len(),
                buffer_frames.len(),
                buffer_channels.len(),
                compiled.buffer_count()
            ),
            0,
            0,
        ));
    }
    if state.state_size_bytes != compiled.state_size_bytes() {
        return Err(Diagnostic::runtime(
            "runtime state buffer size does not match compiled program state layout",
            0,
            0,
        ));
    }
    let required_words = (state.state_size_bytes + 7) / 8;
    if state.state_words.len() < required_words {
        return Err(Diagnostic::runtime(
            "runtime state backing storage is smaller than required by compiled program",
            0,
            0,
        ));
    }
    compiled.sync_proc_buffer_refs(
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs,
        buffer_frames,
        buffer_channels,
    )?;
    Ok(())
}

#[cfg(feature = "llvm-orc")]
unsafe fn process_bound_orc_unchecked(
    compiled: &orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    frames: u32,
    in_ptrs: &[*const u8],
    out_ptrs: &[*mut u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
) {
    compiled.run(
        in_ptrs.as_ptr(),
        out_ptrs.as_ptr(),
        frames,
        params.as_ptr(),
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs.as_ptr(),
        buffer_frames.as_ptr(),
        buffer_channels.as_ptr(),
    );
}

#[cfg(feature = "llvm-orc")]
fn trigger_event_orc(
    compiled: &orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    event_index: usize,
    payload: &[u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
) -> Result<(), Diagnostic> {
    if buffer_ptrs.len() != compiled.buffer_count()
        || buffer_frames.len() != compiled.buffer_count()
        || buffer_channels.len() != compiled.buffer_count()
    {
        return Err(Diagnostic::runtime(
            format!(
                "runtime buffer metadata count mismatch: ptrs={}, frames={}, chans={}, expected={}",
                buffer_ptrs.len(),
                buffer_frames.len(),
                buffer_channels.len(),
                compiled.buffer_count()
            ),
            0,
            0,
        ));
    }
    if state.state_size_bytes != compiled.state_size_bytes() {
        return Err(Diagnostic::runtime(
            "runtime state buffer size does not match compiled program state layout",
            0,
            0,
        ));
    }
    let expected_param_bytes = compiled.param_size_bytes();
    if params.len() != expected_param_bytes {
        return Err(Diagnostic::runtime(
            format!(
                "runtime parameter byte count {} does not match compiled program ({expected_param_bytes})",
                params.len()
            ),
            0,
            0,
        ));
    }
    let required_words = (state.state_size_bytes + 7) / 8;
    if state.state_words.len() < required_words {
        return Err(Diagnostic::runtime(
            "runtime state backing storage is smaller than required by compiled program",
            0,
            0,
        ));
    }
    let event_index_u32 = u32::try_from(event_index).map_err(|_| {
        Diagnostic::runtime(
            "event index does not fit u32 for ORC event entrypoint",
            0,
            0,
        )
    })?;
    compiled.run_event(
        event_index_u32,
        payload.as_ptr(),
        params.as_ptr(),
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs.as_ptr(),
        buffer_frames.as_ptr(),
        buffer_channels.as_ptr(),
    );
    Ok(())
}

#[cfg(feature = "llvm-orc")]
unsafe fn trigger_event_orc_unchecked(
    compiled: &orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    event_index: usize,
    payload: &[u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
) {
    if let Ok(event_index_u32) = u32::try_from(event_index) {
        compiled.run_event(
            event_index_u32,
            payload.as_ptr(),
            params.as_ptr(),
            state.state_words.as_mut_ptr().cast::<u8>(),
            buffer_ptrs.as_ptr(),
            buffer_frames.as_ptr(),
            buffer_channels.as_ptr(),
        );
    }
}

#[cfg(feature = "llvm-orc")]
fn initialize_state_orc(
    compiled: &orc_backend::OrcProcess,
    params: &[u8],
) -> Result<RuntimeState, Diagnostic> {
    let expected_param_bytes = compiled.param_size_bytes();
    if params.len() != expected_param_bytes {
        return Err(Diagnostic::runtime(
            format!(
                "runtime parameter byte count {} does not match compiled program ({expected_param_bytes})",
                params.len()
            ),
            0,
            0,
        ));
    }
    let state_size_bytes = compiled.state_size_bytes();
    let state_words_len = (state_size_bytes + 7) / 8;
    let mut state_words = vec![0_u64; state_words_len];
    compiled.run_init(params.as_ptr(), state_words.as_mut_ptr().cast::<u8>());
    Ok(RuntimeState {
        state_words,
        state_size_bytes,
    })
}
