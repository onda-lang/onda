#[test]
fn parses_graph_proc_array_output_array_slot_sources() {
    let src = r#"
proc Voice {
  outs:
    pair: f32[2]
  sample {
    pair[0] = 0.0
    pair[1] = 1.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = Voice()
}
graph {
  voices[1].pair[0] >> out1
}
"#;

    let program = parse_program(src).expect("graph proc-array output array program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    match &graph.edges[0].source {
        Expr::UserCall { name, args, .. } => {
            assert_eq!(name, GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL);
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_INDEX_BASE_ARG)
                    && matches!(arg.expr, Expr::Var { name: ref base, .. } if base == "voices")
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG)
                    && matches!(arg.expr, Expr::Int { value: 1, .. })
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_FIELD_SENTINEL_ARG)
                    && matches!(arg.expr, Expr::Var { name: ref field, .. } if field == "pair")
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(GRAPH_PROC_FIELD_INDEX_EXPR_ARG)
                    && matches!(arg.expr, Expr::Int { value: 0, .. })
            }));
        }
        other => panic!("expected graph source sentinel call, got {other:?}"),
    }
}

#[test]
fn rejects_multi_dot_inline_indexed_member_chain() {
    let src = r#"
outs { out1 }
struct Node { value: f32 }
struct Wrapper { nodes: Node[2] }
init {
  data: Wrapper[2]
}
sample {
  out1 = data[0].nodes[0].value
}
"#;

    assert!(
        parse_program(src).is_err(),
        "multiple inline dot levels after indexed member access should be rejected"
    );
}

#[test]
fn parses_graph_receiver_edges_with_delay() {
    let src = r#"
outs:
  out1

graph:
  @sample out1 <<[2] 0.5
"#;
    let program = parse_program(src).expect("graph receiver program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].rate, Some(GraphRate::Sample));
    assert_eq!(graph.edges[0].delay, Some(Expr::int(2)));
    assert_eq!(graph.edges[0].source, Expr::number(0.5));
    assert_eq!(graph.edges[0].dests.len(), 1);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::Symbol { ref name, .. } if name == "out1"
    ));
}

#[test]
fn parses_graph_delay_expressions_with_consts_and_namespace_generics() {
    let src = r#"
const TAP = 2

namespace DelayCfg<Base = 1>:
  const LEN = Base + TAP

outs:
  out1

graph:
  0.5 >>[DelayCfg<1>::LEN + 1] out1
"#;
    let program = parse_program(src).expect("graph delay expression program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 1);
    assert!(
        matches!(
            graph.edges[0].delay.as_ref(),
            Some(Expr::Binary { op: BinaryOp::Add, lhs, rhs, .. })
                if matches!(rhs.as_ref(), Expr::Int { value: 1, .. })
                    && matches!(lhs.as_ref(), Expr::Var { name, .. } if name == "DelayCfg<1>::LEN")
        ),
        "delay expr: {:?}",
        graph.edges[0].delay
    );
}

#[test]
fn parses_graph_array_literal_and_slice_sources() {
    let src = r#"
ins:
  in1
  in2
  in_bus: f32[4]
outs:
  out_st: f32[2]

graph:
  [in1, in2] >> out_st
  in_bus[1:3] >> out_st
"#;
    let program = parse_program(src).expect("graph array literal/slice program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    match &graph.edges[0].source {
        Expr::ArrayLiteral { values, .. } => {
            assert_eq!(values.len(), 2);
            assert!(matches!(values[0], Expr::Var { ref name, .. } if name == "in1"));
            assert!(matches!(values[1], Expr::Var { ref name, .. } if name == "in2"));
        }
        other => panic!("expected graph array literal source, got {other:?}"),
    }

    match &graph.edges[1].source {
        Expr::Slice {
            base, start, end, ..
        } => {
            assert_eq!(base, "in_bus");
            assert!(matches!(start.as_deref(), Some(Expr::Int { value: 1, .. })));
            assert!(matches!(end.as_deref(), Some(Expr::Int { value: 3, .. })));
        }
        other => panic!("expected graph slice source, got {other:?}"),
    }
}

#[test]
fn parses_graph_receiver_proc_array_destinations() {
    let src = r#"
proc Voice {
  params { gain = 0.0 }
  outs { out1 }
  sample { out1 = gain }
}
outs { out1 }
init {
  voices: Voice[2] = Voice()
}
graph {
  voices[1].gain << 0.5
  out1 << voices[1].out1
}
"#;

    let program = parse_program(src).expect("graph receiver proc-array program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 2);
    assert_eq!(graph.edges[0].source, Expr::number(0.5));
    assert_eq!(graph.edges[0].dests.len(), 1);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::ProcIndexedField {
            ref proc,
            index: Expr::Int { value: 1, .. },
            ref field,
            ..
        } if proc == "voices" && field == "gain"
    ));
}

#[test]
fn parses_graph_fanout_destinations() {
    let src = r#"
outs:
  out1
  out2

graph:
  0.5 >> { out1, out2 }
"#;
    let program = parse_program(src).expect("graph fanout program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].source, Expr::number(0.5));
    assert_eq!(graph.edges[0].dests.len(), 2);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::Symbol { ref name, .. } if name == "out1"
    ));
    assert!(matches!(
        graph.edges[0].dests[1],
        GraphEndpoint::Symbol { ref name, .. } if name == "out2"
    ));
}

#[test]
fn parses_graph_receiver_fanout_destinations() {
    let src = r#"
outs:
  out1
  out2

graph:
  { out1, out2 } << 0.5
"#;
    let program = parse_program(src).expect("graph receiver fanout program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].source, Expr::number(0.5));
    assert_eq!(graph.edges[0].dests.len(), 2);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::Symbol { ref name, .. } if name == "out1"
    ));
    assert!(matches!(
        graph.edges[0].dests[1],
        GraphEndpoint::Symbol { ref name, .. } if name == "out2"
    ));
}

#[test]
fn stmt_locations_capture_single_line_end_positions() {
    let src = "sample:\n  out1 = in1 + 1.0\n";
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");

    let loc = sample.body[0].loc();
    assert_eq!(loc.file().as_deref(), Some("<memory>"));
    assert_eq!(loc.line, 2);
    assert_eq!(loc.column, 3);
    assert_eq!(loc.end_line, 2);
    assert_eq!(loc.end_column, 19);
}

#[test]
fn stmt_locations_capture_multiline_end_positions() {
    let src = "sample {\n  clamp(\n    in1,\n    0.0,\n    1.0\n  )\n}\n";
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");

    let loc = sample.body[0].loc();
    assert_eq!(loc.file().as_deref(), Some("<memory>"));
    assert_eq!(loc.line, 2);
    assert_eq!(loc.column, 3);
    assert_eq!(loc.end_line, 6);
    assert_eq!(loc.end_column, 4);
}

#[test]
fn declaration_locations_capture_param_ranges() {
    let src = "params:\n  gain = 1.0\nsample:\n  out1 = gain\n";
    let program = parse_program(src).expect("program should parse");
    let params = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Params(params) => Some(params),
            _ => None,
        })
        .expect("params block");

    let loc = params[0].loc.as_ref().expect("param location");
    assert_eq!(loc.file().as_deref(), Some("<memory>"));
    assert_eq!(loc.line, 2);
    assert_eq!(loc.column, 3);
    assert_eq!(loc.end_line(), 2);
    assert_eq!(loc.end_column, 13);
}

#[test]
fn parses_proc_param_bind_hook() {
    let src = r#"
proc Voice:
  params:
    freq = 440.0 {20.0, 20000.0} => update_freq
  outs:
    out1
  sample:
    out1 = freq

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("processor param bind should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) if proc.name == "Voice" => Some(proc),
            _ => None,
        })
        .expect("Voice proc");

    assert_eq!(proc.params[0].name, "freq");
    assert_eq!(proc.params[0].bind.as_deref(), Some("update_freq"));
}

#[test]
fn parses_proc_private_params() {
    let src = r#"
proc Voice:
  params:
    private cutoff = 1000.0
    private coeffs: f32[2] = [0.5, 0.25]
  outs:
    out1
  sample:
    out1 = cutoff + coeffs[0]

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("private processor params should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) if proc.name == "Voice" => Some(proc),
            _ => None,
        })
        .expect("Voice proc");

    assert_eq!(proc.params[0].name, "cutoff");
    assert!(proc.params[0].private);
    assert_eq!(proc.params[1].name, "coeffs");
    assert!(proc.params[1].private);
    assert_eq!(proc.params.len(), 2);
}

#[test]
fn graph_locations_capture_edge_and_endpoint_ranges() {
    let src = "outs:\n  out1\ngraph:\n  0.5 >> out1\n";
    let program = parse_program(src).expect("program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    let edge_loc = graph.edges[0].loc();
    assert_eq!(edge_loc.file().as_deref(), Some("<memory>"));
    assert_eq!(edge_loc.line, 4);
    assert_eq!(edge_loc.column, 3);
    assert_eq!(edge_loc.end_line, 4);
    assert_eq!(edge_loc.end_column, 14);

    let dest_loc = graph.edges[0].dests[0].loc();
    assert_eq!(dest_loc.file().as_deref(), Some("<memory>"));
    assert_eq!(dest_loc.line, 4);
    assert_eq!(dest_loc.column, 10);
    assert_eq!(dest_loc.end_line, 4);
    assert_eq!(dest_loc.end_column, 14);
}

#[test]
fn parses_count_shorthand_span_for_semantic_diagnostics() {
    let src = "outs 0\nsample:\n  out1 = 0.0\n";
    let program = parse_program(src).expect("parse should preserve invalid count for semantics");
    let outs = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Outs(outs) => Some(outs),
            _ => None,
        })
        .expect("outs block");
    let loc = outs.deferred_count.as_ref().expect("deferred count").loc();

    assert_eq!((loc.line, loc.column), (1, 6));
    assert_eq!(loc.end_line, 1);
    assert_eq!(loc.end_column, 7);
}

#[test]
fn preserves_invalid_scalar_const_expr_for_semantics() {
    let src = "const X = foo\nouts:\n  out1\nsample:\n  out1 = 0.0\n";
    let program = parse_program(src).expect("parser should preserve invalid const for semantics");
    let decl = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Const(decl) if decl.name == "X" => Some(decl),
            _ => None,
        })
        .expect("const declaration should be retained");
    let loc = decl.expr.loc();

    assert_eq!((loc.line, loc.column), (1, 11));
    assert_eq!(loc.end_line, 1);
    assert_eq!(loc.end_column, 14);
}

#[test]
fn duplicate_namespace_template_diagnostics_report_namespace_span() {
    let src = "namespace Config<T = 1>:\n  struct A:\n    x: f32\nnamespace Config<T = 1>:\n  struct B:\n    x: f32\n";
    let program = parse_program(src).expect("duplicate namespace templates are semantic errors");
    assert_eq!(
        program
            .blocks
            .iter()
            .filter(|block| matches!(block, Block::Namespace(ns) if ns.name == "Config"))
            .count(),
        2
    );
}

#[test]
fn duplicate_namespace_alias_diagnostics_report_alias_span() {
    let src = "namespace Alias = std::math\nnamespace Alias = std::math\n";
    let program = parse_program(src).expect("duplicate namespace aliases are semantic errors");
    assert_eq!(
        program
            .blocks
            .iter()
            .filter(|block| matches!(block, Block::NamespaceAlias(alias) if alias.name == "Alias"))
            .count(),
        2
    );
}

#[test]
fn unknown_namespace_template_diagnostics_report_use_site_span() {
    let src = "outs:\n  out1\nsample:\n  out1 = Missing<1>::X\n";
    let program = parse_program(src).expect("unknown namespace templates are semantic errors");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample");
    assert!(
        matches!(&sample.body[0], Stmt::Assign { expr: Expr::Var { name, .. }, .. } if name == "Missing<1>::X")
    );
}

#[test]
fn namespace_template_argument_count_diagnostics_report_extra_arg_span() {
    let src = "namespace Data<S = SR, C = 1>:\n  const X = 0.0\nouts:\n  out1\nsample:\n  out1 = Data<1, 2, 3>::X\n";
    let program = parse_program(src).expect("argument count is validated semantically");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample");
    assert!(
        matches!(&sample.body[0], Stmt::Assign { expr: Expr::Var { name, .. }, .. } if name == "Data<1, 2, 3>::X")
    );
}

#[test]
fn typed_decl_namespace_template_diagnostics_report_type_span() {
    let src = "params:\n  gain: Missing<1>::X = 0.0\nouts:\n  out1\nsample:\n  out1 = 0.0\n";
    let program = parse_program(src).expect("unknown namespace templates are semantic errors");
    let params = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Params(params) => Some(params),
            _ => None,
        })
        .expect("params");
    assert!(
        matches!(params.decls[0].ty, Some(DeclType::Generic(ref name)) if name == "Missing<1>::X")
    );
}

#[test]
fn local_typed_decl_namespace_template_diagnostics_report_type_span() {
    let src = "outs:\n  out1\ninit:\n  x: Missing<1>::X = 0.0\nsample:\n  out1 = 0.0\n";
    let program = parse_program(src).expect("unknown namespace templates are semantic errors");
    let init = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init");
    assert!(
        matches!(&init.body[0], Stmt::Assign { generic_decl_ty: Some(name), .. } if name.trim() == "Missing<1>::X")
    );
}

#[test]
fn parses_decimal_literals_with_f64_precision() {
    let src = "outs:\n  out1\nsample:\n  out1 = 0.12345678901234566\n";
    let program = parse_program(src).expect("decimal literal program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(&sample.body),
            _ => None,
        })
        .expect("sample block");
    let expr = match &sample[0] {
        Stmt::Assign { expr, .. } => expr,
        other => panic!("expected sample assignment, got {other:?}"),
    };

    match expr {
        Expr::Number { value, .. } => {
            let expected = 0.12345678901234566_f64;
            let widened_f32 = expected as f32 as f64;
            assert!(
                (value - expected).abs() < 1e-18,
                "expected parser to preserve f64 precision, got {value:?}"
            );
            assert!(
                (value - widened_f32).abs() > 1e-9,
                "expected parser value to differ from widened f32 literal, got {value:?}"
            );
        }
        other => panic!("expected number literal, got {other:?}"),
    }
}

#[test]
fn parses_delegate_forms_and_static_subscription_targets_in_source_order() {
    let source = r#"delegate first(value)
delegates:
  second(values: f32[])
  third(flag: bool = true)

when first(value):
  result = value
when child.second(values):
  result = values[0]
when children[2].third(flag):
  result = f32(flag)
when children.first(index, value):
  result = f32(index + value)

proc Child:
  delegate local(value: i32)
  delegates:
    other(values: f64[2])
  when local(value):
    state = value
  sample:
    out1 = 0.0

sample:
  out1 = 0.0
"#;
    let program = parse_program(source).expect("delegate syntax should parse");
    let delegates = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Delegates(delegates) => Some(&delegates.delegates),
            _ => None,
        })
        .expect("merged top-level delegates");
    assert_eq!(
        delegates
            .iter()
            .map(|delegate| delegate.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
    assert!(matches!(
        delegates[0].params[0].ty,
        EventParamType::Scalar(PrimitiveType::F32)
    ));
    assert!(matches!(
        delegates[1].params[0].ty,
        EventParamType::Slice {
            elem: PrimitiveType::F32
        }
    ));
    assert!(delegates[2].params[0].default.is_some());

    let whens = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::When(when) => Some(when),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(whens.len(), 4);
    assert!(whens[0].target.receiver.is_empty());
    assert_eq!(whens[1].target.receiver, ["child"]);
    assert_eq!(whens[2].target.receiver, ["children"]);
    assert!(whens[2].target.index.is_some());
    assert_eq!(whens[3].bindings[0].name, "index");

    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) => Some(proc),
            _ => None,
        })
        .expect("processor");
    assert_eq!(
        proc.delegates
            .iter()
            .map(|delegate| delegate.name.as_str())
            .collect::<Vec<_>>(),
        ["local", "other"]
    );
    assert_eq!(proc.whens.len(), 1);
}

#[test]
fn rejects_duplicate_delegates_across_singular_and_plural_forms() {
    let errors = parse_program("delegate done()\ndelegates:\n  done(value: i32)\n")
        .expect_err("duplicate delegates should fail during parsing");
    assert!(errors.iter().any(|error| error
        .message
        .contains("duplicate delegate declaration 'done'")));
}
