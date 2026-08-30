use super::*;

#[test]
fn hosted_delegate_batch_is_independent_and_decodes_occurrences() {
    let mut storage = [0_u8; 32];
    storage[..4].copy_from_slice(&3_u32.to_ne_bytes());
    storage[4..8].copy_from_slice(&4_u32.to_ne_bytes());
    storage[8..12].copy_from_slice(&5_u32.to_ne_bytes());
    storage[12..16].copy_from_slice(&17_i32.to_ne_bytes());
    storage[16..20].copy_from_slice(&4_u32.to_ne_bytes());
    storage[20..24].copy_from_slice(&4_u32.to_ne_bytes());
    storage[24..28].copy_from_slice(&8_u32.to_ne_bytes());
    storage[28..].copy_from_slice(&23_i32.to_ne_bytes());
    let mut batch = onda_delegate_batch_t {
        storage: storage.as_mut_ptr(),
        capacity_bytes: storage.len() as u32,
        used_bytes: storage.len() as u32,
        record_count: 2,
        overflow_count: 2,
    };
    let mut occurrence = onda_delegate_occurrence_t {
        delegate_index: 0,
        payload_size_bytes: 0,
        sequence: 0,
        payload: ptr::null(),
    };

    let mut cursor = onda_batch_cursor_t::default();
    assert_eq!(
        unsafe { onda_delegate_batch_next(&batch, &mut cursor, &mut occurrence) },
        1
    );
    assert_eq!(occurrence.delegate_index, 3);
    assert_eq!(occurrence.payload_size_bytes, 4);
    assert_eq!(occurrence.sequence, 5);
    assert_eq!(
        unsafe { ptr::read_unaligned(occurrence.payload.cast::<i32>()) },
        17
    );
    assert_eq!(
        unsafe { onda_delegate_batch_next(&batch, &mut cursor, &mut occurrence) },
        1
    );
    assert_eq!(occurrence.delegate_index, 4);
    assert_eq!(occurrence.sequence, 8);
    assert_eq!(
        unsafe { ptr::read_unaligned(occurrence.payload.cast::<i32>()) },
        23
    );
    assert_eq!(
        unsafe { onda_delegate_batch_next(&batch, &mut cursor, &mut occurrence) },
        0
    );
    assert_eq!(
        unsafe { onda_delegate_batch_occurrence_at(&batch, 1, &mut occurrence) },
        1
    );

    unsafe { onda_delegate_batch_reset(&mut batch) };
    assert_eq!(
        (batch.used_bytes, batch.record_count, batch.overflow_count),
        (0, 0, 0)
    );
}

#[test]
fn owned_diagnostic_strings_escape_nul_and_dispose_idempotently() {
    let mut diagnostic = Diagnostic::internal("left\0right");
    diagnostic.file = Some("module.onda".into());
    diagnostic.trace = vec!["outer".into(), "inner".into()];
    let mut diagnostic = diag_to_c(&diagnostic);

    let message = unsafe { CStr::from_ptr(diagnostic.message) };
    assert_eq!(message.to_bytes(), b"left\\0right");
    let file = unsafe { CStr::from_ptr(diagnostic.file) };
    assert_eq!(file.to_bytes(), b"module.onda");
    let trace = unsafe { CStr::from_ptr(diagnostic.trace) };
    assert_eq!(trace.to_bytes(), b"inner\nouter");

    unsafe {
        onda_diag_dispose(&mut diagnostic);
        onda_diag_dispose(&mut diagnostic);
    }
    assert!(diagnostic.message.is_null());
    assert!(diagnostic.file.is_null());
    assert!(diagnostic.trace.is_null());
}
