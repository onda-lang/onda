use onda_codegen_llvm::jit_program_from_optimized_mir;
use onda_frontend::{parse_program, PrimitiveType};
use onda_runtime::*;
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}
struct Allocator;
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        TRACK.with(|track| {
            if track.get() {
                COUNTS.with(|counts| {
                    let (a, d) = counts.get();
                    counts.set((a + 1, d));
                });
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        TRACK.with(|track| {
            if track.get() {
                COUNTS.with(|counts| {
                    let (a, d) = counts.get();
                    counts.set((a, d + 1));
                });
            }
        });
        unsafe { System.dealloc(pointer, layout) }
    }
}
fn no_heap<T>(operation: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TRACK.with(|track| track.set(false));
        }
    }
    COUNTS.with(|counts| counts.set((0, 0)));
    TRACK.with(|track| track.set(true));
    let reset = Reset;
    let value = operation();
    drop(reset);
    assert_eq!(
        COUNTS.with(Cell::get),
        (0, 0),
        "runtime allocated or deallocated"
    );
    value
}
fn instance(source: &str) -> Instance {
    let typed = analyze_with_options(
        parse_program(source).unwrap(),
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 8,
        },
    )
    .unwrap();
    let program =
        jit_program_from_optimized_mir(lower_program_to_optimized_mir(&typed).unwrap()).unwrap();
    create_instance(program, None).unwrap()
}

#[test]
fn execution_and_rejection_paths_neither_allocate_nor_free() {
    let mut instance = instance(
        r#"
params:
  divisor: i32 = 1
buffers:
  data: f32
init:
  value = 8 / divisor
events:
  set(value: i32):
    print(value)
sample:
  print(value)
  out1 = f32(value / divisor)
"#,
    );
    let mut samples = [0.0f32; 8];
    let mut storage = [0u8; 512];
    let mut prints = PrintBatch::from_storage(&mut storage);
    assert_eq!(
        no_heap(|| process_checked(&mut instance, ExecutionOutput::none())),
        Err(RuntimeError::Uninitialized)
    );
    assert_eq!(
        no_heap(|| init_checked(&mut instance, InitMode::Full, ExecutionOutput::none())),
        Ok(0)
    );
    assert!(matches!(
        no_heap(|| prepare_process(&instance)),
        Err(RuntimeError::MissingBinding { .. })
    ));
    no_heap(|| unsafe { bind_output(&mut instance, 0, samples.as_mut_ptr().cast(), 32) }).unwrap();
    assert!(
        no_heap(|| unsafe { bind_output(&mut instance, 0, samples.as_mut_ptr().cast(), 31) })
            .is_err()
    );
    assert!(no_heap(|| unsafe {
        bind_buffer(
            &mut instance,
            0,
            samples.as_mut_ptr().cast(),
            usize::MAX,
            1,
            48_000.0,
            PrimitiveType::F32,
        )
    })
    .is_err());
    assert!(no_heap(|| set_param_by_index(&mut instance, 1, &[0; 4])).is_err());
    assert!(no_heap(|| process_checked_segment(
        &mut instance,
        7,
        2,
        PROCESSOR_FULL_BLOCK,
        ExecutionOutput::none()
    ))
    .is_err());
    assert_eq!(
        no_heap(|| trigger_event_by_index_checked(&mut instance, 0, &[], ExecutionOutput::none())),
        Ok(PROCESSOR_EXECUTION_INPUT_REJECTED)
    );
    assert_eq!(
        no_heap(|| trigger_event_by_index_checked(
            &mut instance,
            0,
            &1i32.to_ne_bytes(),
            ExecutionOutput::none()
        )),
        Ok(0)
    );
    assert_eq!(
        no_heap(|| process_checked(
            &mut instance,
            ExecutionOutput {
                delegate_batch: None,
                print_batch: Some(&mut prints)
            }
        )),
        Ok(0)
    );
    let mut snapshot = vec![0; instance.state_size_bytes()];
    no_heap(|| instance.write_snapshot_state_bytes(&mut snapshot)).unwrap();
    assert!(no_heap(|| instance.restore_state_bytes(&[])).is_err());
    no_heap(|| set_param_by_index(&mut instance, 0, &0i32.to_ne_bytes())).unwrap();
    assert_eq!(
        no_heap(|| process_checked(
            &mut instance,
            ExecutionOutput {
                delegate_batch: None,
                print_batch: Some(&mut prints)
            }
        )),
        Ok(PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE)
    );
    assert_eq!(prints.record_count, 1);
    assert_eq!(
        no_heap(|| process_checked(&mut instance, ExecutionOutput::none())),
        Err(RuntimeError::Invalidated)
    );
    assert_eq!(
        no_heap(|| instance.restore_state_bytes(&snapshot)),
        Ok(PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE)
    );
    no_heap(|| set_param_by_index(&mut instance, 0, &1i32.to_ne_bytes())).unwrap();
    assert_eq!(no_heap(|| instance.restore_state_bytes(&snapshot)), Ok(0));
    assert_eq!(
        no_heap(|| unsafe { process_unchecked(&mut instance, ExecutionOutput::none()) }),
        Ok(0)
    );
}

#[test]
fn full_blocks_match_segments_in_audio_hooks_prints_and_state() {
    let source = r#"
ins:
  in1
outs:
  out1
init:
  pre = 0.0
  post = 0.0
  sum = 0.0
block:
  pre += 1.0
  sample:
    sum += in1
    out1 = sum + pre * 100.0 + post
    print(sum + pre * 100.0 + post)
  post += 1.0
"#;
    let mut full = instance(source);
    let mut segmented = instance(source);
    let input = [0.25f32; 8];
    let mut a = [0.0f32; 8];
    let mut b = [0.0f32; 8];
    for (instance, output) in [(&mut full, &mut a), (&mut segmented, &mut b)] {
        init_checked(instance, InitMode::Full, ExecutionOutput::none()).unwrap();
        unsafe {
            bind_input(instance, 0, input.as_ptr().cast(), 32).unwrap();
            bind_output(instance, 0, output.as_mut_ptr().cast(), 32).unwrap();
        }
    }
    let mut storage_a = [0u8; 256];
    let mut storage_b = [0u8; 256];
    let mut prints_a = PrintBatch::from_storage(&mut storage_a);
    let mut prints_b = PrintBatch::from_storage(&mut storage_b);
    for _ in 0..32 {
        assert_eq!(
            process_checked(
                &mut full,
                ExecutionOutput {
                    delegate_batch: None,
                    print_batch: Some(&mut prints_a)
                }
            ),
            Ok(0)
        );
        assert_eq!(
            process_checked_segment(
                &mut segmented,
                0,
                8,
                PROCESSOR_FULL_BLOCK,
                ExecutionOutput {
                    delegate_batch: None,
                    print_batch: Some(&mut prints_b)
                }
            ),
            Ok(0)
        );
        assert_eq!(a, b);
        assert_eq!(
            full.snapshot_state_bytes().unwrap(),
            segmented.snapshot_state_bytes().unwrap()
        );
        assert_eq!(
            prints_a.occurrences().collect::<Result<Vec<_>, _>>(),
            prints_b.occurrences().collect::<Result<Vec<_>, _>>()
        );
    }
    for _ in 0..32 {
        assert_eq!(process_checked(&mut full, ExecutionOutput::none()), Ok(0));
        assert_eq!(
            process_checked_segment(
                &mut segmented,
                0,
                3,
                PROCESSOR_BEGIN_BLOCK,
                ExecutionOutput::none()
            ),
            Ok(0)
        );
        assert_eq!(
            process_checked_segment(
                &mut segmented,
                3,
                5,
                PROCESSOR_END_BLOCK,
                ExecutionOutput::none()
            ),
            Ok(0)
        );
        assert_eq!(a, b);
        assert_eq!(
            full.snapshot_state_bytes().unwrap(),
            segmented.snapshot_state_bytes().unwrap()
        );
    }
}

#[test]
fn rejected_rebind_preserves_the_live_binding_and_unbind_is_immediate() {
    let mut instance = instance("sample:\n  out1 = 0.25\n");
    init_checked(&mut instance, InitMode::Full, ExecutionOutput::none()).unwrap();
    let mut first = [0.0f32; 8];
    let mut second = [0.0f32; 8];
    unsafe {
        bind_output(&mut instance, 0, first.as_mut_ptr().cast(), 32).unwrap();
        let bad = second.as_mut_ptr().cast::<u8>().add(1);
        assert!(matches!(
            bind_output(&mut instance, 0, bad, 32),
            Err(RuntimeError::Misaligned { .. })
        ));
    }
    assert_eq!(
        process_checked(&mut instance, ExecutionOutput::none()),
        Ok(0)
    );
    assert_eq!(first, [0.25; 8]);
    assert_eq!(second, [0.0; 8]);
    unsafe {
        bind_output(&mut instance, 0, second.as_mut_ptr().cast(), 32).unwrap();
    }
    assert_eq!(
        process_checked(&mut instance, ExecutionOutput::none()),
        Ok(0)
    );
    assert_eq!(second, [0.25; 8]);
    unsafe {
        bind_output(&mut instance, 0, std::ptr::null_mut(), 0).unwrap();
    }
    assert!(matches!(
        prepare_process(&instance),
        Err(RuntimeError::MissingBinding { .. })
    ));
}

#[cfg(target_arch = "x86_64")]
#[test]
fn runtime_leaves_thread_fp_policy_to_the_host() {
    unsafe fn read() -> u32 {
        let mut value = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut value, options(nostack, preserves_flags));
        value
    }
    unsafe fn write(value: u32) {
        std::arch::asm!("ldmxcsr [{}]", in(reg) &value, options(nostack, preserves_flags));
    }
    struct Restore(u32);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe { write(self.0) }
        }
    }
    let mut instance = instance(
        "events:\n  noop():\n    value = 1.0\ninit:\n  value = 0.0\nsample:\n  out1 = value\n",
    );
    let mut output = [0.0f32; 8];
    unsafe {
        bind_output(&mut instance, 0, output.as_mut_ptr().cast(), 32).unwrap();
    }
    let original = Restore(unsafe { read() });
    let policy = (1 << 6) | (1 << 15);
    unsafe {
        write(original.0 & !policy);
    }
    assert_eq!(
        init_checked(&mut instance, InitMode::Full, ExecutionOutput::none()),
        Ok(0)
    );
    assert_eq!(
        trigger_event_by_index_checked(&mut instance, 0, &[], ExecutionOutput::none()),
        Ok(0)
    );
    assert_eq!(
        process_checked(&mut instance, ExecutionOutput::none()),
        Ok(0)
    );
    assert_eq!(unsafe { read() } & policy, 0);
}
