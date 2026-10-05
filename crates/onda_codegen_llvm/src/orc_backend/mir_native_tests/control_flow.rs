use super::*;

#[test]
fn returning_loops_without_breaks_terminate_the_function() {
    let (_, mut mir) = source_program(
        "ins:\n  in1\ndef identity(value: f32):\n  return value\nsample:\n  out1 = identity(in1)\n",
        1,
    );
    let function = mir
        .functions
        .iter_mut()
        .find(|function| {
            matches!(function.kind, FunctionKind::User) && function.name.contains("identity")
        })
        .expect("identity helper");
    let body = std::mem::take(&mut function.body);
    let mut statement = body.statements[0].clone();
    statement.kind = StatementKind::Loop { body };
    function.body.statements.push(statement);

    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("a returning loop should emit valid LLVM IR without an exit edge");
    assert!(ir.contains("unreachable"), "{ir}");
}
