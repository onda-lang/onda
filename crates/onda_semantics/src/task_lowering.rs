use std::collections::{HashMap, HashSet};

use crate::*;
use onda_frontend::{
    ArrayElemType, ArrayTypeSpec, BinaryOp, CmpOp, FnParamDecl, FnParamType, FnReturnScalarType,
    FnReturnType, LogicalOp, TaskDef, INTERNAL_BARE_RETURN_FN, INTERNAL_TASK_AWAIT_FN,
    INTERNAL_TASK_YIELD_FN,
};

const TASK_FIELD_PREFIX: &str = "__onda_task_";
const TASK_AVAILABLE_FIELD: &str = "__onda_task_available";
const TASK_NODE_LOCAL: &str = "__onda_task_node";
const TASK_RESULT_LOCAL: &str = "__onda_task_result";
const TASK_ABORT_FN: &str = "__onda_task_abort_activation";
const TASK_COMPLETE_PC: i64 = -1;
const TASK_FAILED_PC: i64 = -2;

pub(crate) fn task_available_field() -> &'static str {
    TASK_AVAILABLE_FIELD
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TaskControlContext {
    Task,
    BlockPre,
    Init,
    Event,
    Sample,
    BlockPost,
    Def,
}

impl TaskControlContext {
    fn label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::BlockPre => "block-pre",
            Self::Init => "init",
            Self::Event => "event",
            Self::Sample => "sample",
            Self::BlockPost => "block-post",
            Self::Def => "def",
        }
    }

    fn allows_reset(self) -> bool {
        matches!(self, Self::Init | Self::Event | Self::BlockPre)
    }
}

fn marker_call<'a>(expr: &'a Expr, marker: &str) -> Option<&'a [CallArg]> {
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } if name == marker && type_args.is_empty() => Some(args),
        _ => None,
    }
}

fn await_task_name(expr: &Expr) -> Option<&str> {
    let args = marker_call(expr, INTERNAL_TASK_AWAIT_FN)?;
    let [arg] = args else {
        return None;
    };
    if arg.name.is_some() {
        return None;
    }
    match &arg.expr {
        Expr::Var { name, .. } => Some(name.as_str()),
        _ => None,
    }
}

fn is_yield(expr: &Expr) -> bool {
    marker_call(expr, INTERNAL_TASK_YIELD_FN).is_some_and(<[CallArg]>::is_empty)
}

fn reset_task_name<'a>(expr: &'a Expr, task_names: &HashSet<String>) -> Option<&'a str> {
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        return None;
    };
    if !type_args.is_empty() || !args.is_empty() {
        return None;
    }
    let (receiver, method) = name.rsplit_once('.')?;
    (method == "reset" && task_names.contains(receiver)).then_some(receiver)
}

fn validate_task_control_stmt_list(
    stmts: &[Stmt],
    task_names: &HashSet<String>,
    context: TaskControlContext,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Expr { loc, expr } if is_yield(expr) => {
                if context != TaskControlContext::Task {
                    errors.push(Diagnostic::semantic_span(
                        "yield is only allowed inside a task body",
                        *loc,
                    ));
                }
            }
            Stmt::Expr { loc, expr } if marker_call(expr, INTERNAL_TASK_AWAIT_FN).is_some() => {
                if context != TaskControlContext::BlockPre {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "await is only allowed in the owner's block-pre body, not {} code",
                            context.label()
                        ),
                        *loc,
                    ));
                } else if let Some(name) = await_task_name(expr) {
                    if !task_names.contains(name) {
                        errors.push(Diagnostic::semantic_span(
                            format!("unknown task '{name}' in await"),
                            *loc,
                        ));
                    }
                } else {
                    errors.push(Diagnostic::internal(format!(
                        "malformed internal task await marker at {}:{}",
                        loc.line, loc.column
                    )));
                }
            }
            Stmt::Expr { loc, expr } => {
                if let Some(name) = reset_task_name(expr, task_names) {
                    if !context.allows_reset() {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "task '{name}' can only be reset from init, event, or block-pre code"
                            ),
                            *loc,
                        ));
                    }
                }
            }
            Stmt::Return { loc, expr }
                if context == TaskControlContext::Task && !is_bare_return_expr(expr) =>
            {
                errors.push(Diagnostic::semantic_span(
                    "tasks cannot return a value",
                    *loc,
                ));
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                validate_task_control_stmt_list(then_branch, task_names, context, errors);
                validate_task_control_stmt_list(else_branch, task_names, context, errors);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                validate_task_control_stmt_list(body, task_names, context, errors);
            }
            Stmt::Const { .. }
            | Stmt::Assign { .. }
            | Stmt::Return { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => {}
        }
    }
}

fn validate_task_member_names(proc: &ProcessorDef, errors: &mut Vec<Diagnostic>) {
    let mut members = HashMap::<&str, &'static str>::new();
    for (name, kind) in proc
        .local_defs
        .iter()
        .map(|def| (def.name.as_str(), "proc-local def"))
        .chain(
            proc.events
                .iter()
                .map(|event| (event.name.as_str(), "event")),
        )
        .chain(proc.ins.iter().map(|decl| (decl.name.as_str(), "input")))
        .chain(proc.outs.iter().map(|decl| (decl.name.as_str(), "output")))
        .chain(
            proc.params
                .iter()
                .map(|decl| (decl.name.as_str(), "parameter")),
        )
        .chain(
            proc.buffers
                .iter()
                .map(|decl| (decl.name.as_str(), "buffer")),
        )
        .chain(
            proc.consts
                .iter()
                .map(|decl| (decl.name.as_str(), "constant")),
        )
        .chain(proc.init.body.iter().filter_map(|stmt| match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            } => Some((name.as_str(), "state root")),
            _ => None,
        }))
    {
        members.entry(name).or_insert(kind);
    }

    for task in &proc.tasks {
        if let Some(kind) = members.get(task.name.as_str()) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "task '{}' conflicts with {} '{}' in processor '{}'",
                    task.name, kind, task.name, proc.name
                ),
                task.loc,
            ));
        }
    }
}

fn is_numbered_surface(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn validate_task_expr_access(
    expr: &Expr,
    task: &TaskDef,
    input_names: &HashSet<String>,
    forbidden_calls: &HashMap<String, &'static str>,
    proc_aliases: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Var { name, loc } => {
            if input_names.contains(name) || is_numbered_surface(name, "in") {
                errors.push(Diagnostic::semantic_span(
                    format!("task '{}' cannot read audio input '{name}'", task.name),
                    *loc,
                ));
            }
        }
        Expr::UserCall {
            name, args, loc, ..
        } => {
            if let Some(kind) = forbidden_calls.get(name) {
                errors.push(Diagnostic::semantic_span(
                    format!("task '{}' cannot call {kind} '{name}'", task.name),
                    *loc,
                ));
            } else if name == "init" {
                errors.push(Diagnostic::semantic_span(
                    format!("task '{}' cannot call a processor initializer", task.name),
                    *loc,
                ));
            } else if proc_aliases.contains(name) || name == PROC_INDEX_CALL_SENTINEL {
                errors.push(Diagnostic::semantic_span(
                    if name == PROC_INDEX_CALL_SENTINEL {
                        format!("task '{}' cannot call an indexed processor", task.name)
                    } else {
                        format!("task '{}' cannot call processor '{name}'", task.name)
                    },
                    *loc,
                ));
            }
            for arg in args {
                validate_task_expr_access(
                    &arg.expr,
                    task,
                    input_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
            }
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                validate_task_expr_access(
                    value,
                    task,
                    input_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
            }
        }
        Expr::Index { index, .. } => validate_task_expr_access(
            index,
            task,
            input_names,
            forbidden_calls,
            proc_aliases,
            errors,
        ),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                validate_task_expr_access(
                    coordinate,
                    task,
                    input_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            validate_task_expr_access(
                &spec.size,
                task,
                input_names,
                forbidden_calls,
                proc_aliases,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    validate_task_expr_access(
                        value,
                        task,
                        input_names,
                        forbidden_calls,
                        proc_aliases,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            validate_task_expr_access(
                lhs,
                task,
                input_names,
                forbidden_calls,
                proc_aliases,
                errors,
            );
            validate_task_expr_access(
                rhs,
                task,
                input_names,
                forbidden_calls,
                proc_aliases,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_task_expr_access(
                    arg,
                    task,
                    input_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_task_expr_access(
                expr,
                task,
                input_names,
                forbidden_calls,
                proc_aliases,
                errors,
            )
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn validate_task_body_access(
    statements: &[Stmt],
    task: &TaskDef,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    forbidden_calls: &HashMap<String, &'static str>,
    proc_aliases: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        match statement {
            Stmt::Assign {
                target, expr, loc, ..
            } => {
                let root = match target {
                    AssignTarget::Var(name)
                    | AssignTarget::Index { base: name, .. }
                    | AssignTarget::Slice { base: name, .. } => Some(name.as_str()),
                    AssignTarget::Tuple(_) => None,
                };
                if root.is_some_and(|name| {
                    output_names.contains(name)
                        || is_numbered_surface(name, "out")
                        || is_numbered_surface(name, "kout")
                }) {
                    errors.push(Diagnostic::semantic_span(
                        format!("task '{}' cannot write processor outputs", task.name),
                        *loc,
                    ));
                }
                validate_task_expr_access(
                    expr,
                    task,
                    input_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => validate_task_expr_access(
                expr,
                task,
                input_names,
                forbidden_calls,
                proc_aliases,
                errors,
            ),
            Stmt::Const { decl, .. } => validate_task_expr_access(
                &decl.expr,
                task,
                input_names,
                forbidden_calls,
                proc_aliases,
                errors,
            ),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                validate_task_expr_access(
                    cond,
                    task,
                    input_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
                validate_task_body_access(
                    then_branch,
                    task,
                    input_names,
                    output_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
                validate_task_body_access(
                    else_branch,
                    task,
                    input_names,
                    output_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                for expr in [Some(start), Some(end), step.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    validate_task_expr_access(
                        expr,
                        task,
                        input_names,
                        forbidden_calls,
                        proc_aliases,
                        errors,
                    );
                }
                validate_task_body_access(
                    body,
                    task,
                    input_names,
                    output_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
            }
            Stmt::While { cond, body, .. } => {
                validate_task_expr_access(
                    cond,
                    task,
                    input_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
                validate_task_body_access(
                    body,
                    task,
                    input_names,
                    output_names,
                    forbidden_calls,
                    proc_aliases,
                    errors,
                );
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
}

pub(crate) fn validate_task_source_model(program: &Program, errors: &mut Vec<Diagnostic>) {
    let proc_names = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Proc(proc) => Some(proc.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let top_tasks = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Tasks(tasks) => Some(tasks.tasks.as_slice()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    let top_task_names = top_tasks
        .iter()
        .map(|task| task.name.clone())
        .collect::<HashSet<_>>();
    let top_input_names = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Ins(ports) => Some(ports.decls.as_slice()),
            _ => None,
        })
        .flatten()
        .map(|decl| decl.name.clone())
        .collect::<HashSet<_>>();
    let top_output_names = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Outs(ports) | Block::KOuts(ports) => Some(ports.decls.as_slice()),
            _ => None,
        })
        .flatten()
        .map(|decl| decl.name.clone())
        .collect::<HashSet<_>>();
    let top_event_names = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Events(events) => Some(events.events.as_slice()),
            _ => None,
        })
        .flatten()
        .map(|event| event.name.clone())
        .collect::<HashSet<_>>();
    let mut top_forbidden_calls = proc_names
        .iter()
        .map(|name| (name.clone(), "processor"))
        .collect::<HashMap<_, _>>();
    top_forbidden_calls.extend(
        top_event_names
            .iter()
            .map(|name| (name.clone(), "top-level event")),
    );
    top_forbidden_calls.extend(top_task_names.iter().map(|name| (name.clone(), "task")));
    let top_proc_aliases = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(init.body.as_slice()),
            _ => None,
        })
        .into_iter()
        .flatten()
        .filter_map(|stmt| match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::UserCall { name: ctor, .. },
                ..
            } if proc_names.contains(ctor) => Some(name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    if !top_tasks.is_empty() {
        let mut members = HashMap::<String, &'static str>::new();
        for (name, kind) in program.blocks.iter().filter_map(|block| match block {
            Block::Def(def) => Some((def.name.as_str(), "top-level def")),
            Block::Proc(proc) => Some((proc.name.as_str(), "processor")),
            Block::Const(decl) => Some((decl.name.as_str(), "constant")),
            _ => None,
        }) {
            members.entry(name.to_owned()).or_insert(kind);
        }
        for name in &top_event_names {
            members.entry(name.clone()).or_insert("top-level event");
        }
        for block in &program.blocks {
            let declarations = match block {
                Block::Ins(ports) => Some((ports.decls.as_slice(), "input")),
                Block::Outs(ports) | Block::KOuts(ports) => {
                    Some((ports.decls.as_slice(), "output"))
                }
                _ => None,
            };
            if let Some((declarations, kind)) = declarations {
                for declaration in declarations {
                    members.entry(declaration.name.clone()).or_insert(kind);
                }
            }
            match block {
                Block::Params(params) => {
                    for declaration in &params.decls {
                        members
                            .entry(declaration.name.clone())
                            .or_insert("parameter");
                    }
                }
                Block::Buffers(buffers) => {
                    for declaration in &buffers.decls {
                        members.entry(declaration.name.clone()).or_insert("buffer");
                    }
                }
                Block::Init(init) => {
                    for stmt in &init.body {
                        if let Stmt::Assign {
                            target: AssignTarget::Var(name),
                            ..
                        } = stmt
                        {
                            members.entry(name.clone()).or_insert("state root");
                        }
                    }
                }
                _ => {}
            }
        }
        for task in &top_tasks {
            if let Some(kind) = members.get(&task.name) {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "top-level task '{}' conflicts with {kind} '{}'",
                        task.name, task.name
                    ),
                    task.loc,
                ));
            }
        }
        if let Some(graph) = program.blocks.iter().find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        }) {
            errors.push(Diagnostic::semantic_span(
                "top-level tasks cannot be declared together with a graph block",
                graph.loc,
            ));
        }
    }

    for task in &top_tasks {
        validate_task_control_stmt_list(
            &task.body,
            &top_task_names,
            TaskControlContext::Task,
            errors,
        );
        validate_task_body_access(
            &task.body,
            task,
            &top_input_names,
            &top_output_names,
            &top_forbidden_calls,
            &top_proc_aliases,
            errors,
        );
    }

    for block in &program.blocks {
        match block {
            Block::Init(init) => validate_task_control_stmt_list(
                &init.body,
                &top_task_names,
                TaskControlContext::Init,
                errors,
            ),
            Block::Block(exec) => {
                validate_task_control_stmt_list(
                    &exec.pre,
                    &top_task_names,
                    TaskControlContext::BlockPre,
                    errors,
                );
                if let Some(sample) = &exec.sample {
                    validate_task_control_stmt_list(
                        &sample.body,
                        &top_task_names,
                        TaskControlContext::Sample,
                        errors,
                    );
                }
                validate_task_control_stmt_list(
                    &exec.post,
                    &top_task_names,
                    TaskControlContext::BlockPost,
                    errors,
                );
            }
            Block::Sample(sample) => validate_task_control_stmt_list(
                &sample.body,
                &top_task_names,
                TaskControlContext::Sample,
                errors,
            ),
            Block::Events(events) => {
                for event in &events.events {
                    validate_task_control_stmt_list(
                        &event.body,
                        &top_task_names,
                        TaskControlContext::Event,
                        errors,
                    );
                }
            }
            Block::Tasks(_) => {}
            Block::Def(def) => validate_task_control_stmt_list(
                &def.body,
                &top_task_names,
                TaskControlContext::Def,
                errors,
            ),
            Block::Struct(struct_def) => {
                for method in &struct_def.methods {
                    validate_task_control_stmt_list(
                        &method.body,
                        &top_task_names,
                        TaskControlContext::Def,
                        errors,
                    );
                }
            }
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Const(_)
            | Block::Buffers(_)
            | Block::Assert(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_)
            | Block::Proc(_)
            | Block::Graph(_) => {}
        }
    }

    for proc in program.blocks.iter().filter_map(|block| match block {
        Block::Proc(proc) => Some(proc),
        _ => None,
    }) {
        let task_names = proc
            .tasks
            .iter()
            .map(|task| task.name.clone())
            .collect::<HashSet<_>>();
        let input_names = proc
            .ins
            .iter()
            .map(|decl| decl.name.clone())
            .collect::<HashSet<_>>();
        let output_names = proc
            .outs
            .iter()
            .map(|decl| decl.name.clone())
            .collect::<HashSet<_>>();
        let mut forbidden_calls = proc_names
            .iter()
            .map(|name| (name.clone(), "processor"))
            .collect::<HashMap<_, _>>();
        forbidden_calls.extend(
            proc.events
                .iter()
                .map(|event| (event.name.clone(), "processor event")),
        );
        forbidden_calls.extend(task_names.iter().map(|name| (name.clone(), "task")));
        let proc_aliases = proc
            .init
            .body
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::UserCall { name: ctor, .. },
                    ..
                } if proc_names.contains(ctor) => Some(name.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>();

        validate_task_member_names(proc, errors);
        if !proc.tasks.is_empty() && proc.graph.is_some() {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "processor '{}' cannot declare tasks together with a graph block",
                    proc.name
                ),
                proc.loc,
            ));
        }

        for task in &proc.tasks {
            validate_task_control_stmt_list(
                &task.body,
                &task_names,
                TaskControlContext::Task,
                errors,
            );
            validate_task_body_access(
                &task.body,
                task,
                &input_names,
                &output_names,
                &forbidden_calls,
                &proc_aliases,
                errors,
            );
        }
        validate_task_control_stmt_list(
            &proc.init.body,
            &task_names,
            TaskControlContext::Init,
            errors,
        );
        validate_task_control_stmt_list(
            &proc.block_pre,
            &task_names,
            TaskControlContext::BlockPre,
            errors,
        );
        validate_task_control_stmt_list(
            &proc.sample,
            &task_names,
            TaskControlContext::Sample,
            errors,
        );
        validate_task_control_stmt_list(
            &proc.block_post,
            &task_names,
            TaskControlContext::BlockPost,
            errors,
        );
        for event in &proc.events {
            validate_task_control_stmt_list(
                &event.body,
                &task_names,
                TaskControlContext::Event,
                errors,
            );
        }
        for def in &proc.local_defs {
            validate_task_control_stmt_list(
                &def.body,
                &task_names,
                TaskControlContext::Def,
                errors,
            );
        }
    }
}

#[derive(Clone)]
enum TaskLocalStorage {
    Scalar(PrimitiveType),
    Array {
        spec: ArrayTypeSpec,
        element: PrimitiveType,
        len: usize,
    },
    Tuple(Vec<PrimitiveType>),
}

impl TaskLocalStorage {
    fn tuple_zero_expr(types: &[PrimitiveType]) -> Expr {
        Expr::Tuple {
            loc: Default::default(),
            values: types
                .iter()
                .copied()
                .map(|ty| match ty {
                    PrimitiveType::Bool => Expr::bool(false),
                    _ => cast_expr_to_primitive(zero_expr(ty), ty),
                })
                .collect(),
        }
    }

    fn storage_stmt(&self, name: String, initialize: bool) -> Stmt {
        let (decl_ty, is_typed_decl, expr) = match self {
            Self::Scalar(ty) => (Some(*ty), true, zero_expr(*ty)),
            Self::Array { spec, .. } => (
                None,
                true,
                Expr::ArrayCtor {
                    loc: Default::default(),
                    spec: spec.clone(),
                    init: None,
                    initialize,
                },
            ),
            Self::Tuple(types) => (None, false, Self::tuple_zero_expr(types)),
        };
        Stmt::Assign {
            loc: Default::default(),
            target_loc: Default::default(),
            target: AssignTarget::Var(name),
            decl_ty,
            generic_decl_ty: None,
            is_typed_decl,
            typed_decl_ty_loc: Default::default(),
            expr,
        }
    }

    fn init_stmt(&self, name: String) -> Stmt {
        self.storage_stmt(name, true)
    }

    fn declaration_stmt(&self, name: String) -> Stmt {
        self.storage_stmt(name, false)
    }

    fn reset_stmts(&self, name: String) -> Vec<Stmt> {
        match self {
            Self::Scalar(ty) => vec![assign_var(name, zero_expr(*ty))],
            Self::Array { element, .. } => vec![fill_array(name, zero_expr(*element))],
            Self::Tuple(types) => vec![assign_var(name, Self::tuple_zero_expr(types))],
        }
    }
}

fn task_pc_field(task: &str) -> String {
    format!("{}_pc", task_symbol_stem(task))
}

fn task_local_field(task: &str, local: &str) -> String {
    format!("{}_local_{}_{local}", task_symbol_stem(task), local.len())
}

fn task_resume_def(task: &str) -> String {
    format!("{}_resume", task_symbol_stem(task))
}

fn task_reset_def(task: &str) -> String {
    format!("{}_reset", task_symbol_stem(task))
}

fn task_inline_node_local(task: &str) -> String {
    format!("{}_node", task_symbol_stem(task))
}

fn task_inline_result_local(task: &str) -> String {
    format!("{}_result", task_symbol_stem(task))
}

fn task_inline_scratch_local(task: &str, local: &str) -> String {
    format!("{}_scratch_{}_{local}", task_symbol_stem(task), local.len())
}

fn task_symbol_stem(task: &str) -> String {
    format!("{TASK_FIELD_PREFIX}{}_{task}", task.len())
}

fn bare_return_expr() -> Expr {
    Expr::UserCall {
        loc: Default::default(),
        name: INTERNAL_BARE_RETURN_FN.to_owned(),
        type_args: Vec::new(),
        args: Vec::new(),
    }
}

fn abort_activation_stmt() -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr: user_call(TASK_ABORT_FN, Vec::new()),
    }
}

pub(crate) fn is_task_abort_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } if name == TASK_ABORT_FN && type_args.is_empty() && args.is_empty()
    )
}

pub(crate) fn contains_task_abort(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Expr { expr, .. } => is_task_abort_expr(expr),
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => contains_task_abort(then_branch) || contains_task_abort(else_branch),
        Stmt::For { body, .. } | Stmt::While { body, .. } => contains_task_abort(body),
        _ => false,
    })
}

fn propagate_task_abort_through_loops(stmts: &mut Vec<Stmt>) {
    let mut rewritten = Vec::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        let loop_can_abort = match &mut stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                propagate_task_abort_through_loops(then_branch);
                propagate_task_abort_through_loops(else_branch);
                false
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                propagate_task_abort_through_loops(body);
                contains_task_abort(body)
            }
            _ => false,
        };
        rewritten.push(stmt);
        if loop_can_abort {
            rewritten.push(Stmt::If {
                loc: Default::default(),
                cond: Expr::UnaryNot {
                    loc: Default::default(),
                    expr: Box::new(Expr::var(TASK_AVAILABLE_FIELD)),
                },
                then_branch: vec![abort_activation_stmt()],
                else_branch: Vec::new(),
            });
        }
    }
    *stmts = rewritten;
}

fn assign_var(name: impl Into<String>, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var(name.into()),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

fn assign_index(name: impl Into<String>, index: usize, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Index {
            base: name.into(),
            index: Expr::int(index as i64),
        },
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

fn assign_dynamic_index(name: impl Into<String>, index: impl Into<String>, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Index {
            base: name.into(),
            index: Expr::var(index),
        },
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

fn fill_array(name: impl Into<String>, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Slice {
            base: name.into(),
            selector: None,
            channel: None,
            start: None,
            end: None,
        },
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

fn neutral_output_stmts(name: String, ty: Option<&DeclType>) -> Vec<Stmt> {
    match ty {
        None => vec![assign_var(name, zero_expr(PrimitiveType::F32))],
        Some(DeclType::Scalar(ty)) => vec![assign_var(name, zero_expr(*ty))],
        Some(DeclType::Array {
            elem,
            size: Expr::Int { value, .. },
        }) if *value > 0 => {
            let index = format!("{TASK_FIELD_PREFIX}neutral_output_index");
            vec![Stmt::For {
                loc: Default::default(),
                var: index.clone(),
                var_ty: PrimitiveType::I32,
                step: None,
                start: Expr::int(0),
                end: Expr::int(*value),
                end_inclusive: false,
                body: vec![assign_dynamic_index(name, index, zero_expr(*elem))],
            }]
        }
        // Invalid, unresolved, and non-port declaration types are diagnosed by
        // the ordinary interface analysis. Avoid adding a misleading generated
        // assignment diagnostic on top of the source error.
        Some(_) => Vec::new(),
    }
}

fn collect_neutral_outputs(
    output_names: impl IntoIterator<Item = String>,
    declarations: impl IntoIterator<Item = PortDecl>,
) -> Vec<Stmt> {
    let declarations = declarations
        .into_iter()
        .map(|decl| (decl.name, decl.ty))
        .collect::<HashMap<_, _>>();
    let mut output_names = output_names.into_iter().collect::<Vec<_>>();
    output_names.sort();
    output_names
        .into_iter()
        .flat_map(|name| {
            let ty = declarations.get(&name).and_then(Option::as_ref);
            neutral_output_stmts(name, ty)
        })
        .collect()
}

fn typed_assign(name: impl Into<String>, ty: PrimitiveType, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var(name.into()),
        decl_ty: Some(ty),
        generic_decl_ty: None,
        is_typed_decl: true,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

fn compare(op: CmpOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::Compare {
        loc: Default::default(),
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn logical(op: LogicalOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::Logical {
        loc: Default::default(),
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn user_call(name: impl Into<String>, args: Vec<Expr>) -> Expr {
    Expr::UserCall {
        loc: Default::default(),
        name: name.into(),
        type_args: Vec::new(),
        args: args
            .into_iter()
            .map(|expr| CallArg { name: None, expr })
            .collect(),
    }
}

#[derive(Default)]
struct TaskOwnerTypes {
    scalars: HashMap<String, PrimitiveType>,
    indexed: HashMap<String, PrimitiveType>,
    array_lens: HashMap<String, usize>,
    tuples: HashMap<String, Vec<PrimitiveType>>,
    declared_symbols: DeclaredSymbolMap,
    input_names: HashSet<String>,
    output_names: HashSet<String>,
    param_names: HashSet<String>,
    struct_instances: HashMap<String, String>,
}

#[derive(Clone)]
struct TaskOwnerSurface {
    name: String,
    ins: Vec<PortDecl>,
    outs: Vec<PortDecl>,
    params: Vec<ParamDecl>,
    buffers: Vec<BufferDecl>,
    init_body: Vec<Stmt>,
}

impl TaskOwnerSurface {
    fn from_proc(proc: &ProcessorDef) -> Self {
        Self {
            name: proc.name.clone(),
            ins: proc.ins.clone(),
            outs: proc.outs.clone(),
            params: proc.params.clone(),
            buffers: proc.buffers.clone(),
            init_body: proc.init.body.clone(),
        }
    }

    fn from_top_level(program: &Program) -> Self {
        let mut surface = Self {
            name: "<top-level>".to_owned(),
            ins: Vec::new(),
            outs: Vec::new(),
            params: Vec::new(),
            buffers: Vec::new(),
            init_body: Vec::new(),
        };
        for block in &program.blocks {
            match block {
                Block::Ins(ports) => surface.ins.extend(ports.decls.clone()),
                Block::Outs(ports) | Block::KOuts(ports) => {
                    surface.outs.extend(ports.decls.clone());
                }
                Block::Params(params) => surface.params.extend(params.decls.clone()),
                Block::Buffers(buffers) => surface.buffers.extend(buffers.decls.clone()),
                Block::Init(init) => surface.init_body.extend(init.body.clone()),
                _ => {}
            }
        }
        surface
    }
}

fn record_task_owner_decl_type(types: &mut TaskOwnerTypes, name: &str, ty: Option<&DeclType>) {
    match ty {
        Some(DeclType::Scalar(ty)) => {
            types.scalars.insert(name.to_owned(), *ty);
        }
        Some(DeclType::Array { elem, .. }) => {
            types.indexed.insert(name.to_owned(), *elem);
            if let Some(DeclType::Array {
                size: Expr::Int { value, .. },
                ..
            }) = ty
            {
                if let Ok(len) = usize::try_from(*value) {
                    types.array_lens.insert(name.to_owned(), len);
                }
            }
        }
        Some(DeclType::Tuple(elements)) => {
            types.tuples.insert(name.to_owned(), elements.clone());
        }
        None => {
            types.scalars.insert(name.to_owned(), PrimitiveType::F32);
        }
        Some(DeclType::Generic(_) | DeclType::ArrayGeneric { .. }) => {}
    }
}

fn infer_task_local_storage_type(
    expr: &Expr,
    known: &HashMap<String, TaskLocalStorage>,
    owner_types: &TaskOwnerTypes,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    let local_aliases = known
        .iter()
        .filter_map(|(name, storage)| match storage {
            TaskLocalStorage::Scalar(ty) => Some((name.clone(), *ty)),
            TaskLocalStorage::Array { .. } | TaskLocalStorage::Tuple(_) => None,
        })
        .collect::<LocalAliasTypes>();
    let local_array_aliases = known
        .iter()
        .filter_map(|(name, storage)| match storage {
            TaskLocalStorage::Array { element, len, .. } => Some((
                name.clone(),
                LocalArrayAliasInfo {
                    len: *len,
                    static_len: Some(*len),
                    elem_ty: *element,
                    elem_struct: None,
                    writable: true,
                },
            )),
            TaskLocalStorage::Scalar(_) | TaskLocalStorage::Tuple(_) => None,
        })
        .collect::<HashMap<_, _>>();
    let mut inference_errors = Vec::new();
    let inferred = infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
        expr,
        &owner_types.scalars,
        &owner_types.declared_symbols,
        None,
        &local_aliases,
        &local_array_aliases,
        &HashSet::new(),
        &owner_types.input_names,
        &owner_types.output_names,
        &owner_types.param_names,
        &owner_types.struct_instances,
        struct_defs,
        &HashMap::new(),
        &mut inference_errors,
    );
    inference_errors
        .is_empty()
        .then(|| effective_untyped_assignment_type(expr, inferred))
        .flatten()
}

fn collect_task_owner_types(
    owner: &TaskOwnerSurface,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> TaskOwnerTypes {
    fn record_decl(
        types: &mut TaskOwnerTypes,
        name: &str,
        ty: Option<&DeclType>,
        default: Option<&Expr>,
        struct_defs: &HashMap<String, Vec<TypedStructField>>,
    ) {
        if ty.is_some() {
            record_task_owner_decl_type(types, name, ty);
            return;
        }
        let empty_locals = HashMap::new();
        let inferred = default
            .and_then(|expr| infer_task_local_storage_type(expr, &empty_locals, types, struct_defs))
            .unwrap_or(PrimitiveType::F32);
        types.scalars.insert(name.to_owned(), inferred);
    }

    let mut types = TaskOwnerTypes::default();
    for (name, return_type) in return_types {
        if let Some(ty) = return_type.scalar() {
            types
                .declared_symbols
                .insert(name.clone(), DeclaredSymbolInfo::FunctionReturn { ty });
        }
    }
    for decl in &owner.ins {
        record_decl(
            &mut types,
            &decl.name,
            decl.ty.as_ref(),
            decl.default.as_ref(),
            struct_defs,
        );
    }
    for decl in &owner.outs {
        record_decl(
            &mut types,
            &decl.name,
            decl.ty.as_ref(),
            decl.default.as_ref(),
            struct_defs,
        );
    }
    for decl in &owner.params {
        record_decl(
            &mut types,
            &decl.name,
            decl.ty.as_ref(),
            decl.default.as_ref(),
            struct_defs,
        );
    }
    for decl in &owner.ins {
        types.input_names.insert(decl.name.clone());
        if let Some(ty) = types.scalars.get(&decl.name).copied() {
            types
                .declared_symbols
                .insert(decl.name.clone(), DeclaredSymbolInfo::Input { ty });
        }
    }
    for decl in &owner.outs {
        types.output_names.insert(decl.name.clone());
        if let Some(ty) = types.scalars.get(&decl.name).copied() {
            types
                .declared_symbols
                .insert(decl.name.clone(), DeclaredSymbolInfo::Output { ty });
        }
    }
    for decl in &owner.params {
        types.param_names.insert(decl.name.clone());
        if let Some(ty) = types.scalars.get(&decl.name).copied() {
            types
                .declared_symbols
                .insert(decl.name.clone(), DeclaredSymbolInfo::Param { ty });
        }
    }
    for decl in &owner.buffers {
        if let Some(BufferType {
            elem: BufferElemType::Primitive(elem_ty),
            channels,
        }) = decl.ty.as_ref()
        {
            let channels = match channels {
                BufferChannels::Mono => BufferChannelInfo::Mono,
                BufferChannels::Static(Expr::Int { value, .. }) => usize::try_from(*value)
                    .ok()
                    .filter(|channels| *channels > 0)
                    .map(BufferChannelInfo::Static)
                    .unwrap_or(BufferChannelInfo::Dynamic),
                BufferChannels::Static(_) | BufferChannels::Dynamic => BufferChannelInfo::Dynamic,
            };
            let array_len = match decl.array_size.as_ref() {
                Some(Expr::Int { value, .. }) => usize::try_from(*value).unwrap_or(1),
                Some(_) | None => 1,
            };
            types.indexed.insert(decl.name.clone(), *elem_ty);
            types.declared_symbols.insert(
                decl.name.clone(),
                DeclaredSymbolInfo::Buffer {
                    elem_ty: *elem_ty,
                    channels,
                    array_len,
                    is_array: decl.array_size.is_some(),
                },
            );
        }
    }

    let empty_locals = HashMap::new();
    for stmt in &owner.init_body {
        let Stmt::Assign {
            target: AssignTarget::Var(name),
            decl_ty,
            expr,
            ..
        } = stmt
        else {
            continue;
        };
        if types.scalars.contains_key(name)
            || types.indexed.contains_key(name)
            || types.tuples.contains_key(name)
        {
            continue;
        }
        if let Some(ty) = decl_ty {
            types.scalars.insert(name.clone(), *ty);
        } else if let Expr::UserCall { name: ctor, .. } = expr {
            if struct_defs.contains_key(ctor) {
                register_struct_instance_roots(
                    name,
                    ctor,
                    struct_defs,
                    &mut types.struct_instances,
                );
            } else if let Some(ty) =
                infer_task_local_storage_type(expr, &empty_locals, &types, struct_defs)
            {
                types.scalars.insert(name.clone(), ty);
            }
        } else if let Expr::Var { name: source, .. } = expr {
            if let Some(struct_name) = types.struct_instances.get(source).cloned() {
                register_struct_instance_roots(
                    name,
                    &struct_name,
                    struct_defs,
                    &mut types.struct_instances,
                );
            } else if let Some(ty) =
                infer_task_local_storage_type(expr, &empty_locals, &types, struct_defs)
            {
                types.scalars.insert(name.clone(), ty);
            }
        } else if let Expr::ArrayCtor { spec, .. } = expr {
            if let ArrayElemType::Primitive(element) = spec.elem {
                types.indexed.insert(name.clone(), element);
                if let Expr::Int { value, .. } = spec.size.as_ref() {
                    if let Ok(len) = usize::try_from(*value) {
                        types.array_lens.insert(name.clone(), len);
                    }
                }
                types.declared_symbols.insert(
                    name.clone(),
                    DeclaredSymbolInfo::DataArray { elem_ty: element },
                );
            }
        } else if let Expr::Tuple { values, .. } = expr {
            if let Some(elements) = values
                .iter()
                .map(|value| {
                    infer_task_local_storage_type(value, &empty_locals, &types, struct_defs)
                })
                .collect::<Option<Vec<_>>>()
            {
                types.tuples.insert(name.clone(), elements);
            }
        } else if let Some(ty) =
            infer_task_local_storage_type(expr, &empty_locals, &types, struct_defs)
        {
            types.scalars.insert(name.clone(), ty);
        }
    }
    types
}

fn task_buffer_params(owner: &TaskOwnerSurface, errors: &mut Vec<Diagnostic>) -> Vec<FnParamDecl> {
    owner
        .buffers
        .iter()
        .filter_map(|decl| {
            let Some(buffer) = decl.ty.clone() else {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "cannot determine buffer type for task access to '{}' in processor '{}'",
                        decl.name, owner.name
                    ),
                    decl.loc,
                ));
                return None;
            };
            let ty = match decl.array_size.as_ref() {
                None => FnParamType::Buffer(buffer),
                Some(Expr::Int { value, .. }) => match usize::try_from(*value) {
                    Ok(len) if len > 0 => FnParamType::BufferArray { buffer, len },
                    _ => {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "buffer collection '{}' used by tasks must have a positive fixed size",
                                decl.name
                            ),
                            decl.loc,
                        ));
                        return None;
                    }
                },
                Some(_) => {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "buffer collection '{}' used by tasks must have a resolved fixed size",
                            decl.name
                        ),
                        decl.loc,
                    ));
                    return None;
                }
            };
            Some(FnParamDecl {
                loc: decl.loc,
                name: decl.name.clone(),
                ty: Some(ty),
                ty_loc: decl.ty_loc,
                default: None,
            })
        })
        .collect()
}

fn collect_owner_roots(owner: &TaskOwnerSurface) -> HashSet<String> {
    let mut roots = owner
        .ins
        .iter()
        .map(|decl| decl.name.clone())
        .chain(owner.outs.iter().map(|decl| decl.name.clone()))
        .chain(owner.params.iter().map(|decl| decl.name.clone()))
        .chain(owner.buffers.iter().map(|decl| decl.name.clone()))
        .collect::<HashSet<_>>();
    for stmt in &owner.init_body {
        if let Stmt::Assign {
            target: AssignTarget::Var(name),
            ..
        } = stmt
        {
            roots.insert(name.clone());
        }
    }
    roots
}

fn fresh_task_binding(
    source_name: &str,
    used_names: &mut HashSet<String>,
    next_id: &mut usize,
    source_names: &mut HashMap<String, String>,
) -> String {
    let name = if used_names.insert(source_name.to_owned()) {
        source_name.to_owned()
    } else {
        loop {
            let candidate = format!("__onda_task_binding_{}", *next_id);
            *next_id += 1;
            if used_names.insert(candidate.clone()) {
                break candidate;
            }
        }
    };
    source_names.insert(name.clone(), source_name.to_owned());
    name
}

fn uniquify_task_bindings(
    stmts: &mut [Stmt],
    owner_roots: &HashSet<String>,
) -> HashMap<String, String> {
    struct BindingEnv<'a> {
        owner_roots: &'a HashSet<String>,
        visible: HashMap<String, String>,
        used_names: HashSet<String>,
        source_names: HashMap<String, String>,
        next_id: usize,
    }

    impl BindingEnv<'_> {
        fn fresh(&mut self, source_name: &str) -> String {
            fresh_task_binding(
                source_name,
                &mut self.used_names,
                &mut self.next_id,
                &mut self.source_names,
            )
        }

        fn assignment_name(&mut self, source_name: &str, declares: bool) -> String {
            if !declares {
                if let Some(name) = self.visible.get(source_name) {
                    return name.clone();
                }
                if path_or_ancestor_is_declared(source_name, self.owner_roots) {
                    return source_name.to_owned();
                }
            }
            let name = self.fresh(source_name);
            self.visible.insert(source_name.to_owned(), name.clone());
            name
        }

        fn rewrite_list(&mut self, stmts: &mut [Stmt]) {
            for stmt in stmts {
                match stmt {
                    Stmt::Const { decl, .. } => {
                        rewrite_task_expr(&mut decl.expr, &self.visible);
                    }
                    Stmt::Assign {
                        target,
                        decl_ty,
                        generic_decl_ty,
                        is_typed_decl,
                        expr,
                        ..
                    } => {
                        rewrite_task_expr(expr, &self.visible);
                        match target {
                            AssignTarget::Var(name) => {
                                let source_name = name.clone();
                                *name = self.assignment_name(
                                    &source_name,
                                    *is_typed_decl
                                        || decl_ty.is_some()
                                        || generic_decl_ty.is_some(),
                                );
                            }
                            AssignTarget::Tuple(names) => {
                                for name in names {
                                    let source_name = name.clone();
                                    *name = self.assignment_name(&source_name, false);
                                }
                            }
                            AssignTarget::Index { .. } | AssignTarget::Slice { .. } => {
                                rewrite_task_target(target, &self.visible);
                            }
                        }
                    }
                    Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                        rewrite_task_expr(expr, &self.visible);
                    }
                    Stmt::If {
                        cond,
                        then_branch,
                        else_branch,
                        ..
                    } => {
                        rewrite_task_expr(cond, &self.visible);
                        let outer_visible = self.visible.clone();

                        self.rewrite_list(then_branch);
                        let mut then_visible = self.visible.clone();
                        let then_flow = crate::def_semantics::statement_list_flow(then_branch);

                        self.visible.clone_from(&outer_visible);
                        self.rewrite_list(else_branch);
                        let mut else_visible = self.visible.clone();
                        let else_flow = crate::def_semantics::statement_list_flow(else_branch);

                        self.visible = match (then_flow, else_flow) {
                            (
                                crate::def_semantics::StatementFlow::Continues,
                                crate::def_semantics::StatementFlow::Continues,
                            ) => {
                                let common_sources = then_visible
                                    .keys()
                                    .filter(|source| else_visible.contains_key(*source))
                                    .cloned()
                                    .collect::<Vec<_>>();
                                let mut joined = HashMap::new();
                                for source in common_sources {
                                    let then_name = then_visible
                                        .get(&source)
                                        .expect("common task binding must exist in then branch")
                                        .clone();
                                    let else_name = else_visible
                                        .get(&source)
                                        .expect("common task binding must exist in else branch")
                                        .clone();
                                    let outer_name = outer_visible.get(&source);
                                    let canonical = if outer_name == Some(&then_name) {
                                        then_name.clone()
                                    } else if outer_name == Some(&else_name) {
                                        else_name.clone()
                                    } else {
                                        then_name.clone()
                                    };

                                    if then_name != canonical {
                                        let names =
                                            HashMap::from([(then_name.clone(), canonical.clone())]);
                                        rewrite_task_stmts(then_branch, &names, &HashSet::new());
                                        then_visible.insert(source.clone(), canonical.clone());
                                        self.source_names.remove(&then_name);
                                    }
                                    if else_name != canonical {
                                        let names =
                                            HashMap::from([(else_name.clone(), canonical.clone())]);
                                        rewrite_task_stmts(else_branch, &names, &HashSet::new());
                                        else_visible.insert(source.clone(), canonical.clone());
                                        self.source_names.remove(&else_name);
                                    }
                                    joined.insert(source, canonical);
                                }
                                joined
                            }
                            (
                                crate::def_semantics::StatementFlow::Continues,
                                crate::def_semantics::StatementFlow::Terminates,
                            ) => then_visible,
                            (
                                crate::def_semantics::StatementFlow::Terminates,
                                crate::def_semantics::StatementFlow::Continues,
                            ) => else_visible,
                            (
                                crate::def_semantics::StatementFlow::Terminates,
                                crate::def_semantics::StatementFlow::Terminates,
                            ) => outer_visible,
                        };
                    }
                    Stmt::For {
                        var,
                        start,
                        end,
                        step,
                        body,
                        ..
                    } => {
                        rewrite_task_expr(start, &self.visible);
                        rewrite_task_expr(end, &self.visible);
                        if let Some(step) = step {
                            rewrite_task_expr(step, &self.visible);
                        }
                        let outer_visible = self.visible.clone();
                        let source_name = var.clone();
                        *var = self.fresh(&source_name);
                        self.visible.insert(source_name, var.clone());
                        self.rewrite_list(body);
                        self.visible = outer_visible;
                    }
                    Stmt::While { cond, body, .. } => {
                        rewrite_task_expr(cond, &self.visible);
                        let outer_visible = self.visible.clone();
                        self.rewrite_list(body);
                        self.visible = outer_visible;
                    }
                    Stmt::Break { .. } | Stmt::Continue { .. } => {}
                }
            }
        }
    }

    let mut env = BindingEnv {
        owner_roots,
        visible: HashMap::new(),
        used_names: owner_roots.clone(),
        source_names: HashMap::new(),
        next_id: 0,
    };
    env.used_names.insert(TASK_NODE_LOCAL.to_owned());
    env.rewrite_list(stmts);
    env.source_names
}

#[derive(Default)]
struct TaskBindingStorageTypes {
    scalars: HashMap<String, PrimitiveType>,
    arrays: HashMap<String, LocalArrayAliasInfo>,
    tuples: HashMap<String, Vec<PrimitiveType>>,
}

fn analyze_task_binding_storage(
    stmts: &[Stmt],
    owner_types: &TaskOwnerTypes,
    fn_signatures: &HashMap<String, FnSignature>,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> TaskBindingStorageTypes {
    let output_array_names = owner_types
        .output_names
        .intersection(&owner_types.array_lens.keys().cloned().collect())
        .cloned()
        .collect::<HashSet<_>>();
    let dynamic_param_array_names = owner_types
        .param_names
        .intersection(&owner_types.array_lens.keys().cloned().collect())
        .cloned()
        .collect::<HashSet<_>>();
    let io_surface_names = HashSet::new();
    let io_surface_array_names = HashSet::new();
    let proc_event_names = HashSet::new();
    let common = ScopeAnalysisCtx {
        policy: ScopePolicy::Task,
        input_names: &owner_types.input_names,
        output_names: &owner_types.output_names,
        output_array_names: &output_array_names,
        io_surface_names: &io_surface_names,
        io_surface_array_names: &io_surface_array_names,
        dynamic_param_array_names: &dynamic_param_array_names,
        param_names: &owner_types.param_names,
        struct_defs,
        fn_signatures,
        fn_return_types: return_types,
        options,
        port_index_ins: None,
        port_index_outs: None,
        port_index_params: None,
        port_index_kins: None,
        proc_event_names: &proc_event_names,
    };
    let state_array_struct_roots = HashMap::new();
    let nested_proc_instances = HashMap::new();
    let proc_array_roots = HashMap::new();
    let registration_names = HashSet::new();
    let resolved_scalars = std::cell::RefCell::new(HashMap::new());
    let resolved_arrays = std::cell::RefCell::new(HashMap::new());
    let resolved_tuples = std::cell::RefCell::new(HashMap::new());
    let ctx = FlowStmtAnalysisCtx {
        common,
        registration_mode: RuntimeRegistrationMode::None,
        declared_symbols: &owner_types.declared_symbols,
        state_arrays: &owner_types.array_lens,
        state_array_struct_roots: &state_array_struct_roots,
        nested_proc_instances: &nested_proc_instances,
        struct_instances: &owner_types.struct_instances,
        registration_input_names: &registration_names,
        registration_output_names: &registration_names,
        registration_param_names: &registration_names,
        forbidden_assign_names: &owner_types.output_names,
        forbidden_assign_array_names: &output_array_names,
        proc_array_roots: &proc_array_roots,
        event_policy: None,
        state_tuples: &owner_types.tuples,
        resolved_scalar_locals: Some(&resolved_scalars),
        resolved_array_locals: Some(&resolved_arrays),
        resolved_tuple_locals: Some(&resolved_tuples),
    };
    let mut state_scalars = owner_types.scalars.clone();
    let mut state = ScopeFlowState::new(HashSet::new(), HashMap::new(), HashMap::new());
    analyze_flow_scope_stmts(
        stmts.iter(),
        &HashSet::new(),
        &mut state_scalars,
        &ctx,
        &mut state,
        0,
        0,
        errors,
    );
    let mut scalars = resolved_scalars.into_inner();
    scalars.extend(state.local_aliases);
    TaskBindingStorageTypes {
        scalars,
        arrays: resolved_arrays.into_inner(),
        tuples: resolved_tuples.into_inner(),
    }
}

fn collect_task_locals(
    source_names: &HashMap<String, String>,
    binding_types: &TaskBindingStorageTypes,
    live_across_yield: &HashSet<String>,
    task_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, TaskLocalStorage> {
    let mut locals = HashMap::new();
    for (name, source_name) in source_names {
        if let Some(ty) = binding_types.scalars.get(name) {
            locals.insert(name.clone(), TaskLocalStorage::Scalar(*ty));
        } else if let Some(info) = binding_types.arrays.get(name) {
            if let Some(len) = info.static_len {
                locals.insert(
                    name.clone(),
                    TaskLocalStorage::Array {
                        spec: ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(info.elem_ty),
                            size: Box::new(Expr::int(len as i64)),
                        },
                        element: info.elem_ty,
                        len,
                    },
                );
            } else if live_across_yield.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "task local '{source_name}' is live across a yield in task '{task_name}' but has no fixed primitive, tuple, or fixed-array storage"
                    ),
                    0,
                    0,
                ));
            }
        } else if let Some(types) = binding_types.tuples.get(name) {
            locals.insert(name.clone(), TaskLocalStorage::Tuple(types.clone()));
        } else if live_across_yield.contains(name) {
            errors.push(Diagnostic::semantic(format!(
                "task local '{source_name}' is live across a yield in task '{task_name}' but has no fixed primitive, tuple, or fixed-array storage"
            ), 0, 0));
        }
    }
    locals
}

fn rewrite_task_expr(expr: &mut Expr, names: &HashMap<String, String>) {
    match expr {
        Expr::Var { name, .. } => {
            if let Some(replacement) = names.get(name) {
                *name = replacement.clone();
            }
        }
        Expr::Index { base, index, .. } => {
            if let Some(replacement) = names.get(base) {
                *base = replacement.clone();
            }
            rewrite_task_expr(index, names);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            if let Some(replacement) = names.get(base) {
                *base = replacement.clone();
            }
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_task_expr(coordinate, names);
            }
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_task_expr(value, names);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_task_expr(&mut spec.size, names);
            if let Some(values) = init {
                for value in values {
                    rewrite_task_expr(value, names);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_task_expr(lhs, names);
            rewrite_task_expr(rhs, names);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_task_expr(arg, names);
            }
        }
        Expr::UserCall { name, args, .. } => {
            rewrite_task_callable_name(name, names);
            for arg in args {
                rewrite_task_expr(&mut arg.expr, names);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            rewrite_task_expr(expr, names);
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn rewrite_task_callable_name(name: &mut String, names: &HashMap<String, String>) {
    let Some((receiver, member)) = name.rsplit_once('.') else {
        return;
    };
    if let Some(replacement) = names.get(receiver) {
        *name = format!("{replacement}.{member}");
    }
}

fn rewrite_task_target(target: &mut AssignTarget, names: &HashMap<String, String>) {
    match target {
        AssignTarget::Var(name) => {
            if let Some(replacement) = names.get(name) {
                *name = replacement.clone();
            }
        }
        AssignTarget::Index { base, index } => {
            if let Some(replacement) = names.get(base) {
                *base = replacement.clone();
            }
            rewrite_task_expr(index, names);
        }
        AssignTarget::Slice {
            base,
            selector,
            channel,
            start,
            end,
        } => {
            if let Some(replacement) = names.get(base) {
                *base = replacement.clone();
            }
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_task_expr(coordinate, names);
            }
        }
        AssignTarget::Tuple(values) => {
            for name in values {
                if let Some(replacement) = names.get(name) {
                    *name = replacement.clone();
                }
            }
        }
    }
}

fn rewrite_task_stmts(
    stmts: &mut [Stmt],
    names: &HashMap<String, String>,
    task_locals: &HashSet<String>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => rewrite_task_expr(&mut decl.expr, names),
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => {
                let declared_task_local =
                    matches!(target, AssignTarget::Var(name) if task_locals.contains(name));
                rewrite_task_target(target, names);
                rewrite_task_expr(expr, names);
                if declared_task_local {
                    *decl_ty = None;
                    *generic_decl_ty = None;
                    *is_typed_decl = false;
                }
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => rewrite_task_expr(expr, names),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_task_expr(cond, names);
                rewrite_task_stmts(then_branch, names, task_locals);
                rewrite_task_stmts(else_branch, names, task_locals);
            }
            Stmt::For {
                var,
                start,
                end,
                step,
                body,
                ..
            } => {
                if let Some(replacement) = names.get(var) {
                    *var = replacement.clone();
                }
                rewrite_task_expr(start, names);
                rewrite_task_expr(end, names);
                if let Some(step) = step {
                    rewrite_task_expr(step, names);
                }
                rewrite_task_stmts(body, names, task_locals);
            }
            Stmt::While { cond, body, .. } => {
                rewrite_task_expr(cond, names);
                rewrite_task_stmts(body, names, task_locals);
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
}

fn expand_task_array_initializers(
    stmts: &mut Vec<Stmt>,
    arrays: &HashMap<String, (PrimitiveType, usize)>,
) {
    let mut rewritten = Vec::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        let expansion = match &stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::ArrayCtor { init, .. },
                ..
            } => arrays.get(name).map(|(element, _len)| {
                if let Some(values) = init {
                    values
                        .iter()
                        .cloned()
                        .enumerate()
                        .map(|(index, value)| assign_index(name.clone(), index, value))
                        .collect::<Vec<_>>()
                } else {
                    vec![fill_array(name.clone(), zero_expr(*element))]
                }
            }),
            _ => None,
        };
        if let Some(expansion) = expansion {
            rewritten.extend(expansion);
            continue;
        }

        match &mut stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                expand_task_array_initializers(then_branch, arrays);
                expand_task_array_initializers(else_branch, arrays);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                expand_task_array_initializers(body, arrays);
            }
            _ => {}
        }
        rewritten.push(stmt);
    }
    *stmts = rewritten;
}

fn collect_for_frame_bindings(
    task_name: &str,
    stmts: &[Stmt],
    next_id: &mut usize,
    fields: &mut HashMap<String, TaskForFrameBinding>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_for_frame_bindings(task_name, then_branch, next_id, fields);
                collect_for_frame_bindings(task_name, else_branch, next_id, fields);
            }
            Stmt::For {
                var, var_ty, body, ..
            } => {
                if task_stmts_contain_resume_terminator(body) {
                    let id = *next_id;
                    *next_id += 1;
                    fields.insert(
                        var.clone(),
                        TaskForFrameBinding {
                            end: format!("{}_for_{id}_end", task_symbol_stem(task_name)),
                            step: format!("{}_for_{id}_step", task_symbol_stem(task_name)),
                            ty: *var_ty,
                        },
                    );
                }
                collect_for_frame_bindings(task_name, body, next_id, fields);
            }
            Stmt::While { body, .. } => {
                collect_for_frame_bindings(task_name, body, next_id, fields)
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
struct TaskCfgBlock {
    statements: Vec<Stmt>,
    terminator: TaskCfgTerminator,
}

#[derive(Clone)]
enum TaskCfgTerminator {
    Jump(usize),
    Branch {
        condition: Expr,
        then_block: usize,
        else_block: usize,
    },
    Yield(usize),
    Complete,
}

struct TaskCfgBuilder {
    blocks: Vec<TaskCfgBlock>,
    for_frame_bindings: HashMap<String, TaskForFrameBinding>,
    preserve_structured: bool,
}

#[derive(Clone)]
struct TaskForFrameBinding {
    end: String,
    step: String,
    ty: PrimitiveType,
}

fn collect_expr_uses(expr: &Expr, uses: &mut HashSet<String>) {
    match expr {
        Expr::Var { name, .. } => {
            uses.insert(name.clone());
        }
        Expr::Index { base, index, .. } => {
            uses.insert(base.clone());
            collect_expr_uses(index, uses);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            uses.insert(base.clone());
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_expr_uses(coordinate, uses);
            }
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_expr_uses(value, uses);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_expr_uses(&spec.size, uses);
            if let Some(values) = init {
                for value in values {
                    collect_expr_uses(value, uses);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_expr_uses(lhs, uses);
            collect_expr_uses(rhs, uses);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_uses(arg, uses);
            }
        }
        Expr::UserCall { name, args, .. } => {
            collect_callable_receiver_use(name, uses);
            for arg in args {
                collect_expr_uses(&arg.expr, uses);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_expr_uses(expr, uses)
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn collect_callable_receiver_use(name: &str, uses: &mut HashSet<String>) {
    let Some((receiver, _)) = name.rsplit_once('.') else {
        return;
    };
    uses.insert(receiver.to_owned());
    if let Some(root) = receiver.split('.').next() {
        uses.insert(root.to_owned());
    }
}

fn block_uses_and_defs(block: &TaskCfgBlock) -> (HashSet<String>, HashSet<String>) {
    let mut uses = HashSet::new();
    let mut defs = HashSet::new();
    for stmt in &block.statements {
        match stmt {
            Stmt::Assign { target, expr, .. } => {
                collect_expr_uses(expr, &mut uses);
                match target {
                    AssignTarget::Var(name) => {
                        defs.insert(name.clone());
                    }
                    AssignTarget::Index { base, index } => {
                        uses.insert(base.clone());
                        collect_expr_uses(index, &mut uses);
                    }
                    AssignTarget::Slice {
                        base,
                        selector,
                        channel,
                        start,
                        end,
                    } => {
                        uses.insert(base.clone());
                        for coordinate in [selector, channel, start, end].into_iter().flatten() {
                            collect_expr_uses(coordinate, &mut uses);
                        }
                    }
                    AssignTarget::Tuple(names) => defs.extend(names.iter().cloned()),
                }
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_expr_uses(expr, &mut uses)
            }
            Stmt::Const { decl, .. } => collect_expr_uses(&decl.expr, &mut uses),
            _ => {}
        }
    }
    if let TaskCfgTerminator::Branch { condition, .. } = &block.terminator {
        collect_expr_uses(condition, &mut uses);
    }
    (uses, defs)
}

fn task_locals_live_across_yield(body: &[Stmt], task_locals: &HashSet<String>) -> HashSet<String> {
    let mut cfg = TaskCfgBuilder {
        blocks: Vec::new(),
        for_frame_bindings: HashMap::new(),
        preserve_structured: false,
    };
    let complete = cfg.push(Vec::new(), TaskCfgTerminator::Complete);
    let _ = cfg.lower_list(body, complete, None);
    let uses_defs = cfg
        .blocks
        .iter()
        .map(block_uses_and_defs)
        .collect::<Vec<_>>();
    let mut live_in = vec![HashSet::<String>::new(); cfg.blocks.len()];
    loop {
        let mut changed = false;
        for id in (0..cfg.blocks.len()).rev() {
            let successors = match &cfg.blocks[id].terminator {
                TaskCfgTerminator::Jump(target) | TaskCfgTerminator::Yield(target) => {
                    vec![*target]
                }
                TaskCfgTerminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => vec![*then_block, *else_block],
                TaskCfgTerminator::Complete => Vec::new(),
            };
            let mut next = successors
                .into_iter()
                .flat_map(|successor| live_in[successor].iter().cloned())
                .collect::<HashSet<_>>();
            let (uses, defs) = &uses_defs[id];
            next.retain(|name| !defs.contains(name));
            next.extend(uses.iter().cloned());
            if next != live_in[id] {
                live_in[id] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    cfg.blocks
        .iter()
        .filter_map(|block| match block.terminator {
            TaskCfgTerminator::Yield(target) => Some(target),
            _ => None,
        })
        .flat_map(|target| live_in[target].iter().cloned())
        .filter(|name| task_locals.contains(name))
        .collect()
}

impl TaskCfgBuilder {
    fn push(&mut self, statements: Vec<Stmt>, terminator: TaskCfgTerminator) -> usize {
        let id = self.blocks.len();
        self.blocks.push(TaskCfgBlock {
            statements,
            terminator,
        });
        id
    }

    fn lower_list(
        &mut self,
        statements: &[Stmt],
        mut next: usize,
        loop_targets: Option<(usize, usize)>,
    ) -> usize {
        let mut straight_line = Vec::new();
        for statement in statements.iter().rev() {
            if self.preserve_structured && task_stmt_can_remain_structured(statement) {
                straight_line.push(statement.clone());
                continue;
            }
            if !straight_line.is_empty() {
                straight_line.reverse();
                next = self.push(
                    std::mem::take(&mut straight_line),
                    TaskCfgTerminator::Jump(next),
                );
            }
            next = self.lower_stmt(statement, next, loop_targets);
        }
        if !straight_line.is_empty() {
            straight_line.reverse();
            next = self.push(straight_line, TaskCfgTerminator::Jump(next));
        }
        next
    }

    fn lower_stmt(
        &mut self,
        statement: &Stmt,
        next: usize,
        loop_targets: Option<(usize, usize)>,
    ) -> usize {
        match statement {
            Stmt::Expr { expr, .. } if is_yield(expr) => {
                self.push(Vec::new(), TaskCfgTerminator::Yield(next))
            }
            Stmt::Return { .. } => self.push(Vec::new(), TaskCfgTerminator::Complete),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let then_block = self.lower_list(then_branch, next, loop_targets);
                let else_block = self.lower_list(else_branch, next, loop_targets);
                self.push(
                    Vec::new(),
                    TaskCfgTerminator::Branch {
                        condition: cond.clone(),
                        then_block,
                        else_block,
                    },
                )
            }
            Stmt::While { cond, body, .. } => {
                let header = self.push(Vec::new(), TaskCfgTerminator::Complete);
                let body = self.lower_list(body, header, Some((next, header)));
                self.blocks[header].terminator = TaskCfgTerminator::Branch {
                    condition: cond.clone(),
                    then_block: body,
                    else_block: next,
                };
                header
            }
            Stmt::For {
                var,
                var_ty,
                start,
                end,
                step,
                end_inclusive,
                body,
                ..
            } => {
                let induction_ty = *var_ty;
                let step_expr = cast_expr_to_primitive(
                    step.clone().unwrap_or_else(|| Expr::int(1)),
                    induction_ty,
                );
                let frame = self
                    .for_frame_bindings
                    .get(var)
                    .cloned()
                    .unwrap_or_else(|| TaskForFrameBinding {
                        end: format!("{var}__end"),
                        step: format!("{var}__step"),
                        ty: induction_ty,
                    });
                debug_assert_eq!(frame.ty, induction_ty);
                let end_field = frame.end;
                let step_field = frame.step;
                let positive = compare(CmpOp::Gt, Expr::var(step_field.clone()), Expr::int(0));
                let negative = compare(CmpOp::Lt, Expr::var(step_field.clone()), Expr::int(0));
                let advanced = || Expr::Binary {
                    loc: Default::default(),
                    op: BinaryOp::Add,
                    lhs: Box::new(Expr::var(var.clone())),
                    rhs: Box::new(Expr::var(step_field.clone())),
                };
                let header = self.push(Vec::new(), TaskCfgTerminator::Complete);
                let increment = self.push(
                    vec![assign_var(var.clone(), advanced())],
                    TaskCfgTerminator::Jump(header),
                );
                // Stop before signed addition wraps and makes an extrema-bound loop re-enter.
                let latch = self.push(
                    Vec::new(),
                    TaskCfgTerminator::Branch {
                        condition: logical(
                            LogicalOp::Or,
                            logical(
                                LogicalOp::And,
                                positive.clone(),
                                compare(CmpOp::Gt, advanced(), Expr::var(var.clone())),
                            ),
                            logical(
                                LogicalOp::And,
                                negative.clone(),
                                compare(CmpOp::Lt, advanced(), Expr::var(var.clone())),
                            ),
                        ),
                        then_block: increment,
                        else_block: next,
                    },
                );
                let body = self.lower_list(body, latch, Some((next, latch)));
                let forward = compare(
                    if *end_inclusive { CmpOp::Le } else { CmpOp::Lt },
                    Expr::var(var.clone()),
                    Expr::var(end_field.clone()),
                );
                let backward = compare(
                    if *end_inclusive { CmpOp::Ge } else { CmpOp::Gt },
                    Expr::var(var.clone()),
                    Expr::var(end_field.clone()),
                );
                self.blocks[header].terminator = TaskCfgTerminator::Branch {
                    condition: logical(
                        LogicalOp::Or,
                        logical(LogicalOp::And, positive, forward),
                        logical(LogicalOp::And, negative, backward),
                    ),
                    then_block: body,
                    else_block: next,
                };
                self.push(
                    vec![
                        assign_var(
                            var.clone(),
                            cast_expr_to_primitive(start.clone(), induction_ty),
                        ),
                        assign_var(end_field, cast_expr_to_primitive(end.clone(), induction_ty)),
                        assign_var(step_field, step_expr),
                    ],
                    TaskCfgTerminator::Jump(header),
                )
            }
            Stmt::Break { .. } => {
                let target = loop_targets.map(|targets| targets.0).unwrap_or(next);
                self.push(Vec::new(), TaskCfgTerminator::Jump(target))
            }
            Stmt::Continue { .. } => {
                let target = loop_targets.map(|targets| targets.1).unwrap_or(next);
                self.push(Vec::new(), TaskCfgTerminator::Jump(target))
            }
            _ => self.push(vec![statement.clone()], TaskCfgTerminator::Jump(next)),
        }
    }
}

fn task_stmt_can_remain_structured(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
        Stmt::Expr { expr, .. } if is_yield(expr) => false,
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            !task_stmts_contain_resume_terminator(then_branch)
                && !task_stmts_contain_resume_terminator(else_branch)
                && !task_stmts_have_unbound_loop_control(then_branch, 0)
                && !task_stmts_have_unbound_loop_control(else_branch, 0)
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            !task_stmts_contain_resume_terminator(body)
        }
        Stmt::Const { .. } | Stmt::Assign { .. } | Stmt::Expr { .. } => true,
    }
}

fn task_stmts_contain_resume_terminator(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Return { .. } => true,
        Stmt::Expr { expr, .. } => is_yield(expr),
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            task_stmts_contain_resume_terminator(then_branch)
                || task_stmts_contain_resume_terminator(else_branch)
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            task_stmts_contain_resume_terminator(body)
        }
        _ => false,
    })
}

fn task_stmts_have_unbound_loop_control(stmts: &[Stmt], loop_depth: usize) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Break { .. } | Stmt::Continue { .. } => loop_depth == 0,
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            task_stmts_have_unbound_loop_control(then_branch, loop_depth)
                || task_stmts_have_unbound_loop_control(else_branch, loop_depth)
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            task_stmts_have_unbound_loop_control(body, loop_depth + 1)
        }
        _ => false,
    })
}

fn cfg_pc(id: usize) -> Expr {
    Expr::int((id + 1) as i64)
}

fn build_task_dispatch(mut arms: Vec<Vec<Stmt>>, first_id: usize, node: &str) -> Vec<Stmt> {
    debug_assert!(!arms.is_empty());
    if arms.len() == 1 {
        return arms.pop().expect("task dispatch must contain an arm");
    }

    let right = arms.split_off(arms.len() / 2);
    let last_left_id = first_id + arms.len() - 1;
    vec![Stmt::If {
        loc: Default::default(),
        cond: compare(CmpOp::Le, Expr::var(node), cfg_pc(last_left_id)),
        then_branch: build_task_dispatch(arms, first_id, node),
        else_branch: build_task_dispatch(right, last_left_id + 1, node),
    }]
}

fn compile_task_resume_body(
    task_name: &str,
    body: &[Stmt],
    local_initializers: Vec<Stmt>,
    node: &str,
    result: &str,
    declare_scratch: bool,
    for_frame_bindings: &HashMap<String, TaskForFrameBinding>,
) -> Vec<Stmt> {
    let mut cfg = TaskCfgBuilder {
        blocks: Vec::new(),
        for_frame_bindings: for_frame_bindings.clone(),
        preserve_structured: true,
    };
    let complete = cfg.push(Vec::new(), TaskCfgTerminator::Complete);
    let entry = cfg.lower_list(body, complete, None);
    let pc = task_pc_field(task_name);
    let mut execution = local_initializers;
    execution.push(if declare_scratch {
        typed_assign(node, PrimitiveType::I32, Expr::var(pc.clone()))
    } else {
        assign_var(node, Expr::var(pc.clone()))
    });
    execution.push(Stmt::If {
        loc: Default::default(),
        cond: compare(CmpOp::Eq, Expr::var(node), Expr::int(0)),
        then_branch: vec![assign_var(node, cfg_pc(entry))],
        else_branch: Vec::new(),
    });
    // A runtime check can return from generated code at any following
    // statement. Publish Failed first, then replace it only along the two
    // successful exits (yield and completion).
    execution.push(assign_var(pc.clone(), Expr::int(TASK_FAILED_PC)));

    let mut dispatch_arms = Vec::with_capacity(cfg.blocks.len());
    for block in &cfg.blocks {
        let mut arm = block.statements.clone();
        match &block.terminator {
            TaskCfgTerminator::Jump(target) => {
                arm.push(assign_var(node, cfg_pc(*target)));
                arm.push(Stmt::Continue {
                    loc: Default::default(),
                });
            }
            TaskCfgTerminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                arm.push(Stmt::If {
                    loc: Default::default(),
                    cond: condition.clone(),
                    then_branch: vec![assign_var(node, cfg_pc(*then_block))],
                    else_branch: vec![assign_var(node, cfg_pc(*else_block))],
                });
                arm.push(Stmt::Continue {
                    loc: Default::default(),
                });
            }
            TaskCfgTerminator::Yield(target) => {
                arm.push(assign_var(pc.clone(), cfg_pc(*target)));
                arm.push(assign_var(result, Expr::bool(false)));
                arm.push(assign_var(node, Expr::int(0)));
            }
            TaskCfgTerminator::Complete => {
                arm.push(assign_var(pc.clone(), Expr::int(TASK_COMPLETE_PC)));
                arm.push(assign_var(result, Expr::bool(true)));
                arm.push(assign_var(node, Expr::int(0)));
            }
        }
        dispatch_arms.push(arm);
    }
    let invalid_node_exit = vec![
        assign_var(pc.clone(), Expr::int(TASK_FAILED_PC)),
        assign_var(result, Expr::bool(false)),
        assign_var(node, Expr::int(0)),
    ];
    execution.push(Stmt::If {
        loc: Default::default(),
        cond: compare(CmpOp::Lt, Expr::var(node), cfg_pc(0)),
        then_branch: invalid_node_exit.clone(),
        else_branch: Vec::new(),
    });
    execution.push(Stmt::If {
        loc: Default::default(),
        cond: compare(
            CmpOp::Gt,
            Expr::var(node),
            Expr::int(cfg.blocks.len() as i64),
        ),
        then_branch: invalid_node_exit,
        else_branch: Vec::new(),
    });
    execution.push(Stmt::While {
        loc: Default::default(),
        cond: compare(CmpOp::Ne, Expr::var(node), Expr::int(0)),
        body: build_task_dispatch(dispatch_arms, 0, node),
    });

    let initialize_result = if declare_scratch {
        typed_assign(result, PrimitiveType::Bool, Expr::bool(false))
    } else {
        assign_var(result, Expr::bool(false))
    };
    vec![
        initialize_result,
        Stmt::If {
            loc: Default::default(),
            cond: compare(
                CmpOp::Eq,
                Expr::var(pc.clone()),
                Expr::int(TASK_COMPLETE_PC),
            ),
            then_branch: vec![assign_var(result, Expr::bool(true))],
            else_branch: vec![Stmt::If {
                loc: Default::default(),
                cond: compare(CmpOp::Ne, Expr::var(pc), Expr::int(TASK_FAILED_PC)),
                then_branch: execution,
                else_branch: Vec::new(),
            }],
        },
    ]
}

fn compile_task_resume(
    task_name: &str,
    body: &[Stmt],
    local_initializers: Vec<Stmt>,
    params: Vec<FnParamDecl>,
    for_frame_bindings: &HashMap<String, TaskForFrameBinding>,
) -> FunctionDef {
    let mut function_body = compile_task_resume_body(
        task_name,
        body,
        local_initializers,
        TASK_NODE_LOCAL,
        TASK_RESULT_LOCAL,
        true,
        for_frame_bindings,
    );
    function_body.push(Stmt::Return {
        loc: Default::default(),
        expr: Expr::var(TASK_RESULT_LOCAL),
    });

    FunctionDef {
        loc: Default::default(),
        is_const: false,
        type_params: Vec::new(),
        name: task_resume_def(task_name),
        params,
        return_ty: Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(
            PrimitiveType::Bool,
        ))),
        return_ty_loc: Default::default(),
        body: function_body,
    }
}

fn compile_inline_task_resume(
    task_name: &str,
    body: &[Stmt],
    local_initializers: Vec<Stmt>,
    for_frame_bindings: &HashMap<String, TaskForFrameBinding>,
) -> Vec<Stmt> {
    compile_task_resume_body(
        task_name,
        body,
        local_initializers,
        &task_inline_node_local(task_name),
        &task_inline_result_local(task_name),
        false,
        for_frame_bindings,
    )
}

fn rewrite_task_controls(
    stmts: &mut Vec<Stmt>,
    task_names: &HashSet<String>,
    buffer_names: &[String],
    unavailable: &[Stmt],
) {
    let mut rewritten = Vec::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        match &mut stmt {
            Stmt::Expr { expr, .. } => {
                if let Some(task) = await_task_name(expr).map(str::to_owned) {
                    rewritten.push(Stmt::If {
                        loc: Default::default(),
                        cond: Expr::UnaryNot {
                            loc: Default::default(),
                            expr: Box::new(user_call(
                                task_resume_def(&task),
                                buffer_names.iter().cloned().map(Expr::var).collect(),
                            )),
                        },
                        then_branch: unavailable.to_vec(),
                        else_branch: Vec::new(),
                    });
                    continue;
                }
                if let Some(task) = reset_task_name(expr, task_names).map(str::to_owned) {
                    *expr = user_call(task_reset_def(&task), Vec::new());
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_task_controls(then_branch, task_names, buffer_names, unavailable);
                rewrite_task_controls(else_branch, task_names, buffer_names, unavailable);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                rewrite_task_controls(body, task_names, buffer_names, unavailable)
            }
            _ => {}
        }
        rewritten.push(stmt);
    }
    *stmts = rewritten;
}

fn materialize_task_abort_in_stmts(stmts: &mut Vec<Stmt>) {
    for stmt in stmts {
        match stmt {
            Stmt::Expr {
                loc,
                expr:
                    Expr::UserCall {
                        name,
                        type_args,
                        args,
                        ..
                    },
            } if name == TASK_ABORT_FN && type_args.is_empty() && args.is_empty() => {
                *stmt = Stmt::Return {
                    loc: *loc,
                    expr: bare_return_expr(),
                };
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                materialize_task_abort_in_stmts(then_branch);
                materialize_task_abort_in_stmts(else_branch);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                materialize_task_abort_in_stmts(body)
            }
            _ => {}
        }
    }
}

pub(crate) fn materialize_task_abort_returns(blocks: &mut [Block]) {
    for block in blocks {
        if let Block::Def(def) = block {
            materialize_task_abort_in_stmts(&mut def.body);
        }
    }
}

fn collect_numbered_outputs(stmts: &[Stmt], prefix: &str, outputs: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            } if name.strip_prefix(prefix).is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())
            }) =>
            {
                outputs.insert(name.clone());
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_numbered_outputs(then_branch, prefix, outputs);
                collect_numbered_outputs(else_branch, prefix, outputs);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                collect_numbered_outputs(body, prefix, outputs)
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
enum TaskResumePlacement {
    HelperFunction,
    Inline,
}

struct PreparedTask {
    name: String,
    body: Vec<Stmt>,
    resume_local_initializers: Vec<Stmt>,
    inline_scratch_declarations: Vec<Stmt>,
    init_stmts: Vec<Stmt>,
    reset_stmts: Vec<Stmt>,
    pinned_fields: Vec<String>,
    for_frame_bindings: HashMap<String, TaskForFrameBinding>,
}

#[derive(Clone)]
struct InlineTaskExpansion {
    resume: Vec<Stmt>,
    reset: Vec<Stmt>,
    result: String,
}

#[allow(clippy::too_many_arguments)]
fn prepare_task(
    task: &TaskDef,
    owner_roots: &HashSet<String>,
    return_types: &HashMap<String, ReturnType>,
    fn_signatures: &HashMap<String, FnSignature>,
    owner_types: &TaskOwnerTypes,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    options: AnalysisOptions,
    placement: TaskResumePlacement,
    errors: &mut Vec<Diagnostic>,
) -> PreparedTask {
    let mut body = task.body.clone();
    let mut task_flow_errors = Vec::new();
    analyze_task_binding_storage(
        &body,
        owner_types,
        fn_signatures,
        return_types,
        struct_defs,
        options,
        &mut task_flow_errors,
    );
    errors.extend(task_flow_errors.into_iter().filter(|error| {
        error.message.starts_with("binding '")
            || error.message.starts_with("typed array declaration")
    }));
    let source_names = uniquify_task_bindings(&mut body, owner_roots);
    let task_binding_names = source_names.keys().cloned().collect::<HashSet<_>>();
    let live_across_yield = task_locals_live_across_yield(&body, &task_binding_names);
    let mut ignored_errors = Vec::new();
    let binding_types = analyze_task_binding_storage(
        &body,
        owner_types,
        fn_signatures,
        return_types,
        struct_defs,
        options,
        &mut ignored_errors,
    );
    let locals = collect_task_locals(
        &source_names,
        &binding_types,
        &live_across_yield,
        &task.name,
        errors,
    );
    let task_local_names = locals.keys().cloned().collect::<HashSet<_>>();
    let names = locals
        .keys()
        .filter_map(|name| {
            if live_across_yield.contains(name) {
                Some((name.clone(), task_local_field(&task.name, name)))
            } else if matches!(placement, TaskResumePlacement::Inline) {
                Some((name.clone(), task_inline_scratch_local(&task.name, name)))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>();
    rewrite_task_stmts(&mut body, &names, &task_local_names);
    let task_arrays = locals
        .iter()
        .filter_map(|(name, storage)| {
            let TaskLocalStorage::Array { element, len, .. } = storage else {
                return None;
            };
            let lowered_name = names.get(name).cloned().unwrap_or_else(|| name.clone());
            Some((lowered_name, (*element, *len)))
        })
        .collect::<HashMap<_, _>>();
    expand_task_array_initializers(&mut body, &task_arrays);

    let mut init_stmts = vec![typed_assign(
        task_pc_field(&task.name),
        PrimitiveType::I32,
        Expr::int(0),
    )];
    let mut pinned_fields = vec![task_pc_field(&task.name)];
    let mut reset_stmts = vec![assign_var(task_pc_field(&task.name), Expr::int(0))];
    let mut resume_local_initializers = Vec::new();
    let inline_scratch_declarations = vec![
        typed_assign(
            task_inline_node_local(&task.name),
            PrimitiveType::I32,
            Expr::int(0),
        ),
        typed_assign(
            task_inline_result_local(&task.name),
            PrimitiveType::Bool,
            Expr::bool(false),
        ),
    ];

    let mut local_names = locals.keys().cloned().collect::<Vec<_>>();
    local_names.sort();
    for local in local_names {
        let storage = &locals[&local];
        if live_across_yield.contains(&local) {
            let field = names[&local].clone();
            init_stmts.push(storage.init_stmt(field.clone()));
            pinned_fields.push(field.clone());
            reset_stmts.extend(storage.reset_stmts(field));
        } else {
            match placement {
                TaskResumePlacement::HelperFunction => {
                    resume_local_initializers.push(storage.declaration_stmt(local));
                }
                TaskResumePlacement::Inline => {
                    let scratch = names[&local].clone();
                    resume_local_initializers.push(storage.declaration_stmt(scratch));
                }
            }
        }
    }

    let mut for_frame_bindings = HashMap::new();
    let mut next_for_frame_id = 0;
    collect_for_frame_bindings(
        &task.name,
        &body,
        &mut next_for_frame_id,
        &mut for_frame_bindings,
    );
    let mut for_frame_fields = for_frame_bindings
        .values()
        .flat_map(|frame| {
            [
                (frame.end.clone(), frame.ty),
                (frame.step.clone(), frame.ty),
            ]
        })
        .collect::<Vec<_>>();
    for_frame_fields.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
    for (field, ty) in for_frame_fields {
        init_stmts.push(typed_assign(field.clone(), ty, Expr::int(0)));
        pinned_fields.push(field.clone());
        reset_stmts.push(assign_var(field, Expr::int(0)));
    }

    PreparedTask {
        name: task.name.clone(),
        body,
        resume_local_initializers,
        inline_scratch_declarations,
        init_stmts,
        reset_stmts,
        pinned_fields,
        for_frame_bindings,
    }
}

fn rewrite_inline_task_controls(
    stmts: &mut Vec<Stmt>,
    tasks: &HashMap<String, InlineTaskExpansion>,
    task_names: &HashSet<String>,
    unavailable: &[Stmt],
) {
    let mut rewritten = Vec::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        match &mut stmt {
            Stmt::Expr { expr, .. } => {
                if let Some(task) = await_task_name(expr) {
                    if let Some(expansion) = tasks.get(task) {
                        rewritten.extend(expansion.resume.clone());
                        rewritten.push(Stmt::If {
                            loc: Default::default(),
                            cond: Expr::UnaryNot {
                                loc: Default::default(),
                                expr: Box::new(Expr::var(&expansion.result)),
                            },
                            then_branch: unavailable.to_vec(),
                            else_branch: Vec::new(),
                        });
                        continue;
                    }
                }
                if let Some(task) = reset_task_name(expr, task_names) {
                    if let Some(expansion) = tasks.get(task) {
                        rewritten.extend(expansion.reset.clone());
                        continue;
                    }
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_inline_task_controls(then_branch, tasks, task_names, unavailable);
                rewrite_inline_task_controls(else_branch, tasks, task_names, unavailable);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                rewrite_inline_task_controls(body, tasks, task_names, unavailable);
            }
            _ => {}
        }
        rewritten.push(stmt);
    }
    *stmts = rewritten;
}

fn take_top_level_tasks(program: &mut Program) -> Vec<TaskDef> {
    let mut tasks = Vec::new();
    let mut blocks = Vec::with_capacity(program.blocks.len());
    for block in std::mem::take(&mut program.blocks) {
        match block {
            Block::Tasks(mut block) => tasks.append(&mut block.tasks),
            block => blocks.push(block),
        }
    }
    program.blocks = blocks;
    tasks
}

fn lower_top_level_tasks(
    program: &mut Program,
    tasks: &[TaskDef],
    call_semantics: &TaskCallSemantics,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if tasks.is_empty() {
        return;
    }

    let owner_surface = TaskOwnerSurface::from_top_level(program);
    let owner_types =
        collect_task_owner_types(&owner_surface, &call_semantics.return_types, struct_defs);
    let call_env = task_call_type_env(&owner_surface, &owner_types);
    let owner_roots = collect_owner_roots(&owner_surface);
    let mut init_prefix = vec![typed_assign(
        TASK_AVAILABLE_FIELD,
        PrimitiveType::Bool,
        Expr::bool(true),
    )];
    let mut pinned_fields = Vec::new();
    let mut scratch_declarations = Vec::new();
    let mut expansions = HashMap::new();
    for task in tasks {
        let mut task = task.clone();
        rewrite_task_overloads(&mut task, &call_env, call_semantics, struct_defs);
        let prepared = prepare_task(
            &task,
            &owner_roots,
            &call_semantics.return_types,
            &call_semantics.signatures,
            &owner_types,
            struct_defs,
            options,
            TaskResumePlacement::Inline,
            errors,
        );
        init_prefix.extend(prepared.init_stmts);
        pinned_fields.extend(prepared.pinned_fields);
        scratch_declarations.extend(prepared.inline_scratch_declarations);
        let for_frame_bindings = prepared.for_frame_bindings.clone();
        expansions.insert(
            prepared.name.clone(),
            InlineTaskExpansion {
                resume: compile_inline_task_resume(
                    &prepared.name,
                    &prepared.body,
                    prepared.resume_local_initializers,
                    &for_frame_bindings,
                ),
                reset: prepared.reset_stmts,
                result: task_inline_result_local(&prepared.name),
            },
        );
    }

    let mut audio_outputs = HashSet::new();
    let mut audio_output_decls = Vec::new();
    let mut control_outputs = HashSet::new();
    let mut control_output_decls = Vec::new();
    for block in &program.blocks {
        match block {
            Block::Outs(ports) => {
                audio_outputs.extend(ports.decls.iter().map(|decl| decl.name.clone()));
                audio_output_decls.extend(ports.decls.iter().cloned());
            }
            Block::KOuts(ports) => {
                control_outputs.extend(ports.decls.iter().map(|decl| decl.name.clone()));
                control_output_decls.extend(ports.decls.iter().cloned());
            }
            Block::Block(exec) => {
                collect_numbered_outputs(&exec.pre, "kout", &mut control_outputs);
                if let Some(sample) = &exec.sample {
                    collect_numbered_outputs(&sample.body, "out", &mut audio_outputs);
                }
                collect_numbered_outputs(&exec.post, "kout", &mut control_outputs);
            }
            Block::Sample(sample) => {
                collect_numbered_outputs(&sample.body, "out", &mut audio_outputs)
            }
            _ => {}
        }
    }
    let neutral_audio_outputs = collect_neutral_outputs(audio_outputs, audio_output_decls);
    let mut unavailable = collect_neutral_outputs(control_outputs, control_output_decls);
    unavailable.push(assign_var(TASK_AVAILABLE_FIELD, Expr::bool(false)));
    unavailable.push(abort_activation_stmt());
    let task_names = expansions.keys().cloned().collect::<HashSet<_>>();

    for block in &mut program.blocks {
        match block {
            Block::Init(init) => {
                rewrite_inline_task_controls(
                    &mut init.body,
                    &expansions,
                    &task_names,
                    &unavailable,
                );
            }
            Block::Block(exec) => {
                rewrite_inline_task_controls(&mut exec.pre, &expansions, &task_names, &unavailable);
                propagate_task_abort_through_loops(&mut exec.pre);
                let mut body = vec![assign_var(TASK_AVAILABLE_FIELD, Expr::bool(true))];
                body.extend(scratch_declarations.clone());
                body.append(&mut exec.pre);
                exec.pre = body;
                if let Some(sample) = &mut exec.sample {
                    let original = std::mem::take(&mut sample.body);
                    sample.body = vec![Stmt::If {
                        loc: Default::default(),
                        cond: Expr::var(TASK_AVAILABLE_FIELD),
                        then_branch: original,
                        else_branch: neutral_audio_outputs.clone(),
                    }];
                }
                let original_post = std::mem::take(&mut exec.post);
                exec.post = vec![Stmt::If {
                    loc: Default::default(),
                    cond: Expr::var(TASK_AVAILABLE_FIELD),
                    then_branch: original_post,
                    else_branch: Vec::new(),
                }];
            }
            Block::Sample(sample) => {
                let original = std::mem::take(&mut sample.body);
                sample.body = vec![Stmt::If {
                    loc: Default::default(),
                    cond: Expr::var(TASK_AVAILABLE_FIELD),
                    then_branch: original,
                    else_branch: neutral_audio_outputs.clone(),
                }];
            }
            Block::Events(events) => {
                for event in &mut events.events {
                    rewrite_inline_task_controls(
                        &mut event.body,
                        &expansions,
                        &task_names,
                        &unavailable,
                    );
                }
            }
            _ => {}
        }
    }

    let init = if let Some(init) = program.blocks.iter_mut().find_map(|block| match block {
        Block::Init(init) => Some(init),
        _ => None,
    }) {
        init
    } else {
        program.blocks.push(Block::Init(InitBlock {
            loc: Default::default(),
            default_ty: None,
            default_ty_loc: Default::default(),
            pinned_roots: Vec::new(),
            body: Vec::new(),
        }));
        match program.blocks.last_mut() {
            Some(Block::Init(init)) => init,
            _ => unreachable!("the inserted top-level init block must remain last"),
        }
    };
    let mut old_init = std::mem::take(&mut init.body);
    init_prefix.append(&mut old_init);
    init.body = init_prefix;
    init.pinned_roots.extend(pinned_fields);
}

fn task_callable_defs(program: &Program) -> Vec<FunctionDef> {
    let mut defs = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Def(def) => Some(def.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for struct_def in program.blocks.iter().filter_map(|block| match block {
        Block::Struct(def) => Some(def),
        _ => None,
    }) {
        defs.extend(struct_def.methods.iter().cloned().map(|mut method| {
            method.name = format!("{}.{}", struct_def.name, method.name);
            if let Some(self_param) = method
                .params
                .first_mut()
                .filter(|param| param.name == "self")
            {
                self_param.ty = Some(FnParamType::Struct(struct_def.name.clone()));
            }
            method
        }));
    }
    defs
}

struct TaskCallSemantics {
    overloads: HashMap<String, Vec<crate::def_semantics::OverloadCandidate>>,
    callable_symbols: HashSet<String>,
    signatures: HashMap<String, FnSignature>,
    return_types: HashMap<String, ReturnType>,
}

fn desugar_task_callable_methods(
    defs: &mut [FunctionDef],
    env: &crate::def_semantics::CallTypeEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    callable_symbols: &HashSet<String>,
) {
    for def in defs {
        let mut struct_instances = env.struct_instances.clone();
        let mut struct_array_roots = HashMap::new();
        for param in &def.params {
            if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                register_struct_instance_and_array_roots(
                    &param.name,
                    struct_name,
                    struct_defs,
                    &mut struct_instances,
                    &mut struct_array_roots,
                );
            }
        }
        let current_ns = namespace_of_symbol(&def.name);
        for stmt in &mut def.body {
            crate::proc_call_rewrite::desugar_init_instance_method_calls(
                stmt,
                &mut struct_instances,
                &mut struct_array_roots,
                struct_defs,
                &current_ns,
                callable_symbols,
            );
        }
    }
}

fn resolve_task_callable_return_types(
    defs: &mut [FunctionDef],
    overloads: &HashMap<String, Vec<crate::def_semantics::OverloadCandidate>>,
    env: &crate::def_semantics::CallTypeEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    seed: &HashMap<String, ReturnType>,
) -> HashMap<String, ReturnType> {
    let signatures = defs
        .iter()
        .map(|def| (def.name.clone(), FnSignature::from_def(def)))
        .collect::<HashMap<_, _>>();
    let mut return_types = seed.clone();
    let mut ignored_errors = Vec::new();

    for _ in 0..=defs.len() {
        let inferred = crate::def_semantics::infer_known_def_return_types_with_seed(
            defs,
            &signatures,
            env,
            struct_defs,
            seed,
        );
        let returns_changed = inferred != return_types;
        return_types = inferred;

        let context = crate::def_semantics::CallTypeContext {
            return_types: &return_types,
            struct_defs,
        };
        let resolved = defs
            .iter_mut()
            .map(|def| {
                crate::def_semantics::rewrite_overloaded_calls_in_function(
                    def,
                    env,
                    context,
                    crate::def_semantics::OverloadOwnerContext {
                        defer_dependent_calls: true,
                    },
                    overloads,
                    &mut ignored_errors,
                )
            })
            .sum::<usize>();
        ignored_errors.clear();
        if resolved == 0 && !returns_changed {
            break;
        }
    }

    crate::def_semantics::infer_known_def_return_types_with_seed(
        defs,
        &signatures,
        env,
        struct_defs,
        seed,
    )
}

fn task_call_semantics(
    program: &Program,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> TaskCallSemantics {
    let mut defs = task_callable_defs(program);
    let callable_symbols = defs
        .iter()
        .map(|def| def.name.clone())
        .collect::<HashSet<_>>();
    desugar_task_callable_methods(
        &mut defs,
        &crate::def_semantics::CallTypeEnv::default(),
        struct_defs,
        &callable_symbols,
    );
    let (overloads, _) = crate::def_semantics::prepare_function_overloads(&mut defs);
    let return_types = resolve_task_callable_return_types(
        &mut defs,
        &overloads,
        &crate::def_semantics::CallTypeEnv::default(),
        struct_defs,
        &HashMap::new(),
    );
    let signatures = defs
        .iter()
        .map(|def| (def.name.clone(), FnSignature::from_def(def)))
        .collect();
    TaskCallSemantics {
        overloads,
        callable_symbols,
        signatures,
        return_types,
    }
}

fn proc_task_call_semantics(
    local_defs: &[FunctionDef],
    global: &TaskCallSemantics,
    env: &crate::def_semantics::CallTypeEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> TaskCallSemantics {
    let local_names = local_defs
        .iter()
        .map(|def| def.name.as_str())
        .collect::<HashSet<_>>();
    let overloads = global
        .overloads
        .iter()
        .filter(|(name, _)| !local_names.contains(name.as_str()))
        .map(|(name, candidates)| (name.clone(), candidates.clone()))
        .collect::<HashMap<_, _>>();
    let mut defs = local_defs.to_vec();
    let callable_symbols = global
        .callable_symbols
        .iter()
        .cloned()
        .chain(local_defs.iter().map(|def| def.name.clone()))
        .collect::<HashSet<_>>();
    desugar_task_callable_methods(&mut defs, env, struct_defs, &callable_symbols);
    let return_types = resolve_task_callable_return_types(
        &mut defs,
        &overloads,
        env,
        struct_defs,
        &global.return_types,
    );
    let mut signatures = global.signatures.clone();
    signatures.retain(|name, _| !local_names.contains(name.as_str()));
    signatures.extend(
        defs.iter()
            .map(|def| (def.name.clone(), FnSignature::from_def(def))),
    );
    TaskCallSemantics {
        overloads,
        callable_symbols,
        signatures,
        return_types,
    }
}

fn task_call_type_env(
    owner: &TaskOwnerSurface,
    owner_types: &TaskOwnerTypes,
) -> crate::def_semantics::CallTypeEnv {
    let mut env = crate::def_semantics::CallTypeEnv::default();
    env.scalar_types.clone_from(&owner_types.scalars);
    env.struct_instances
        .clone_from(&owner_types.struct_instances);
    env.tuple_elem_types.clone_from(&owner_types.tuples);
    env.array_types
        .extend(owner_types.indexed.iter().map(|(name, ty)| {
            (
                name.clone(),
                crate::def_semantics::CallArrayType::primitive(*ty, None),
            )
        }));
    for decl in owner.ins.iter().chain(&owner.outs) {
        if let Some(DeclType::Array {
            elem,
            size: Expr::Int { value, .. },
        }) = &decl.ty
        {
            if let Ok(len) = usize::try_from(*value) {
                env.array_types.insert(
                    decl.name.clone(),
                    crate::def_semantics::CallArrayType::primitive(*elem, Some(len)),
                );
            }
        }
    }
    for decl in &owner.params {
        if let Some(DeclType::Array {
            elem,
            size: Expr::Int { value, .. },
        }) = &decl.ty
        {
            if let Ok(len) = usize::try_from(*value) {
                env.array_types.insert(
                    decl.name.clone(),
                    crate::def_semantics::CallArrayType::primitive(*elem, Some(len)),
                );
            }
        }
    }
    for decl in &owner.buffers {
        let Some(BufferType {
            elem: BufferElemType::Primitive(elem),
            channels,
        }) = &decl.ty
        else {
            continue;
        };
        let channels = match channels {
            BufferChannels::Mono => TypedBufferChannels::Mono,
            BufferChannels::Static(Expr::Int { value, .. }) => usize::try_from(*value)
                .ok()
                .filter(|value| *value > 0)
                .map(TypedBufferChannels::Static)
                .unwrap_or(TypedBufferChannels::Dynamic),
            BufferChannels::Static(_) | BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
        };
        env.buffer_types
            .insert(decl.name.clone(), (*elem, channels));
        if let Some(Expr::Int { value, .. }) = &decl.array_size {
            if let Ok(len) = usize::try_from(*value) {
                env.buffer_array_lens.insert(decl.name.clone(), len);
            }
        }
    }
    env
}

fn rewrite_task_overloads(
    task: &mut TaskDef,
    env: &crate::def_semantics::CallTypeEnv,
    semantics: &TaskCallSemantics,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    let mut struct_instances = env.struct_instances.clone();
    let mut struct_array_roots = HashMap::new();
    for stmt in &mut task.body {
        crate::proc_call_rewrite::desugar_init_instance_method_calls(
            stmt,
            &mut struct_instances,
            &mut struct_array_roots,
            struct_defs,
            "",
            &semantics.callable_symbols,
        );
    }
    let mut env = env.clone();
    let mut ignored_errors = Vec::new();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut task.body,
        &mut env,
        crate::def_semantics::CallTypeContext {
            return_types: &semantics.return_types,
            struct_defs,
        },
        crate::def_semantics::OverloadOwnerContext {
            defer_dependent_calls: true,
        },
        &semantics.overloads,
        &mut ignored_errors,
    );
}

pub(crate) fn lower_tasks(
    program: &mut Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let has_source_tasks = program.blocks.iter().any(|block| match block {
        Block::Tasks(tasks) => !tasks.tasks.is_empty(),
        Block::Proc(proc) => !proc.tasks.is_empty(),
        _ => false,
    });
    if !has_source_tasks {
        return;
    }

    let raw_struct_defs = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Struct(def) => Some((def.name.clone(), def.clone())),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let struct_defs = coerce_struct_defs_for_inference(&raw_struct_defs, options);
    let call_semantics = task_call_semantics(program, &struct_defs);

    let top_level_tasks = take_top_level_tasks(program);
    lower_top_level_tasks(
        program,
        &top_level_tasks,
        &call_semantics,
        &struct_defs,
        options,
        errors,
    );

    let mut has_tasks = !top_level_tasks.is_empty();
    for proc in program.blocks.iter_mut().filter_map(|block| match block {
        Block::Proc(proc) => Some(proc),
        _ => None,
    }) {
        if proc.tasks.is_empty() {
            continue;
        }
        has_tasks = true;
        let owner_surface = TaskOwnerSurface::from_proc(proc);
        let preliminary_owner_types =
            collect_task_owner_types(&owner_surface, &call_semantics.return_types, &struct_defs);
        let preliminary_call_env = task_call_type_env(&owner_surface, &preliminary_owner_types);
        let proc_call_semantics = proc_task_call_semantics(
            &proc.local_defs,
            &call_semantics,
            &preliminary_call_env,
            &struct_defs,
        );
        let owner_types = collect_task_owner_types(
            &owner_surface,
            &proc_call_semantics.return_types,
            &struct_defs,
        );
        let call_env = task_call_type_env(&owner_surface, &owner_types);
        let owner_roots = collect_owner_roots(&owner_surface);
        let buffer_params = task_buffer_params(&owner_surface, errors);
        let buffer_names = buffer_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>();
        let task_names = proc
            .tasks
            .iter()
            .map(|task| task.name.clone())
            .collect::<HashSet<_>>();
        let mut outputs = proc
            .outs
            .iter()
            .map(|decl| decl.name.clone())
            .collect::<HashSet<_>>();
        let output_prefix = match proc.outs_timing {
            OutputTiming::Sample => "out",
            OutputTiming::Block => "kout",
        };
        collect_numbered_outputs(&proc.block_pre, output_prefix, &mut outputs);
        collect_numbered_outputs(&proc.sample, output_prefix, &mut outputs);
        let mut outputs = outputs.into_iter().collect::<Vec<_>>();
        outputs.sort();

        let mut init_prefix = vec![typed_assign(
            TASK_AVAILABLE_FIELD,
            PrimitiveType::Bool,
            Expr::bool(true),
        )];
        let mut pinned_task_fields = Vec::new();
        let mut generated_defs = Vec::new();
        for task in &proc.tasks {
            let mut task = task.clone();
            rewrite_task_overloads(&mut task, &call_env, &proc_call_semantics, &struct_defs);
            let prepared = prepare_task(
                &task,
                &owner_roots,
                &proc_call_semantics.return_types,
                &proc_call_semantics.signatures,
                &owner_types,
                &struct_defs,
                options,
                TaskResumePlacement::HelperFunction,
                errors,
            );
            init_prefix.extend(prepared.init_stmts);
            pinned_task_fields.extend(prepared.pinned_fields);
            let for_frame_bindings = prepared.for_frame_bindings.clone();
            generated_defs.push(compile_task_resume(
                &prepared.name,
                &prepared.body,
                prepared.resume_local_initializers,
                buffer_params.clone(),
                &for_frame_bindings,
            ));
            generated_defs.push(FunctionDef {
                loc: Default::default(),
                is_const: false,
                type_params: Vec::new(),
                name: task_reset_def(&prepared.name),
                params: Vec::new(),
                return_ty: None,
                return_ty_loc: Default::default(),
                body: prepared.reset_stmts,
            });
        }

        let neutral_outputs = collect_neutral_outputs(outputs, proc.outs.iter().cloned());
        let mut unavailable = if proc.outs_timing == OutputTiming::Block {
            neutral_outputs.clone()
        } else {
            Vec::new()
        };
        unavailable.push(assign_var(TASK_AVAILABLE_FIELD, Expr::bool(false)));
        unavailable.push(abort_activation_stmt());

        rewrite_task_controls(
            &mut proc.init.body,
            &task_names,
            &buffer_names,
            &unavailable,
        );
        rewrite_task_controls(
            &mut proc.block_pre,
            &task_names,
            &buffer_names,
            &unavailable,
        );
        for event in &mut proc.events {
            rewrite_task_controls(&mut event.body, &task_names, &buffer_names, &unavailable);
        }
        let mut old_init = std::mem::take(&mut proc.init.body);
        init_prefix.append(&mut old_init);
        proc.init.body = init_prefix;
        proc.block_pre
            .insert(0, assign_var(TASK_AVAILABLE_FIELD, Expr::bool(true)));

        let mut unavailable_sample = if proc.outs_timing == OutputTiming::Sample {
            neutral_outputs
        } else {
            Vec::new()
        };
        unavailable_sample.push(abort_activation_stmt());
        proc.sample.insert(
            0,
            Stmt::If {
                loc: Default::default(),
                cond: Expr::UnaryNot {
                    loc: Default::default(),
                    expr: Box::new(Expr::var(TASK_AVAILABLE_FIELD)),
                },
                then_branch: unavailable_sample,
                else_branch: Vec::new(),
            },
        );
        proc.block_post.insert(
            0,
            Stmt::If {
                loc: Default::default(),
                cond: Expr::UnaryNot {
                    loc: Default::default(),
                    expr: Box::new(Expr::var(TASK_AVAILABLE_FIELD)),
                },
                then_branch: vec![abort_activation_stmt()],
                else_branch: Vec::new(),
            },
        );
        proc.local_defs.extend(generated_defs);
        proc.init.pinned_roots.extend(pinned_task_fields);
        proc.tasks.clear();
    }
    if has_tasks {
        program.blocks.push(Block::Def(FunctionDef {
            loc: Default::default(),
            is_const: false,
            type_params: Vec::new(),
            name: TASK_ABORT_FN.to_owned(),
            params: Vec::new(),
            return_ty: None,
            return_ty_loc: Default::default(),
            body: Vec::new(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze, lower_program_to_optimized_mir};
    use onda_frontend::parse_program;

    fn validate(source: &str) -> Vec<Diagnostic> {
        let program = parse_program(source).expect("task source should parse");
        let mut errors = Vec::new();
        validate_task_source_model(&program, &mut errors);
        errors
    }

    fn dispatch_depth(stmts: &[Stmt]) -> usize {
        stmts
            .iter()
            .map(|stmt| match stmt {
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => 1 + dispatch_depth(then_branch).max(dispatch_depth(else_branch)),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn task_dispatch_is_balanced() {
        let arms = (0..64)
            .map(|id| vec![assign_var("selected", Expr::int(id))])
            .collect();
        let dispatch = build_task_dispatch(arms, 0, TASK_NODE_LOCAL);
        assert_eq!(dispatch_depth(&dispatch), 6);
    }

    #[test]
    fn accepts_well_placed_task_controls() {
        let errors = validate(
            "proc P:\n  tasks:\n    load():\n      yield\n      return\n  event restart():\n    load.reset()\n  block:\n    await load()\n    sample:\n      out1 = 0.0\n",
        );
        assert!(errors.is_empty(), "unexpected diagnostics: {errors:?}");
    }

    #[test]
    fn accepts_well_placed_top_level_task_controls() {
        let errors = validate(
            "task load():\n  yield\n  return\nevent restart():\n  load.reset()\nblock:\n  await load()\n  sample:\n    out1 = 0.0\n",
        );
        assert!(errors.is_empty(), "unexpected diagnostics: {errors:?}");
    }

    #[test]
    fn task_locals_use_shared_expression_typing() {
        let source = r#"
struct Counter:
  value: i32 = 7

  def read(self) -> i32:
    return self.value

buffers:
  impulse: f32[2]

def clamp_count(value: i32) -> i32:
  return max(value, 1)

init:
  counter = Counter()

task load():
  frames = min(impulse.len(), 480000)
  count = clamp_count(frames)
  field_value = counter.value
  method_value = counter.read()
  yield
  count = count + field_value + method_value

event reload():
  load.reset()

block:
  await load()
  sample:
    out1 = 0.0
"#;

        crate::analyze(onda_frontend::parse_program(source).expect("source should parse"))
            .expect("task locals should follow ordinary expression typing rules");
    }

    #[test]
    fn executable_scopes_share_scalar_inference() {
        let source = r#"
struct Counter:
  value: i32 = 7

  def read(self) -> i32:
    return self.value

def require_i32(value: i32) -> i32:
  return value

def read_counter(counter: Counter) -> i32:
  value = counter.read()
  return require_i32(value)

proc Reader:
  init:
    counter = Counter()
    init_value = counter.read()
    init_checked = require_i32(init_value)

  task prepare():
    task_value = counter.read()
    task_checked = require_i32(task_value)
    yield

  event restart():
    event_value = counter.read()
    event_checked = require_i32(event_value)
    prepare.reset()

  block:
    block_value = counter.read()
    block_checked = require_i32(block_value)
    await prepare()

    sample:
      sample_value = counter.read()
      out1 = f32(require_i32(sample_value) + read_counter(counter))

init:
  reader = Reader()

sample:
  out1 = reader()
"#;

        crate::analyze(onda_frontend::parse_program(source).expect("source should parse"))
            .expect("all executable scopes should share scalar inference");
    }

    #[test]
    fn rejects_invalid_task_control_placement_and_targets() {
        let cases = [
            (
                "proc P:\n  init:\n    yield\n  sample:\n    out1 = 0.0\n",
                "yield is only allowed inside a task body",
            ),
            (
                "proc P:\n  task load():\n    return\n  sample:\n    await load()\n    out1 = 0.0\n",
                "await is only allowed",
            ),
            (
                "proc P:\n  task load():\n    return\n  block:\n    await missing()\n    sample:\n      out1 = 0.0\n",
                "unknown task 'missing'",
            ),
            (
                "proc P:\n  task load():\n    return 1.0\n  sample:\n    out1 = 0.0\n",
                "tasks cannot return a value",
            ),
            (
                "proc P:\n  task load():\n    return\n  sample:\n    load.reset()\n    out1 = 0.0\n",
                "can only be reset from init, event, or block-pre",
            ),
        ];

        for (source, expected) in cases {
            let errors = validate(source);
            assert!(
                errors.iter().any(|error| error.message.contains(expected)),
                "missing '{expected}' in {errors:?}"
            );
        }
    }

    #[test]
    fn rejects_task_member_conflicts_and_graph_owners() {
        let conflicts = validate(
            "proc P:\n  task load():\n    return\n  def load():\n    return\n  sample:\n    out1 = 0.0\n",
        );
        assert!(conflicts
            .iter()
            .any(|error| error.message.contains("conflicts with proc-local def")));

        let proc_const_conflict = validate(
            "proc P:\n  const load = 1\n  task load():\n    return\n  sample:\n    out1 = 0.0\n",
        );
        assert!(proc_const_conflict
            .iter()
            .any(|error| error.message.contains("conflicts with constant")));

        let proc_state_conflict = validate(
            "proc P:\n  init:\n    load: i32 = 0\n  task load():\n    return\n  sample:\n    out1 = 0.0\n",
        );
        assert!(proc_state_conflict
            .iter()
            .any(|error| error.message.contains("conflicts with state root")));

        let graph =
            validate("proc P:\n  task load():\n    return\n  graph:\n    source() >> out1\n");
        assert!(graph
            .iter()
            .any(|error| error.message.contains("tasks together with a graph block")));

        let top_conflict =
            validate("init:\n  load: i32 = 0\ntask load():\n  return\nsample:\n  out1 = 0.0\n");
        assert!(top_conflict
            .iter()
            .any(|error| error.message.contains("conflicts with state root")));

        let top_graph = validate("task load():\n  return\ngraph:\n  source() >> out1\n");
        assert!(top_graph.iter().any(|error| error
            .message
            .contains("tasks cannot be declared together with a graph")));
    }

    #[test]
    fn rejects_proc_task_constant_conflicts_before_folding_await_markers() {
        let source = r#"
proc P:
  const prepare = 1
  task prepare():
    yield
  block:
    await prepare()
    sample:
      out1 = 0.0
"#;
        let errors = analyze(parse_program(source).expect("task source should parse"))
            .expect_err("task and constant names should conflict");
        assert!(errors.iter().any(|error| error
            .message
            .contains("task 'prepare' in processor 'P' conflicts with local constant")));
        assert!(errors.iter().all(|error| !error
            .message
            .contains("malformed internal task await marker")));
    }

    #[test]
    fn rejects_task_io_and_processor_calls() {
        let errors = validate(
            r#"
proc Child:
  sample:
    out1 = in1
proc Owner:
  init:
    child = Child()
    children: Child[2] = Child()
  task load():
    value = in1
    out1 = value
    child()
    children[0]()
  block:
    await load()
    sample:
      out1 = 0.0
"#,
        );
        for expected in [
            "cannot read audio input 'in1'",
            "cannot write processor outputs",
            "cannot call processor 'child'",
            "cannot call an indexed processor",
        ] {
            assert!(
                errors.iter().any(|error| error.message.contains(expected)),
                "missing '{expected}' in {errors:?}"
            );
        }
    }

    #[test]
    fn permits_child_proc_events_in_tasks() {
        let source = r#"
proc Child:
  init:
    value: i32 = 0
  event add(amount: i32):
    value += amount
  sample:
    out1 = f32(value)

proc Owner:
  init:
    child = Child()
    children: Child[2] = Child()
  task prepare():
    child.add(1)
    index: i32 = 1
    children[index].add(2)
    child.init()
  block:
    await prepare()
    sample:
      out1 = child() + children[1]()

init:
  owner = Owner()
sample:
  out1 = owner()
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("child proc events in tasks should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("child proc events in tasks should lower to valid MIR");
    }

    #[test]
    fn permits_child_proc_events_in_top_level_tasks() {
        let source = r#"
proc Child:
  init:
    value: i32 = 0
  event add(amount: i32):
    value += amount
  sample:
    out1 = f32(value)

init:
  child = Child()

task prepare():
  child.add(2)
  yield
  child.add(3)

block:
  await prepare()
  sample:
    out1 = child()
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("child proc events in top-level tasks should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("child proc events in top-level tasks should lower to valid MIR");
    }

    #[test]
    fn permits_tasks_to_observe_and_write_resettable_state() {
        let source = r#"
proc Loader:
  init:
    progress: i32 = 0
  task load():
    progress += 1
    yield
  block:
    await load()
    sample:
      out1 = 0.0
"#;
        let parsed = parse_program(source).expect("task source should parse");
        analyze(parsed).expect("resettable task state should be accepted");
    }

    #[test]
    fn lowers_tasks_to_executable_runtime_defs() {
        let source = r#"
proc Loader:
  init:
    pin progress: i32 = 0
  task load():
    progress += 1
    yield
    progress += 1

  block:
    await load()

    sample:
      out1 = f32(progress)

init:
  loader = Loader()

sample:
  out1 = loader()
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("task source should lower and analyze");
        assert!(typed
            .defs
            .iter()
            .any(|def| def.name.contains(&task_resume_def("load"))));
        assert!(typed
            .state_vars
            .iter()
            .any(|name| name.contains(&task_pc_field("load"))));
        let mir =
            lower_program_to_optimized_mir(&typed).expect("lowered task should produce valid MIR");
        assert!(mir
            .state
            .iter()
            .any(|slot| { slot.name.contains(&task_pc_field("load")) && slot.pinned }));
        assert!(mir
            .state
            .iter()
            .any(|slot| { slot.name == "loader.progress" && slot.pinned }));
    }

    #[test]
    fn task_frames_only_store_locals_live_across_yield() {
        let source = r#"
proc Loader:
  init:
    pin result: i32 = 0
  task load():
    scratch: i32 = 10
    carried: i32 = scratch + 1
    yield
    result = carried
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("task source should analyze");
        assert!(typed
            .state_vars
            .iter()
            .any(|name| name.contains(&task_local_field("load", "carried"))));
        assert!(!typed
            .state_vars
            .iter()
            .any(|name| name.contains(&task_local_field("load", "scratch"))));
    }

    #[test]
    fn task_allows_reference_locals_that_are_dead_before_yield() {
        let source = r#"
buffers:
  data: f32

init:
  pin observed: i32 = 0
task load():
  window = data[:]
  observed = window.len()
  yield

block:
  await load()

sample:
  out1 = f32(observed)
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("a reference local that dies before yield should analyze");
        assert!(!typed
            .state_vars
            .iter()
            .any(|name| name.contains(&task_local_field("load", "window"))));
        lower_program_to_optimized_mir(&typed)
            .expect("the ephemeral reference should remain local to one resume arm");
    }

    #[test]
    fn task_allows_reference_locals_created_after_yield() {
        let source = r#"
buffers:
  data: f32

init:
  pin observed: i32 = 0
task load():
  yield
  window = data[:]
  observed = window.len()

block:
  await load()

sample:
  out1 = f32(observed)
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("a reference local created after yield should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("the post-yield reference should remain within its resume arm");
    }

    #[test]
    fn task_rejects_reference_locals_that_cross_yield() {
        let source = r#"
buffers:
  data: f32

init:
  pin observed: i32 = 0
task load():
  window = data[:]
  yield
  observed = window.len()

block:
  await load()

sample:
  out1 = f32(observed)
"#;
        let errors = analyze(parse_program(source).expect("task source should parse"))
            .expect_err("a reference local cannot be stored in a task frame");
        assert!(
            errors.iter().any(|error| {
                error.message.contains("window") && error.message.contains("live across a yield")
            }),
            "unexpected diagnostics: {errors:?}"
        );
    }

    #[test]
    fn task_loop_frame_names_do_not_alias_user_locals() {
        let source = r#"
init:
  pin result: i32 = 0
task prepare():
  i__end: i32 = 99
  for i in 0..2:
    yield
  result = i__end

block:
  await prepare()

sample:
  out1 = f32(result)
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("loop bookkeeping should use collision-free names");
        assert!(typed
            .state_vars
            .iter()
            .any(|name| name.contains(&task_local_field("prepare", "i__end"))));
        assert!(typed
            .state_vars
            .iter()
            .any(|name| name.contains(&format!("{}_for_0_end", task_symbol_stem("prepare")))));
        lower_program_to_optimized_mir(&typed)
            .expect("distinct user and loop frame fields should lower");
    }

    #[test]
    fn large_task_scratch_array_uses_one_body_fill() {
        let source = r#"
task prepare():
  scratch: f32[4096]
  scratch[0] = 1.0

block:
  await prepare()

sample:
  out1 = 0.0
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("large task scratch array should analyze");
        let mir =
            lower_program_to_optimized_mir(&typed).expect("large task scratch array should lower");
        let dump = onda_mir::format_program(mir.as_program());
        assert_eq!(
            dump.matches("slice_fill").count(),
            1,
            "task initialization should remain one operation rather than one CFG node per element"
        );
        let scratch_local = dump
            .lines()
            .find(|line| {
                line.contains("prepare_scratch") && line.trim_start().starts_with("local ")
            })
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("scratch array MIR local");
        assert_eq!(
            dump.matches(&format!("{scratch_local}[")).count(),
            1,
            "declaration-only scratch storage must not emit an unrolled zero store per element"
        );
    }

    #[test]
    fn task_branch_bindings_use_canonical_shape_compatibility() {
        let source = r#"
params:
  choose: bool = false
init:
  pin result = 0.0
task prepare():
  if choose:
    carried: f32[2] = [1.0, 2.0]
  else:
    carried: f32[3] = [3.0, 4.0, 5.0]
  yield
  result = carried[0]
block:
  await prepare()
  sample:
    out1 = result
"#;

        let errors = analyze(parse_program(source).expect("task source should parse"))
            .expect_err("different fixed array shapes must not join across task branches");
        assert!(
            errors.iter().any(|error| {
                error.message
                    == "binding 'carried' has incompatible branch types: arrays have different element types or fixed lengths"
            }),
            "unexpected diagnostics: {errors:?}"
        );
    }

    #[test]
    fn initialized_task_array_frame_preserves_its_values() {
        let source = r#"
proc Loader:
  init:
    pin result: i32 = 0
  task load():
    values: i32[2] = [3, 5]
    yield
    result = values[0] + values[1] + values.len()
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("task source should analyze");
        let mir =
            lower_program_to_optimized_mir(&typed).expect("array task should produce valid MIR");
        let dump = onda_mir::format_program(mir.as_program());
        assert!(dump.contains("i32(3)"));
        assert!(dump.contains("i32(5)"));
    }

    #[test]
    fn task_bodies_receive_proc_and_lexical_constant_folding() {
        let source = r#"
proc Loader:
  const Width = 2
  init:
    pin result: i32 = 0
  task load():
    const Left = 3
    values: i32[Width] = [Left, 5]
    yield
    result = values[0] + values[1]
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("task constants should fold before lowering");
        lower_program_to_optimized_mir(&typed)
            .expect("task constants should not survive into runtime MIR");
    }

    #[test]
    fn task_frame_types_infer_from_owner_bindings() {
        let source = r#"
proc Loader:
  params:
    parameter: i32 = 7
  buffers:
    samples: f32
  init:
    scalar: i64 = 3
    values: i32[2] = [5, 11]
    pin result = 0.0
  task load():
    scalar_copy = scalar
    array_copy = values[0]
    parameter_copy = parameter
    buffer_copy = samples[0]
    yield
    result = f32(scalar_copy + array_copy + parameter_copy) + buffer_copy
  block:
    await load()
    sample:
      out1 = result
init:
  loader = Loader(parameter = 7, samples = samples)
buffers:
  samples: f32
sample:
  out1 = loader()
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("task locals should infer from owner binding types");
        for local in ["scalar_copy", "array_copy", "parameter_copy", "buffer_copy"] {
            assert!(
                typed
                    .state_vars
                    .iter()
                    .any(|name| name.contains(&task_local_field("load", local))),
                "missing frame storage for {local}"
            );
        }
        lower_program_to_optimized_mir(&typed)
            .expect("task buffer parameters and inferred frame types should lower");
    }

    #[test]
    fn task_aggregate_fields_remain_owner_state_and_inherit_pinning() {
        let source = r#"
struct Accumulator:
  value: i32 = 0

proc Loader:
  init:
    pin accumulator = Accumulator()
  task load():
    accumulator.value += 1
    yield
    accumulator.value += 1
  block:
    await load()
    sample:
      out1 = f32(accumulator.value)

init:
  loader = Loader()
  pin top = Accumulator()
sample:
  out1 = loader() + f32(top.value)
"#;
        let typed = analyze(parse_program(source).expect("aggregate task source should parse"))
            .expect("aggregate task state should analyze");
        assert!(!typed
            .state_vars
            .iter()
            .any(|name| name.contains(&task_local_field("load", "accumulator"))));
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("aggregate task state should lower to valid MIR");
        for name in ["loader.accumulator.value", "top.value"] {
            let slot = mir
                .state
                .iter()
                .find(|slot| slot.name == name)
                .unwrap_or_else(|| panic!("missing flattened state slot {name}"));
            assert!(slot.pinned);
        }
    }

    #[test]
    fn top_level_task_aggregate_fields_remain_owner_state() {
        let source = r#"
struct Accumulator:
  value: i32 = 0

init:
  pin accumulator = Accumulator()
task load():
  accumulator.value += 1
  yield
  accumulator.value += 1
block:
  await load()
  sample:
    out1 = f32(accumulator.value)
"#;
        let typed = analyze(parse_program(source).expect("aggregate task source should parse"))
            .expect("top-level aggregate task state should analyze");
        assert!(!typed
            .state_vars
            .iter()
            .any(|name| name.contains(&task_local_field("load", "accumulator"))));
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("top-level aggregate task state should lower");
        let accumulator = mir
            .state
            .iter()
            .find(|slot| slot.name == "accumulator.value")
            .expect("flattened accumulator state");
        assert!(accumulator.pinned);
    }

    #[test]
    fn proc_task_supports_non_frame_array_locals() {
        let source = r#"
proc Loader:
  init:
    pin result: i32 = 0
  task load():
    values: i32[2] = [3, 5]
    result = values[0] + values[1]
    yield
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
        let typed = analyze(parse_program(source).expect("array task source should parse"))
            .expect("non-frame task array should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("non-frame task array should lower to valid MIR");
    }

    #[test]
    fn tuple_destructured_task_locals_can_cross_yield() {
        let source = r#"
def pair() -> (i32, i32):
  return (3, 5)

proc Loader:
  init:
    pin result: i32 = 0
  task load():
    (left, right) = pair()
    yield
    result = left + right
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
        let typed = analyze(parse_program(source).expect("tuple task source should parse"))
            .expect("tuple task locals should analyze");
        for local in ["left", "right"] {
            assert!(typed
                .state_vars
                .iter()
                .any(|name| name.contains(&task_local_field("load", local))));
        }
        lower_program_to_optimized_mir(&typed)
            .expect("tuple task locals should lower to valid MIR");
    }

    #[test]
    fn fixed_tuple_task_local_can_cross_yield() {
        let source = r#"
init:
  pin result: f32 = 0.0

task prepare():
  pair = (i32(3), i64(5))
  yield
  result = f32(pair[0]) + f32(pair[1])

block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("a fixed tuple task local should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("a fixed tuple task local should lower to valid MIR");
        let tuple_fields = mir
            .as_program()
            .state
            .iter()
            .filter(|slot| slot.name.contains("prepare_local") && slot.name.contains("pair.__"))
            .collect::<Vec<_>>();
        assert_eq!(tuple_fields.len(), 2);
        assert!(tuple_fields
            .iter()
            .all(|slot| slot.pinned && !slot.authored));
    }

    #[test]
    fn task_for_bounds_use_the_language_induction_coercion() {
        let source = r#"
init:
  pin result: i32 = 0

task prepare():
  for i in (i64(0))..(i64(2)):
    result += i
    yield

block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("task for bounds should use ordinary loop coercion");
        lower_program_to_optimized_mir(&typed)
            .expect("coerced task for bounds should lower to valid MIR");
    }

    #[test]
    fn task_barriers_neutralize_outputs_by_declared_type_and_shape() {
        let sources = [
            r#"
const Channels = 2

outs:
  ready: bool
  stereo: f32[Channels]

task prepare():
  yield

block:
  await prepare()
  sample:
    ready = true
    stereo[0] = 1.0
    stereo[1] = 2.0
"#,
            r#"
proc Loader:
  outs:
    stereo: f32[2]
  task prepare():
    yield
  block:
    await prepare()
    sample:
      stereo[0] = 1.0
      stereo[1] = 2.0

outs:
  out1
init:
  loader = Loader()
graph:
  loader.stereo[0] >> out1
"#,
        ];

        for source in sources {
            let typed = analyze(parse_program(source).expect("task source should parse"))
                .expect("typed and array outputs should be neutralized correctly");
            lower_program_to_optimized_mir(&typed)
                .expect("typed and array task outputs should lower to valid MIR");
        }
    }

    #[test]
    fn proc_task_barrier_returns_the_declared_scalar_type() {
        let source = r#"
proc Gate:
  outs:
    out1: bool
  task prepare():
    yield
  block:
    await prepare()
    sample:
      out1 = true

outs:
  out1: bool
init:
  gate = Gate()
sample:
  out1 = gate()
"#;

        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("a task-gated bool processor output should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("a task-gated bool processor output should lower to valid MIR");
    }

    #[test]
    fn task_for_with_early_return_has_complete_cfg_storage() {
        let source = r#"
init:
  pin result: i32 = 0

task prepare():
  for i in 0..4:
    if i == 2:
      return
    result += 1

block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("an early task return inside a for loop should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("an early task return inside a for loop should lower to valid MIR");
    }

    #[test]
    fn task_frame_locals_use_inferred_callable_return_types() {
        let sources = [
            r#"
def value():
  result: i64 = 2
  return result

init:
  pin result: i64 = 0

task prepare():
  carried = value()
  yield
  result = carried

block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
            r#"
def source_value():
  local: i64 = 2
  return local

proc Loader:
  outs:
    out1
  init:
    pin result: i64 = 0
  def value():
    return source_value()
  task prepare():
    carried = value()
    yield
    result = carried
  block:
    await prepare()
    sample:
      out1 = f32(result)

init:
  loader = Loader()
sample:
  out1 = loader()
"#,
            r#"
proc Loader:
  outs:
    out1
  init:
    source: i64 = 2
    pin result: i64 = 0
  def value():
    return source
  task prepare():
    carried = value()
    yield
    result = carried
  block:
    await prepare()
    sample:
      out1 = f32(result)

init:
  loader = Loader()
sample:
  out1 = loader()
"#,
            r#"
struct Counter:
  value: i64 = 2
  def read(self):
    return self.value

init:
  counter = Counter()
  pin result: i64 = 0

task prepare():
  carried = counter.read()
  yield
  result = carried

block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        ];

        for source in sources {
            let typed = analyze(parse_program(source).expect("task source should parse"))
                .expect("task frame storage should use inferred callable returns");
            lower_program_to_optimized_mir(&typed)
                .expect("inferred callable task locals should lower to valid MIR");
        }
    }

    #[test]
    fn proc_task_frame_typing_uses_the_selected_global_overload() {
        let sources = [
            r#"
def value(x: i32) -> i32:
  return x + 1
def value(x: f64) -> f64:
  return x + 2.0
proc Loader:
  outs:
    out1
  init:
    pin result: i32 = 0
  task prepare():
    carried = value(i32(3))
    yield
    result = carried
  block:
    await prepare()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
            r#"
def value(x: f64) -> f64:
  return x + 2.0
def value(x: i32) -> i32:
  return x + 1
proc Loader:
  outs:
    out1
  init:
    pin result: i32 = 0
  task prepare():
    carried = value(i32(3))
    yield
    result = carried
  block:
    await prepare()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        ];

        for source in sources {
            let typed = analyze(parse_program(source).expect("task source should parse"))
                .expect("proc task frame typing should be independent of overload order");
            lower_program_to_optimized_mir(&typed)
                .expect("selected proc task overload should lower to valid MIR");
        }
    }

    #[test]
    fn task_frame_typing_uses_the_selected_method_overload() {
        let source = r#"
struct Calculator:
  def value(self, x: i32) -> i32:
    return x + 1
  def value(self, x: f64) -> f64:
    return x + 2.0
init:
  calculator = Calculator()
  pin result: i32 = 0
task prepare():
  carried = calculator.value(i32(3))
  yield
  result = carried
block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("task frame typing should use ordinary method overload selection");
        lower_program_to_optimized_mir(&typed)
            .expect("selected task method overload should lower to valid MIR");
    }

    #[test]
    fn proc_block_task_barrier_neutralizes_block_timed_outputs() {
        let source = r#"
proc Control:
  kouts 1
  init:
    pin value: i32 = 0
  task load():
    yield
    value += 1
  block:
    await load()
    kout1 = f32(value)

init:
  control = Control()

block:
  kout1 = control().kout1
"#;
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("a task barrier should support block-timed proc outputs");
        lower_program_to_optimized_mir(&typed)
            .expect("block-timed proc task outputs should lower to valid MIR");
    }
}
