//! Native cost decisions using shared MIR facts. These hints do not change
//! legality, floating-point permissions, or the facts available to alias analysis.
use super::*;
use onda_mir::{BinaryOp, Function, ScalarValue};

impl FunctionEmitter<'_, '_> {
    pub(super) unsafe fn mark_short_memory_loop(&self, branch: LLVMValueRef, body: &Block) {
        // LLVM 21's x86 cost model tail-folds tiny runtime-length copies into
        // masked loads/stores. Their setup and masked memory costs exceed
        // scalar code on measured AVX2 hosts. Leave arithmetic-heavy loops and
        // other targets to their ordinary cost models; SLP remains enabled.
        if !self.module.scalar_short_memory_loops || self.parallel_loop.is_some() {
            return;
        }
        let Some(counted) = onda_mir::counted_loop(body) else {
            return;
        };
        let maximum = match counted.limit {
            Value::Local(local) => self.ranges.local(local).map(|range| range.max()),
            Value::Constant(ScalarValue::I32(limit)) => Some(i64::from(limit)),
            _ => None,
        };
        if !maximum.is_some_and(|maximum| (1..=7).contains(&maximum))
            || !nonnegative_induction(self.function, counted.index)
            || !cheap_memory_body(self.module.program, self.function, counted.body)
        {
            return;
        }
        let context = self.module.context;
        let mut property = [
            LLVMMDStringInContext2(context, c"llvm.loop.vectorize.enable".as_ptr(), 26),
            LLVMValueAsMetadata(LLVMConstInt(LLVMInt1TypeInContext(context), 0, 0)),
        ];
        let property = LLVMMDNodeInContext2(context, property.as_mut_ptr(), property.len());
        let node = self_referential_metadata_node(context, &[property]);
        let kind = LLVMGetMDKindIDInContext(context, c"llvm.loop".as_ptr(), 9);
        LLVMSetMetadata(branch, kind, LLVMMetadataAsValue(context, node));
    }
}

fn nonnegative_induction(function: &Function, induction: onda_mir::LocalId) -> bool {
    let mut pending = vec![&function.body];
    let mut resets = 0;
    let mut steps = 0;
    while let Some(block) = pending.pop() {
        for statement in &block.statements {
            match &statement.kind {
                StatementKind::Assign {
                    value:
                        Rvalue::MakeSlice {
                            source:
                                onda_mir::SliceSource::Place(Place {
                                    base: onda_mir::PlaceBase::Local(local),
                                    ..
                                }),
                            ..
                        },
                    ..
                } if *local == induction => return false,
                StatementKind::Assign {
                    destination:
                        Place {
                            base: onda_mir::PlaceBase::Local(local),
                            projections,
                        },
                    value,
                } if *local == induction && projections.is_empty() => match value {
                    Rvalue::Use(Value::Constant(ScalarValue::I32(0))) => resets += 1,
                    Rvalue::Binary {
                        op: BinaryOp::Add,
                        lhs: Value::Local(local),
                        rhs: Value::Constant(ScalarValue::I32(1)),
                    } if *local == induction => steps += 1,
                    _ => return false,
                },
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    pending.push(then_block);
                    pending.push(else_block);
                }
                StatementKind::Loop { body } => pending.push(body),
                StatementKind::Call { args, results, .. }
                    if results.contains(&induction)
                        || args.iter().any(|argument| {
                            matches!(argument,
                        CallArgument::Place(Place { base: onda_mir::PlaceBase::Local(local), .. })
                        if *local == induction)
                        }) =>
                {
                    return false
                }
                _ => {}
            }
        }
    }
    resets == 1 && steps == 1
}

fn cheap_memory_body(program: &Program, function: &Function, body: &[onda_mir::Statement]) -> bool {
    let mut reads = 0;
    let mut writes = 0;
    for statement in body {
        match &statement.kind {
            StatementKind::Assign {
                destination:
                    Place {
                        base: onda_mir::PlaceBase::Local(local),
                        projections,
                    },
                value,
            } if projections.is_empty() => match value {
                Rvalue::SliceLoad { .. } => reads += 1,
                Rvalue::Use(_) | Rvalue::SliceLen(_) => {}
                Rvalue::Binary { op, .. }
                    if !matches!(op, BinaryOp::Divide | BinaryOp::Remainder)
                        && matches!(
                            program.types[function.locals[local.index()].ty.index()],
                            Type::Scalar(ScalarType::I32 | ScalarType::I64)
                        ) => {}
                Rvalue::Cast { .. } | Rvalue::Compare { .. } | Rvalue::NormalizeIndex { .. }
                    if matches!(
                        program.types[function.locals[local.index()].ty.index()],
                        Type::Scalar(ScalarType::I32 | ScalarType::I64 | ScalarType::Bool)
                    ) => {}
                _ => return false,
            },
            StatementKind::SliceStore { .. } => writes += 1,
            _ => return false,
        }
    }
    reads > 0 && writes > 0
}
