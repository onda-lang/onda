//! Conventional host-driven event surfaces shared by Onda tooling and hosts.

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum HostEventFamily {
    Midi,
    HostContext,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct HostEventParam {
    pub name: &'static str,
    pub type_repr: &'static str,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct HostEvent {
    pub name: &'static str,
    pub signature: &'static str,
    pub family: HostEventFamily,
    pub params: &'static [HostEventParam],
}

include!(concat!(env!("OUT_DIR"), "/events.rs"));

pub fn event_by_name(name: &str) -> Option<&'static HostEvent> {
    PLUGIN_MIDI_EVENTS
        .iter()
        .chain(PLUGIN_HOST_CONTEXT_EVENTS.iter())
        .find(|event| event.name == name)
}

pub fn signature_matches<'a>(
    event: &HostEvent,
    params: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> bool {
    let mut params = params.into_iter();
    event.params.iter().all(|expected| {
        params
            .next()
            .is_some_and(|actual| actual == (expected.name, expected.type_repr))
    }) && params.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::{event_by_name, signature_matches, HostEventFamily};

    #[test]
    fn signatures_require_exact_names_types_and_order() {
        let note_on = event_by_name("note_on").expect("canonical note_on");
        assert_eq!(note_on.family, HostEventFamily::Midi);
        assert!(signature_matches(
            note_on,
            [
                ("id", "i32"),
                ("channel", "i32"),
                ("key", "i32"),
                ("velocity", "f32"),
            ]
        ));
        assert!(!signature_matches(
            note_on,
            [
                ("channel", "i32"),
                ("id", "i32"),
                ("key", "i32"),
                ("velocity", "f32"),
            ]
        ));
    }
}
