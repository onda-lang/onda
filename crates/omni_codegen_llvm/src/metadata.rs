#![cfg_attr(not(feature = "llvm-orc"), allow(dead_code))]

use std::collections::{HashMap, HashSet};

use omni_frontend::{AssignTarget, CallArg, Expr, PrimitiveType, Stmt};
use omni_semantics::{
    TypedArrayInfo, TypedBufferChannels, TypedBufferDecl, TypedConstValue, TypedEventParamDefault,
    TypedEventParamType, TypedFnParam, TypedProgram, TypedValueRange,
};

use crate::primitives::{
    append_typed_const_bytes, primitive_type_bytes, primitive_type_name, typed_const_to_f64,
};
use crate::{
    DeclaredBuffer, DeclaredBufferChannels, DeclaredEvent, DeclaredEventParam, DeclaredIo,
};

pub(crate) struct ProgramMetadata {
    pub(crate) inputs: Vec<DeclaredIo>,
    pub(crate) outputs: Vec<DeclaredIo>,
    pub(crate) params: Vec<DeclaredIo>,
    pub(crate) events: Vec<DeclaredEvent>,
    pub(crate) buffers: Vec<DeclaredBuffer>,
    pub(crate) input_index: HashMap<String, usize>,
    pub(crate) output_index: HashMap<String, usize>,
    pub(crate) param_index: HashMap<String, usize>,
    pub(crate) event_index: HashMap<String, usize>,
    pub(crate) buffer_index: HashMap<String, usize>,
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

    pub fn has_default(&self) -> bool {
        self.default_bytes.is_some()
    }

    pub fn default_bytes(&self) -> Option<&[u8]> {
        self.default_bytes.as_deref()
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

pub(crate) fn build_program_metadata(typed: &TypedProgram) -> ProgramMetadata {
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
    let params = build_declared_param_ios(typed);
    let events = build_declared_events(typed);
    let buffers = build_declared_buffers(typed);

    ProgramMetadata {
        input_index: build_name_to_index(&inputs),
        output_index: build_name_to_index(&outputs),
        param_index: build_name_to_index(&params),
        event_index: build_event_name_to_index(&events),
        buffer_index: buffers
            .iter()
            .enumerate()
            .map(|(idx, buffer)| (buffer.name.clone(), idx))
            .collect(),
        inputs,
        outputs,
        params,
        events,
        buffers,
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
        .map(|(idx, entry)| (entry.name.clone(), idx))
        .collect()
}

fn build_event_name_to_index(entries: &[DeclaredEvent]) -> HashMap<String, usize> {
    entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| (entry.name.clone(), idx))
        .collect()
}

fn build_declared_buffers(typed: &TypedProgram) -> Vec<DeclaredBuffer> {
    let written_top_level_buffers = infer_written_top_level_buffers(typed);
    typed
        .buffers
        .iter()
        .map(|buffer: &TypedBufferDecl| DeclaredBuffer {
            name: buffer.name.clone(),
            elem_ty: buffer.elem_ty,
            channels: match buffer.channels {
                TypedBufferChannels::Mono => DeclaredBufferChannels::Mono,
                TypedBufferChannels::Static(ch) => DeclaredBufferChannels::Static(ch),
                TypedBufferChannels::Dynamic => DeclaredBufferChannels::Dynamic,
            },
            may_write: written_top_level_buffers.contains(&buffer.name),
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
        .map(|buffer| buffer.name.clone())
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
        Expr::ArrayLiteral { values: items, .. } => {
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
        Expr::ArrayCtor { spec, init, .. } => {
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
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
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
        Expr::Tuple { values, .. } => {
            for v in values {
                collect_expr_buffer_write_usage(
                    v,
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
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
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
            if let Expr::Var { name: base, .. } = &first_arg.expr {
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
        if let Expr::Var { name: base, .. } = arg_expr {
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
            if let Some(param_idx) = params.iter().position(|param| param == name) {
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
                        let default_bytes = match &param.default {
                            Some(TypedEventParamDefault::Scalar(value)) => {
                                let mut bytes = Vec::with_capacity(primitive_type_bytes(elem_ty));
                                append_typed_const_bytes(&mut bytes, *value, elem_ty);
                                Some(bytes)
                            }
                            _ => None,
                        };
                        params.push(DeclaredEventParam {
                            name: param.name.clone(),
                            elem_ty,
                            array_len: 1,
                            is_slice: false,
                            byte_offset,
                            default_bytes,
                        });
                        byte_offset = byte_offset.saturating_add(primitive_type_bytes(elem_ty));
                        if let Some(total) = payload_bytes.as_mut() {
                            *total = total.saturating_add(primitive_type_bytes(elem_ty));
                        }
                    }
                    TypedEventParamType::Array { elem, len } => {
                        let default_bytes = match &param.default {
                            Some(TypedEventParamDefault::Array(values)) => {
                                let mut bytes = Vec::with_capacity(
                                    primitive_type_bytes(elem).saturating_mul(len),
                                );
                                for value in values {
                                    append_typed_const_bytes(&mut bytes, *value, elem);
                                }
                                Some(bytes)
                            }
                            _ => None,
                        };
                        params.push(DeclaredEventParam {
                            name: param.name.clone(),
                            elem_ty: elem,
                            array_len: len,
                            is_slice: false,
                            byte_offset,
                            default_bytes,
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
                            default_bytes: None,
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
        .collect()
}
