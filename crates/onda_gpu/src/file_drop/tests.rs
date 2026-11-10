use super::*;

fn ui() -> RunUi {
    let mut ui = RunUi::default();
    ui.resize(720.0, 840.0, 1.0);
    ui.message(
        json!({"type":"state","state":{"path":"sample.onda","supportsScope":false,
        "buffers":[{"name":"audio","type":"f32"}]}}),
    );
    assert_eq!(ui.buffer_target(42.0, 150.0).as_deref(), Some("audio"));
    ui
}
fn offer(id: i64) -> Offer {
    Offer {
        id: DataTransferId::from_raw(id),
        request: Some(AsyncRequestSerial::get()),
        position: Some([42.0, 150.0]),
        paths: None,
        buffer: None,
        accepted: false,
    }
}
fn assert_dropped(response: Response, paths: &[PathBuf]) {
    let Response::Dropped {
        paths: actual,
        buffer,
    } = response
    else {
        panic!("drop was lost");
    };
    assert_eq!(actual, paths);
    assert_eq!(buffer.as_deref(), Some("audio"));
}

#[test]
fn asynchronous_file_delivery_retains_the_drop_target_after_leaving_and_a_new_drag() {
    let mut ui = ui();
    let first = offer(1);
    let (id, request) = (first.id, first.request.unwrap());
    let mut drag = FileDrop {
        drag: Some(first),
        ..Default::default()
    };
    assert!(matches!(drag.drop(id, &mut ui), Response::Handled));
    drag.leave(&mut ui);
    drag.drag = Some(offer(2));
    ui.message(json!({"type":"state","state":{"buffers":[]}}));
    let paths = vec![PathBuf::from("sample.wav")];
    assert_dropped(drag.receive(id, request, paths.clone()), &paths);
    assert!(
        drag.drag.is_some(),
        "a pending drop must not consume the next drag"
    );
    assert!(
        matches!(drag.receive(id, request, paths), Response::Handled),
        "duplicate delivery must not bind twice"
    );
}

#[test]
fn file_delivery_before_the_drop_uses_the_same_target() {
    let mut ui = ui();
    let first = offer(1);
    let (id, request) = (first.id, first.request.unwrap());
    let mut drag = FileDrop {
        drag: Some(first),
        ..Default::default()
    };
    let paths = vec![PathBuf::from("sample.wav")];
    assert!(matches!(
        drag.receive(id, request, paths.clone()),
        Response::Handled
    ));
    assert_dropped(drag.drop(id, &mut ui), &paths);
    assert!(matches!(drag.drop(id, &mut ui), Response::Handled));
}

#[test]
fn leaving_before_dropping_or_unloading_discards_late_file_delivery() {
    let mut ui = ui();
    for dropped in [false, true] {
        let first = offer(1);
        let (id, request) = (first.id, first.request.unwrap());
        let mut drag = FileDrop {
            drag: Some(first),
            ..Default::default()
        };
        if dropped {
            drag.drop(id, &mut ui);
            drag.cancel(&mut ui);
        } else {
            drag.leave(&mut ui);
        }
        assert!(matches!(
            drag.receive(id, request, vec!["sample.wav".into()]),
            Response::Handled
        ));
        assert!(drag.drag.is_none());
        assert!(drag.pending.is_empty());
    }
}

#[test]
fn stale_file_responses_cannot_replace_a_new_offer() {
    let second = offer(1);
    let request = second.request.unwrap();
    let mut drag = FileDrop {
        drag: Some(second),
        ..Default::default()
    };
    for (id, serial) in [
        (DataTransferId::from_raw(2), request),
        (DataTransferId::from_raw(1), AsyncRequestSerial::get()),
    ] {
        drag.receive(id, serial, vec!["wrong.wav".into()]);
        assert!(drag.drag.as_ref().unwrap().paths.is_none());
    }
}

#[test]
fn file_uris_decode_spaces_unicode_and_reject_non_file_or_mixed_offers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("a space è.wav");
    let uri = url::Url::from_file_path(&path).unwrap().to_string();
    assert_eq!(file_paths(vec![uri.clone()]), [path]);
    for invalid in ["https://example.com/file.wav", "not a URI"] {
        assert!(file_paths(vec![uri.clone(), invalid.into()]).is_empty());
    }
    // Windows also permits native UNC file paths.
    #[cfg(not(windows))]
    assert!(file_paths(vec!["file://remote/file.wav".into()]).is_empty());
    assert!(single_file(&["a.wav".into(), "b.wav".into()]).is_none());
    assert_eq!(
        single_file(&["a.wav".into(), "a.wav".into()]),
        Some(Path::new("a.wav"))
    );
}

#[cfg(target_os = "linux")]
#[path = "native_x11_tests.rs"]
mod native_x11;
