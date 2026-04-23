use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::*;

mod emission;
mod inference;
mod orchestration;
mod planning;
mod resolution;
mod rewriting;
mod surface;
mod topology;
mod validation;
use emission::*;
use inference::*;
pub(crate) use orchestration::lower_graph_blocks;
use planning::*;
use resolution::*;
use rewriting::*;
use surface::*;
use topology::*;
use validation::*;

const GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL: &str = "__onda_graph_proc_array_field_index";
const GRAPH_PROC_FIELD_INDEX_EXPR_ARG: &str = "__proc_field_index_expr";

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum GraphNodeKey {
    Direct(String),
    Indexed { base: String, index: usize },
}

#[derive(Debug, Clone)]
struct GraphNodeInfo {
    proc_name: String,
}

#[derive(Debug, Clone)]
enum GraphDestKind {
    TopOutput(String),
    ProcInput { node: GraphNodeKey, port: String },
    ProcParam { node: GraphNodeKey, param: String },
}

#[derive(Debug, Clone)]
struct GraphDelayState {
    buf_name: String,
    head_name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
}

#[derive(Debug, Clone)]
struct ResolvedGraphSourcePlan {
    rate: GraphRate,
    delay: Option<usize>,
    deps: BTreeSet<GraphNodeKey>,
    original_source: Expr,
    source: Expr,
    delay_state: Option<GraphDelayState>,
    shared_tmp: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedGraphEdge {
    source_plan: usize,
    dest: GraphDestKind,
    dest_value_ty: GraphValueType,
}

#[derive(Debug, Clone)]
struct LoweredGraph {
    init_stmts: Vec<Stmt>,
    block_pre: Vec<Stmt>,
    sample: Vec<Stmt>,
}

#[derive(Debug, Clone)]
enum GraphSourceExpansion {
    Shared { expr: Expr, use_shared_tmp: bool },
    PerDest(Vec<Expr>),
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum GraphUsePoint {
    BeforeNode(GraphNodeKey),
    BeforeOutputs,
}
