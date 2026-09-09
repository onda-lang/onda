use super::*;

fn field(name: &str, ty: PayloadType) -> PayloadField {
    PayloadField {
        name: name.to_owned(),
        ty,
        default: None,
    }
}

fn schema() -> PayloadSchema {
    PayloadSchema {
        params: vec![
            field("enabled", PayloadType::scalar(ScalarEncoding::Bool)),
            field(
                "notes",
                PayloadType::Slice {
                    element: Box::new(PayloadType::Struct {
                        name: "Note".into(),
                        fields: vec![
                            field("gain", PayloadType::scalar(ScalarEncoding::F64)),
                            field(
                                "bins",
                                PayloadType::Array {
                                    len: 2,
                                    element: Box::new(PayloadType::scalar(ScalarEncoding::I32)),
                                },
                            ),
                        ],
                    }),
                },
            ),
            field("last", PayloadType::scalar(ScalarEncoding::I64)),
        ],
    }
}

fn input() -> Vec<u8> {
    let mut result = vec![1];
    result.extend(2_i32.to_le_bytes());
    for gain in [1.5_f64, -0.0] {
        result.extend(gain.to_le_bytes());
    }
    for bin in [3_i32, 5, 7, 11] {
        result.extend(bin.to_le_bytes());
    }
    result.extend(i64::MIN.to_le_bytes());
    result
}

#[repr(align(8))]
struct Workspace([u8; 128]);

#[test]
fn nested_soa_payloads_prepare_align_and_round_trip() {
    let plan = PayloadPlan::new(&schema()).unwrap();
    assert_eq!(
        plan.tensors()
            .iter()
            .map(|tensor| tensor.path.as_str())
            .collect::<Vec<_>>(),
        ["enabled", "notes.gain", "notes.bins", "last"]
    );
    assert_eq!(plan.tensors()[2].shape, [2]);
    assert_eq!(plan.sizes(&[2]).unwrap(), (45, 48));
    let input = input();
    let mut workspace = Workspace([0xa5; 128]);
    let prepared = plan.prepare(&input, &mut workspace.0).unwrap();
    let mut extents = Vec::new();
    prepared.visit_tensors(|tensor, region, bytes| {
        assert_eq!(bytes.as_ptr() as usize % tensor.encoding.byte_size(), 0);
        extents.push(region.elements);
    });
    assert_eq!(extents, [1, 2, 4, 1]);
    let mut output = vec![0; input.len()];
    prepared.encode(&mut output).unwrap();
    assert_eq!(output, input);
}

#[test]
fn rejected_input_or_capacity_does_not_mutate_workspace_or_output() {
    let plan = PayloadPlan::new(&schema()).unwrap();
    let input = input();
    let mut workspace = Workspace([0xa5; 128]);
    for len in 0..input.len() {
        assert!(plan.prepare(&input[..len], &mut workspace.0).is_err());
        assert_eq!(workspace.0, [0xa5; 128]);
    }
    assert!(matches!(
        plan.prepare(&input, &mut workspace.0[..47]),
        Err(PayloadError::InsufficientCapacity)
    ));
    assert_eq!(workspace.0, [0xa5; 128]);
    let mut trailing = input.clone();
    trailing.push(0);
    assert_eq!(
        plan.required_workspace(&trailing),
        Err(PayloadError::TrailingBytes)
    );
    let mut negative = input.clone();
    negative[1..5].copy_from_slice(&(-1_i32).to_le_bytes());
    assert_eq!(
        plan.required_workspace(&negative),
        Err(PayloadError::NegativeLength)
    );
    let prepared = plan.prepare(&input, &mut workspace.0).unwrap();
    let mut output = [0xa5; 44];
    assert_eq!(
        prepared.encode(&mut output),
        Err(PayloadError::InsufficientCapacity)
    );
    assert_eq!(output, [0xa5; 44]);
}

#[test]
fn dynamic_empty_payloads_keep_later_parameters_and_plan_size_bounded() {
    let plan = PayloadPlan::new(&schema()).unwrap();
    assert_eq!(plan.sizes(&[0]).unwrap(), (13, 16));
    assert_eq!(plan.tensors().len(), 4);
    assert!(plan.sizes(&[1_000_000]).is_ok());
    assert_eq!(plan.sizes(&[i32::MAX]), Err(PayloadError::Overflow));
    assert_eq!(plan.sizes(&[]), Err(PayloadError::InvalidLengths));
    assert_eq!(plan.sizes(&[1, 2]), Err(PayloadError::InvalidLengths));
}

#[test]
fn wire_capacity_accounts_for_worst_case_workspace_alignment() {
    let schema = PayloadSchema {
        params: vec![
            field(
                "values",
                PayloadType::Slice {
                    element: Box::new(PayloadType::scalar(ScalarEncoding::F64)),
                },
            ),
            field("tail", PayloadType::scalar(ScalarEncoding::Bool)),
        ],
    };
    let plan = PayloadPlan::new(&schema).unwrap();
    assert_eq!(plan.sizes(&[8_191]).unwrap(), (65_533, 65_537));
    assert_eq!(
        plan.workspace_capacity_for_wire_capacity(65_536).unwrap(),
        65_546
    );
}

#[test]
fn normalization_preserves_input_and_handles_full_width_integer_domains() {
    let domain = crate::IntegerRangeMetadata {
        min: crate::IntegerRangeEndpoint {
            scalar: "i64".into(),
            value: "-3".into(),
        },
        max: crate::IntegerRangeEndpoint {
            scalar: "i64".into(),
            value: "3".into(),
        },
        mode: "wrap".into(),
    };
    let schema = PayloadSchema {
        params: vec![field(
            "count",
            PayloadType::Scalar {
                encoding: ScalarEncoding::I64,
                integer_range: Some(domain),
            },
        )],
    };
    let plan = PayloadPlan::new(&schema).unwrap();
    let input = i64::MIN.to_le_bytes();
    let mut workspace = Workspace([0; 128]);
    let prepared = plan.prepare(&input, &mut workspace.0).unwrap();
    assert_eq!(input, i64::MIN.to_le_bytes());
    let expected = (-3_i128 + (i64::MIN as i128 + 3).rem_euclid(7)) as i64;
    assert_eq!(prepared.storage(), expected.to_ne_bytes());
}

impl PayloadSink for PayloadDefault {
    fn scalar(encoding: ScalarEncoding, bytes: &[u8]) -> Self {
        let text = match encoding {
            ScalarEncoding::Bool => (bytes[0] != 0).to_string(),
            ScalarEncoding::I32 => i32::from_le_bytes(bytes.try_into().unwrap()).to_string(),
            ScalarEncoding::I64 => i64::from_le_bytes(bytes.try_into().unwrap()).to_string(),
            ScalarEncoding::F32 => f32::from_le_bytes(bytes.try_into().unwrap()).to_string(),
            ScalarEncoding::F64 => f64::from_le_bytes(bytes.try_into().unwrap()).to_string(),
        };
        Self::Scalar(text)
    }
    fn aggregate(_: &PayloadType, values: Vec<Self>) -> Self {
        Self::Aggregate(values)
    }
}

#[test]
fn logical_values_round_trip_nested_arrays_and_exact_integers() {
    let plan = PayloadPlan::new(&schema()).unwrap();
    let values = plan.decode_values::<PayloadDefault>(&input()).unwrap();
    assert_eq!(values[2], PayloadDefault::Scalar(i64::MIN.to_string()));
    assert_eq!(plan.encode_values(&values).unwrap(), input());
    let mut empty = vec![1];
    empty.extend(0_i32.to_le_bytes());
    empty.extend(i64::MAX.to_le_bytes());
    let values = plan.decode_values::<PayloadDefault>(&empty).unwrap();
    assert_eq!(plan.encode_values(&values).unwrap(), empty);
    assert_eq!(
        plan.encode_values(&values[..2]),
        Err(PayloadError::InvalidValue)
    );
}

#[test]
fn invalid_schema_defaults_and_ambiguous_paths_are_rejected() {
    let mut schema = schema();
    schema.params[0].default = Some(PayloadDefault::Scalar("2".into()));
    assert!(PayloadPlan::new(&schema).is_err());
    schema.params[0].default = Some(PayloadDefault::Scalar("true".into()));
    assert!(PayloadPlan::new(&schema).is_ok());
    schema.params[0].name = "notes.gain".into();
    assert!(PayloadPlan::new(&schema).is_err());
}

mod allocation_check {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! { static ALLOCATIONS: Cell<usize> = const { Cell::new(0) }; }
    struct TrackingAllocator;
    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            unsafe { System.realloc(pointer, layout, size) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: TrackingAllocator = TrackingAllocator;

    #[test]
    fn prepared_host_encoding_and_transfer_do_not_allocate() {
        let plan = PayloadPlan::new(&schema()).unwrap();
        let wire = input();
        let values = plan.decode_values::<PayloadDefault>(&wire).unwrap();
        let mut encoder = PayloadEncoder::new(plan.clone(), 128).unwrap();
        let mut workspace = Workspace([0; 128]);
        let mut output = [0; 128];
        let before = ALLOCATIONS.with(Cell::get);
        for _ in 0..100 {
            let encoded = encoder.encode(&values).unwrap();
            let prepared = plan.prepare(encoded, &mut workspace.0).unwrap();
            prepared.encode(&mut output).unwrap();
            assert_eq!(&output[..wire.len()], wire);
        }
        assert_eq!(ALLOCATIONS.with(Cell::get), before);
    }
}
