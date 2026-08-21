use std::collections::{HashMap, HashSet};

use crate::*;
use onda_frontend::{
    ArrayElemType, ArrayTypeSpec, BinaryOp, CmpOp, FnParamDecl, FnReturnScalarType, FnReturnType,
    TaskDef, INTERNAL_BARE_RETURN_FN, INTERNAL_TASK_AWAIT_FN, INTERNAL_TASK_YIELD_FN,
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
}

impl TaskLocalStorage {
    fn init_stmt(&self, name: String) -> Stmt {
        let (decl_ty, expr) = match self {
            Self::Scalar(ty) => (
                Some(*ty),
                match ty {
                    PrimitiveType::F32 | PrimitiveType::F64 => Expr::number(0.0),
                    PrimitiveType::I32 | PrimitiveType::I64 => Expr::int(0),
                    PrimitiveType::Bool => Expr::bool(false),
                },
            ),
            Self::Array { spec, .. } => (
                None,
                Expr::ArrayCtor {
                    loc: Default::default(),
                    spec: spec.clone(),
                    init: None,
                },
            ),
        };
        Stmt::Assign {
            loc: Default::default(),
            target_loc: Default::default(),
            target: AssignTarget::Var(name),
            decl_ty,
            generic_decl_ty: None,
            is_typed_decl: true,
            typed_decl_ty_loc: Default::default(),
            expr,
        }
    }

    fn reset_stmts(&self, name: String) -> Vec<Stmt> {
        match self {
            Self::Scalar(ty) => vec![assign_var(name, zero_scalar(*ty))],
            Self::Array { element, len, .. } => (0..*len)
                .map(|index| assign_index(name.clone(), index, zero_scalar(*element)))
                .collect(),
        }
    }
}

fn task_pc_field(task: &str) -> String {
    format!("{TASK_FIELD_PREFIX}{task}_pc")
}

fn task_local_field(task: &str, local: &str) -> String {
    format!("{TASK_FIELD_PREFIX}{task}_local_{local}")
}

fn task_resume_def(task: &str) -> String {
    format!("{TASK_FIELD_PREFIX}{task}_resume")
}

fn task_reset_def(task: &str) -> String {
    format!("{TASK_FIELD_PREFIX}{task}_reset")
}

fn task_inline_node_local(task: &str) -> String {
    format!("{TASK_FIELD_PREFIX}{task}_node")
}

fn task_inline_result_local(task: &str) -> String {
    format!("{TASK_FIELD_PREFIX}{task}_result")
}

fn task_inline_scratch_local(task: &str, local: &str) -> String {
    format!("{TASK_FIELD_PREFIX}{task}_scratch_{local}")
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

fn zero_scalar(ty: PrimitiveType) -> Expr {
    match ty {
        PrimitiveType::F32 | PrimitiveType::F64 => Expr::number(0.0),
        PrimitiveType::I32 | PrimitiveType::I64 => Expr::int(0),
        PrimitiveType::Bool => Expr::bool(false),
    }
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
    tuples: HashMap<String, Vec<PrimitiveType>>,
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

fn infer_task_scalar_type(
    expr: &Expr,
    known: &HashMap<String, TaskLocalStorage>,
    owner_types: &TaskOwnerTypes,
    return_types: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { value, .. } => Some(if i32::try_from(*value).is_ok() {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool { .. } | Expr::Compare { .. } | Expr::Logical { .. } | Expr::UnaryNot { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::Cast { to, .. } => Some(*to),
        Expr::Var { name, .. } => match known.get(name) {
            Some(TaskLocalStorage::Scalar(ty)) => Some(*ty),
            _ => owner_types.scalars.get(name).copied(),
        },
        Expr::Index { base, index, .. } => match known.get(base) {
            Some(TaskLocalStorage::Array { element, .. }) => Some(*element),
            _ => owner_types.indexed.get(base).copied().or_else(|| {
                let Expr::Int { value, .. } = index.as_ref() else {
                    return None;
                };
                usize::try_from(*value)
                    .ok()
                    .and_then(|index| owner_types.tuples.get(base)?.get(index).copied())
            }),
        },
        Expr::Binary { lhs, rhs, .. } => {
            let lhs = infer_task_scalar_type(lhs, known, owner_types, return_types);
            let rhs = infer_task_scalar_type(rhs, known, owner_types, return_types);
            match (lhs, rhs) {
                (Some(PrimitiveType::F64), _) | (_, Some(PrimitiveType::F64)) => {
                    Some(PrimitiveType::F64)
                }
                (Some(PrimitiveType::F32), _) | (_, Some(PrimitiveType::F32)) => {
                    Some(PrimitiveType::F32)
                }
                (Some(PrimitiveType::I64), _) | (_, Some(PrimitiveType::I64)) => {
                    Some(PrimitiveType::I64)
                }
                (Some(ty), _) | (_, Some(ty)) => Some(ty),
                _ => None,
            }
        }
        Expr::UnaryBitNot { expr, .. } => {
            infer_task_scalar_type(expr, known, owner_types, return_types)
        }
        Expr::Call { args, .. } => args
            .first()
            .and_then(|arg| infer_task_scalar_type(arg, known, owner_types, return_types)),
        Expr::UserCall { name, .. } => return_types.get(name).copied(),
        Expr::Slice { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::ArrayCtor { .. }
        | Expr::Tuple { .. } => None,
    }
}

fn collect_task_owner_types(
    owner: &TaskOwnerSurface,
    return_types: &HashMap<String, PrimitiveType>,
) -> TaskOwnerTypes {
    fn record_decl(
        types: &mut TaskOwnerTypes,
        name: &str,
        ty: Option<&DeclType>,
        default: Option<&Expr>,
        return_types: &HashMap<String, PrimitiveType>,
    ) {
        if ty.is_some() {
            record_task_owner_decl_type(types, name, ty);
            return;
        }
        let empty_locals = HashMap::new();
        let inferred = default
            .and_then(|expr| infer_task_scalar_type(expr, &empty_locals, types, return_types))
            .unwrap_or(PrimitiveType::F32);
        types.scalars.insert(name.to_owned(), inferred);
    }

    let mut types = TaskOwnerTypes::default();
    for decl in &owner.ins {
        record_decl(
            &mut types,
            &decl.name,
            decl.ty.as_ref(),
            decl.default.as_ref(),
            return_types,
        );
    }
    for decl in &owner.outs {
        record_decl(
            &mut types,
            &decl.name,
            decl.ty.as_ref(),
            decl.default.as_ref(),
            return_types,
        );
    }
    for decl in &owner.params {
        record_decl(
            &mut types,
            &decl.name,
            decl.ty.as_ref(),
            decl.default.as_ref(),
            return_types,
        );
    }
    for decl in &owner.buffers {
        if decl.array_size.is_some() {
            continue;
        }
        if let Some(BufferType {
            elem: BufferElemType::Primitive(element),
            ..
        }) = decl.ty.as_ref()
        {
            types.indexed.insert(decl.name.clone(), *element);
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
        } else if let Expr::ArrayCtor { spec, .. } = expr {
            if let ArrayElemType::Primitive(element) = spec.elem {
                types.indexed.insert(name.clone(), element);
            }
        } else if let Expr::Tuple { values, .. } = expr {
            if let Some(elements) = values
                .iter()
                .map(|value| infer_task_scalar_type(value, &empty_locals, &types, return_types))
                .collect::<Option<Vec<_>>>()
            {
                types.tuples.insert(name.clone(), elements);
            }
        } else if let Some(ty) = infer_task_scalar_type(expr, &empty_locals, &types, return_types) {
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
                if self.owner_roots.contains(source_name) {
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
                        self.visible.clone_from(&outer_visible);
                        self.rewrite_list(else_branch);
                        self.visible = outer_visible;
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

fn collect_task_locals(
    stmts: &[Stmt],
    owner_roots: &HashSet<String>,
    return_types: &HashMap<String, PrimitiveType>,
    owner_types: &TaskOwnerTypes,
    source_names: &HashMap<String, String>,
    task_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, TaskLocalStorage> {
    fn visit(
        stmts: &[Stmt],
        owner_roots: &HashSet<String>,
        return_types: &HashMap<String, PrimitiveType>,
        owner_types: &TaskOwnerTypes,
        source_names: &HashMap<String, String>,
        task_name: &str,
        locals: &mut HashMap<String, TaskLocalStorage>,
        errors: &mut Vec<Diagnostic>,
    ) {
        for stmt in stmts {
            match stmt {
                Stmt::Assign {
                    loc,
                    target: AssignTarget::Var(name),
                    decl_ty,
                    expr,
                    ..
                } if !owner_roots.contains(name) => {
                    if locals.contains_key(name) {
                        continue;
                    }
                    let is_array_ctor = matches!(expr, Expr::ArrayCtor { .. });
                    let storage = if let Expr::ArrayCtor { spec, init, .. } = expr {
                        let element = match spec.elem {
                            ArrayElemType::Primitive(element) => Some(element),
                            ArrayElemType::Struct(_) => None,
                        };
                        let len = match spec.size.as_ref() {
                            Expr::Int { value, .. } => usize::try_from(*value).ok(),
                            _ => None,
                        };
                        match (element, len) {
                            (Some(element), Some(len)) if len > 0 => {
                                if init.as_ref().is_some_and(|values| values.len() != len) {
                                    errors.push(Diagnostic::semantic_span(
                                        format!(
                                            "fixed-array local '{}' in task '{task_name}' expects {len} initializer values",
                                            source_names.get(name).unwrap_or(name)
                                        ),
                                        *loc,
                                    ));
                                }
                                Some(TaskLocalStorage::Array {
                                    spec: spec.clone(),
                                    element,
                                    len,
                                })
                            }
                            _ => {
                                errors.push(Diagnostic::semantic_span(
                                    format!(
                                        "task local '{}' must have fixed primitive storage",
                                        source_names.get(name).unwrap_or(name)
                                    ),
                                    *loc,
                                ));
                                None
                            }
                        }
                    } else {
                        (*decl_ty)
                            .or_else(|| {
                                infer_task_scalar_type(expr, locals, owner_types, return_types)
                            })
                            .map(TaskLocalStorage::Scalar)
                    };
                    if let Some(storage) = storage {
                        locals.insert(name.clone(), storage);
                    } else if !is_array_ctor {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "cannot determine fixed storage type for local '{}' in task '{task_name}'; add an explicit primitive or fixed-array type",
                                source_names.get(name).unwrap_or(name)
                            ),
                            *loc,
                        ));
                    }
                }
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    visit(
                        then_branch,
                        owner_roots,
                        return_types,
                        owner_types,
                        source_names,
                        task_name,
                        locals,
                        errors,
                    );
                    visit(
                        else_branch,
                        owner_roots,
                        return_types,
                        owner_types,
                        source_names,
                        task_name,
                        locals,
                        errors,
                    );
                }
                Stmt::For { var, body, .. } => {
                    locals
                        .entry(var.clone())
                        .or_insert(TaskLocalStorage::Scalar(PrimitiveType::I32));
                    visit(
                        body,
                        owner_roots,
                        return_types,
                        owner_types,
                        source_names,
                        task_name,
                        locals,
                        errors,
                    );
                }
                Stmt::While { body, .. } => visit(
                    body,
                    owner_roots,
                    return_types,
                    owner_types,
                    source_names,
                    task_name,
                    locals,
                    errors,
                ),
                _ => {}
            }
        }
    }

    let mut locals = HashMap::new();
    visit(
        stmts,
        owner_roots,
        return_types,
        owner_types,
        source_names,
        task_name,
        &mut locals,
        errors,
    );
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
        Expr::UserCall { args, .. } => {
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

fn expand_task_array_frame_initializers(
    stmts: &mut Vec<Stmt>,
    frame_arrays: &HashMap<String, (PrimitiveType, usize)>,
) {
    let mut rewritten = Vec::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        let expansion = match &stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::ArrayCtor { init, .. },
                ..
            } => frame_arrays.get(name).map(|(element, len)| {
                if let Some(values) = init {
                    values
                        .iter()
                        .cloned()
                        .enumerate()
                        .map(|(index, value)| assign_index(name.clone(), index, value))
                        .collect::<Vec<_>>()
                } else {
                    (0..*len)
                        .map(|index| assign_index(name.clone(), index, zero_scalar(*element)))
                        .collect()
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
                expand_task_array_frame_initializers(then_branch, frame_arrays);
                expand_task_array_frame_initializers(else_branch, frame_arrays);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                expand_task_array_frame_initializers(body, frame_arrays);
            }
            _ => {}
        }
        rewritten.push(stmt);
    }
    *stmts = rewritten;
}

fn collect_for_frame_fields(stmts: &[Stmt], fields: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_for_frame_fields(then_branch, fields);
                collect_for_frame_fields(else_branch, fields);
            }
            Stmt::For { var, body, .. } => {
                fields.insert(format!("{var}__end"));
                fields.insert(format!("{var}__step"));
                collect_for_frame_fields(body, fields);
            }
            Stmt::While { body, .. } => collect_for_frame_fields(body, fields),
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
        Expr::UserCall { args, .. } => {
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
    let mut cfg = TaskCfgBuilder { blocks: Vec::new() };
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
        for statement in statements.iter().rev() {
            next = self.lower_stmt(statement, next, loop_targets);
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
                start,
                end,
                step,
                end_inclusive,
                body,
                ..
            } => {
                let step_expr = step.clone().unwrap_or_else(|| Expr::int(1));
                let end_field = format!("{var}__end");
                let step_field = format!("{var}__step");
                let header = self.push(Vec::new(), TaskCfgTerminator::Complete);
                let latch = self.push(
                    vec![assign_var(
                        var.clone(),
                        Expr::Binary {
                            loc: Default::default(),
                            op: BinaryOp::Add,
                            lhs: Box::new(Expr::var(var.clone())),
                            rhs: Box::new(Expr::var(step_field.clone())),
                        },
                    )],
                    TaskCfgTerminator::Jump(header),
                );
                let body = self.lower_list(body, latch, Some((next, latch)));
                let positive = compare(CmpOp::Gt, Expr::var(step_field.clone()), Expr::int(0));
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
                    condition: Expr::Logical {
                        loc: Default::default(),
                        op: onda_frontend::LogicalOp::Or,
                        lhs: Box::new(Expr::Logical {
                            loc: Default::default(),
                            op: onda_frontend::LogicalOp::And,
                            lhs: Box::new(positive),
                            rhs: Box::new(forward),
                        }),
                        rhs: Box::new(Expr::Logical {
                            loc: Default::default(),
                            op: onda_frontend::LogicalOp::And,
                            lhs: Box::new(compare(
                                CmpOp::Lt,
                                Expr::var(step_field.clone()),
                                Expr::int(0),
                            )),
                            rhs: Box::new(backward),
                        }),
                    },
                    then_block: body,
                    else_block: next,
                };
                self.push(
                    vec![
                        assign_var(var.clone(), start.clone()),
                        assign_var(end_field, end.clone()),
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
) -> Vec<Stmt> {
    let mut cfg = TaskCfgBuilder { blocks: Vec::new() };
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
) -> FunctionDef {
    let mut function_body = compile_task_resume_body(
        task_name,
        body,
        local_initializers,
        TASK_NODE_LOCAL,
        TASK_RESULT_LOCAL,
        true,
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
) -> Vec<Stmt> {
    compile_task_resume_body(
        task_name,
        body,
        local_initializers,
        &task_inline_node_local(task_name),
        &task_inline_result_local(task_name),
        false,
    )
}

fn rewrite_task_controls(
    stmts: &mut Vec<Stmt>,
    task_names: &HashSet<String>,
    buffer_names: &[String],
) {
    let mut rewritten = Vec::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        match &mut stmt {
            Stmt::Expr { expr, .. } => {
                if let Some(task) = await_task_name(expr).map(str::to_owned) {
                    let mut unavailable = vec![assign_var(TASK_AVAILABLE_FIELD, Expr::bool(false))];
                    unavailable.push(abort_activation_stmt());
                    rewritten.push(Stmt::If {
                        loc: Default::default(),
                        cond: Expr::UnaryNot {
                            loc: Default::default(),
                            expr: Box::new(user_call(
                                task_resume_def(&task),
                                buffer_names.iter().cloned().map(Expr::var).collect(),
                            )),
                        },
                        then_branch: unavailable,
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
                rewrite_task_controls(then_branch, task_names, buffer_names);
                rewrite_task_controls(else_branch, task_names, buffer_names);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                rewrite_task_controls(body, task_names, buffer_names)
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
    retained_fields: Vec<String>,
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
    return_types: &HashMap<String, PrimitiveType>,
    owner_types: &TaskOwnerTypes,
    placement: TaskResumePlacement,
    errors: &mut Vec<Diagnostic>,
) -> PreparedTask {
    let mut body = task.body.clone();
    let source_names = uniquify_task_bindings(&mut body, owner_roots);
    let locals = collect_task_locals(
        &body,
        owner_roots,
        return_types,
        owner_types,
        &source_names,
        &task.name,
        errors,
    );
    let task_local_names = locals.keys().cloned().collect::<HashSet<_>>();
    let live_across_yield = task_locals_live_across_yield(&body, &task_local_names);
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
    let expanded_arrays = locals
        .iter()
        .filter_map(|(name, storage)| {
            let TaskLocalStorage::Array { element, len, .. } = storage else {
                return None;
            };
            let lowered_name = names.get(name).cloned().unwrap_or_else(|| name.clone());
            (live_across_yield.contains(name) || matches!(placement, TaskResumePlacement::Inline))
                .then_some((lowered_name, (*element, *len)))
        })
        .collect::<HashMap<_, _>>();
    expand_task_array_frame_initializers(&mut body, &expanded_arrays);

    let mut init_stmts = vec![typed_assign(
        task_pc_field(&task.name),
        PrimitiveType::I32,
        Expr::int(0),
    )];
    let mut retained_fields = vec![task_pc_field(&task.name)];
    let mut reset_stmts = vec![assign_var(task_pc_field(&task.name), Expr::int(0))];
    let mut resume_local_initializers = Vec::new();
    let mut inline_scratch_declarations = vec![
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
            retained_fields.push(field.clone());
            reset_stmts.extend(storage.reset_stmts(field));
        } else {
            match placement {
                TaskResumePlacement::HelperFunction => {
                    resume_local_initializers.push(storage.init_stmt(local));
                }
                TaskResumePlacement::Inline => {
                    let scratch = names[&local].clone();
                    inline_scratch_declarations.push(storage.init_stmt(scratch.clone()));
                    resume_local_initializers.extend(storage.reset_stmts(scratch));
                }
            }
        }
    }

    let mut for_frame_fields = HashSet::new();
    collect_for_frame_fields(&body, &mut for_frame_fields);
    let mut for_frame_fields = for_frame_fields.into_iter().collect::<Vec<_>>();
    for_frame_fields.sort();
    for field in for_frame_fields {
        init_stmts.push(typed_assign(
            field.clone(),
            PrimitiveType::I32,
            Expr::int(0),
        ));
        retained_fields.push(field.clone());
        reset_stmts.push(assign_var(field, Expr::int(0)));
    }

    PreparedTask {
        name: task.name.clone(),
        body,
        resume_local_initializers,
        inline_scratch_declarations,
        init_stmts,
        reset_stmts,
        retained_fields,
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
    return_types: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) {
    if tasks.is_empty() {
        return;
    }

    let owner_surface = TaskOwnerSurface::from_top_level(program);
    let owner_types = collect_task_owner_types(&owner_surface, return_types);
    let owner_roots = collect_owner_roots(&owner_surface);
    let mut init_prefix = vec![typed_assign(
        TASK_AVAILABLE_FIELD,
        PrimitiveType::Bool,
        Expr::bool(true),
    )];
    let mut retained_fields = Vec::new();
    let mut scratch_declarations = Vec::new();
    let mut expansions = HashMap::new();
    for task in tasks {
        let prepared = prepare_task(
            task,
            &owner_roots,
            return_types,
            &owner_types,
            TaskResumePlacement::Inline,
            errors,
        );
        init_prefix.extend(prepared.init_stmts);
        retained_fields.extend(prepared.retained_fields);
        scratch_declarations.extend(prepared.inline_scratch_declarations);
        expansions.insert(
            prepared.name.clone(),
            InlineTaskExpansion {
                resume: compile_inline_task_resume(
                    &prepared.name,
                    &prepared.body,
                    prepared.resume_local_initializers,
                ),
                reset: prepared.reset_stmts,
                result: task_inline_result_local(&prepared.name),
            },
        );
    }

    let mut audio_outputs = HashSet::new();
    let mut control_outputs = HashSet::new();
    for block in &program.blocks {
        match block {
            Block::Outs(ports) => {
                audio_outputs.extend(ports.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::KOuts(ports) => {
                control_outputs.extend(ports.decls.iter().map(|decl| decl.name.clone()));
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
    let mut audio_outputs = audio_outputs.into_iter().collect::<Vec<_>>();
    audio_outputs.sort();
    let neutral_audio_outputs = audio_outputs
        .into_iter()
        .map(|name| assign_var(name, Expr::int(0)))
        .collect::<Vec<_>>();
    let mut control_outputs = control_outputs.into_iter().collect::<Vec<_>>();
    control_outputs.sort();
    let mut unavailable = control_outputs
        .into_iter()
        .map(|name| assign_var(name, Expr::int(0)))
        .collect::<Vec<_>>();
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
            retained_roots: Vec::new(),
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
    init.retained_roots.extend(retained_fields);
}

pub(crate) fn lower_tasks(program: &mut Program, errors: &mut Vec<Diagnostic>) {
    let return_types = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Def(def) => Some(def),
            _ => None,
        })
        .filter_map(|def| match &def.return_ty {
            Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(ty))) => {
                Some((def.name.clone(), *ty))
            }
            _ => None,
        })
        .collect::<HashMap<_, _>>();

    let top_level_tasks = take_top_level_tasks(program);
    lower_top_level_tasks(program, &top_level_tasks, &return_types, errors);

    let mut has_tasks = !top_level_tasks.is_empty();
    for proc in program.blocks.iter_mut().filter_map(|block| match block {
        Block::Proc(proc) => Some(proc),
        _ => None,
    }) {
        if proc.tasks.is_empty() {
            continue;
        }
        has_tasks = true;
        let mut proc_return_types = return_types.clone();
        proc_return_types.extend(
            proc.local_defs
                .iter()
                .filter_map(|def| match &def.return_ty {
                    Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(ty))) => {
                        Some((def.name.clone(), *ty))
                    }
                    _ => None,
                }),
        );
        let owner_surface = TaskOwnerSurface::from_proc(proc);
        let owner_types = collect_task_owner_types(&owner_surface, &proc_return_types);
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
        let mut retained_task_fields = Vec::new();
        let mut generated_defs = Vec::new();
        for task in &proc.tasks {
            let prepared = prepare_task(
                task,
                &owner_roots,
                &proc_return_types,
                &owner_types,
                TaskResumePlacement::HelperFunction,
                errors,
            );
            init_prefix.extend(prepared.init_stmts);
            retained_task_fields.extend(prepared.retained_fields);
            generated_defs.push(compile_task_resume(
                &prepared.name,
                &prepared.body,
                prepared.resume_local_initializers,
                buffer_params.clone(),
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

        rewrite_task_controls(&mut proc.init.body, &task_names, &buffer_names);
        rewrite_task_controls(&mut proc.block_pre, &task_names, &buffer_names);
        for event in &mut proc.events {
            rewrite_task_controls(&mut event.body, &task_names, &buffer_names);
        }
        let mut old_init = std::mem::take(&mut proc.init.body);
        init_prefix.append(&mut old_init);
        proc.init.body = init_prefix;
        proc.block_pre
            .insert(0, assign_var(TASK_AVAILABLE_FIELD, Expr::bool(true)));

        let mut unavailable = outputs
            .iter()
            .map(|name| assign_var(name.clone(), Expr::int(0)))
            .collect::<Vec<_>>();
        unavailable.push(abort_activation_stmt());
        proc.sample.insert(
            0,
            Stmt::If {
                loc: Default::default(),
                cond: Expr::UnaryNot {
                    loc: Default::default(),
                    expr: Box::new(Expr::var(TASK_AVAILABLE_FIELD)),
                },
                then_branch: unavailable,
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
        proc.init.retained_roots.extend(retained_task_fields);
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
    progress: i32 = 0 {retain}

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
            .any(|def| def.name.contains("__onda_task_load_resume")));
        assert!(typed
            .state_vars
            .iter()
            .any(|name| name.contains("__onda_task_load_pc")));
        let mir =
            lower_program_to_optimized_mir(&typed).expect("lowered task should produce valid MIR");
        assert!(mir.state.iter().any(|slot| {
            slot.name.contains("__onda_task_load_pc")
                && slot.reset == onda_mir::StateResetPolicy::Retain
        }));
        assert!(mir.state.iter().any(|slot| {
            slot.name == "loader.progress" && slot.reset == onda_mir::StateResetPolicy::Retain
        }));
    }

    #[test]
    fn task_frames_only_store_locals_live_across_yield() {
        let source = r#"
proc Loader:
  init:
    result: i32 = 0 {retain}
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
            .any(|name| name.contains("__onda_task_load_local_carried")));
        assert!(!typed
            .state_vars
            .iter()
            .any(|name| name.contains("__onda_task_load_local_scratch")));
    }

    #[test]
    fn initialized_task_array_frame_preserves_its_values() {
        let source = r#"
proc Loader:
  init:
    result: i32 = 0 {retain}
  task load():
    values: i32[2] = [3, 5]
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
    result: i32 = 0 {retain}
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
    result = 0.0 {retain}
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
                    .any(|name| name.contains(&format!("__onda_task_load_local_{local}"))),
                "missing frame storage for {local}"
            );
        }
        lower_program_to_optimized_mir(&typed)
            .expect("task buffer parameters and inferred frame types should lower");
    }
}
