use super::*;

#[test]
fn trusted_fixed_indices_export_their_logical_bounds_to_llvm() {
    let (_, mir) = source_program(
        "params:\n  index: i32 = 2\ninit:\n  values: f32[4]\nsample:\n  out1 = values.read_unsafe(index)\n",
        1,
    );
    let mut target = crate::TargetConfig::host();
    target.opt_level = TargetOptLevel::O0;
    let ir = lower_mir_to_target_llvm_ir(
        &mir,
        &MirTargetOptions {
            fast_math: false,
            target,
        },
    )
    .expect("trusted fixed index emits IR");
    assert!(
        ir.contains("call void @llvm.assume(i1 %proven_index"),
        "{ir}"
    );
    assert!(
        ir.lines().any(|line| line.contains("proven_index")
            && line.contains("icmp ult i32")
            && line.ends_with(", 4")),
        "{ir}"
    );
}

#[test]
fn address_offsets_use_index_width_not_pointer_width() {
    assert_eq!(max_signed_index_offset("e-p:32:32", 32), i32::MAX as u64);
    assert_eq!(max_signed_index_offset("e-p:64:64", 64), i64::MAX as u64);
    assert_eq!(
        max_signed_index_offset("e-p0:64:64:64:32", 64),
        i32::MAX as u64
    );
    assert_eq!(
        max_signed_index_offset("e-p:64:64:64:16", 64),
        i16::MAX as u64
    );
    assert_eq!(max_signed_index_offset("e-p1:32:32", 64), i64::MAX as u64);
}

#[test]
fn fixed_array_address_guarantees_follow_target_byte_extent() {
    let (_, original) = source_program(
        "params:\n  index: i32 = 0\ninit:\n  values: f32[4]\nsample:\n  out1 = values[index]\n",
        1,
    );
    for triple in [
        "x86_64-unknown-linux-gnu",
        "i686-unknown-linux-gnu",
        "wasm32-unknown-unknown",
    ] {
        // MIR itself rejects fixed aggregates larger than i32::MAX bytes.
        // Exercise the largest representable f32 array without allocating it.
        for len in [4, 536_870_911, 536_870_912] {
            let mut mir = original.clone();
            for ty in &mut mir.types {
                if let Type::Array { len: array_len, .. } = ty {
                    *array_len = len;
                }
            }
            let mut target = crate::TargetConfig::for_triple(triple);
            target.opt_level = TargetOptLevel::O0;
            let result = lower_mir_to_target_llvm_ir(
                &mir,
                &MirTargetOptions {
                    fast_math: false,
                    target,
                },
            );
            if len == 536_870_912 {
                assert!(result
                    .unwrap_err()
                    .iter()
                    .any(|error| error.message.contains("logical size exceeds")));
                continue;
            }
            let ir = result.expect("fixed array IR");
            let address = ir
                .lines()
                .find(|line| line.contains("array_element") && line.contains("getelementptr"))
                .expect("array address");
            assert!(
                address.contains("getelementptr inbounds"),
                "{triple}, {len} elements: {address}"
            );
        }
    }
}

#[test]
fn bounded_array_addresses_preserve_clamped_and_trusted_accesses() {
    let source = r#"
params:
  low: i32 = -2147483647
  high: i32 = 2147483647
  valid: i32 = 2
init:
  values: f32[4] = [10.0, 20.0, 30.0, 40.0]
sample:
  values[low] += 1.0
  values[high] += 2.0
  out1 = values[low]
  out2 = values[high]
  out3 = values.read_unsafe(valid)
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(
            run_native_outputs_with_opt_level(source, 3, level),
            [
                vec![11.0, 12.0, 13.0],
                vec![42.0, 44.0, 46.0],
                vec![30.0; 3],
            ]
        );
    }
}
