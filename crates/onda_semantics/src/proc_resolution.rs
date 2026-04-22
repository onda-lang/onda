use std::collections::HashSet;

use crate::*;

pub(crate) fn specialized_proc_template_bases(proc_symbols: &HashSet<String>) -> HashSet<String> {
    proc_symbols
        .iter()
        .filter_map(|name| {
            name.rsplit_once(".__gen__")
                .map(|(base, _)| base.to_owned())
        })
        .collect()
}

pub(crate) fn resolve_proc_ctor_symbol_name(
    ctor_name: &str,
    current_ns: &str,
    proc_symbols: &HashSet<String>,
) -> Option<String> {
    let direct = if ctor_name.contains("::") {
        proc_symbols
            .contains(ctor_name)
            .then_some(ctor_name.to_owned())
    } else {
        resolve_unqualified_symbol_name(ctor_name, current_ns, proc_symbols)
    };
    if direct.is_some() {
        return direct;
    }

    let resolved_base = if ctor_name.contains("::") {
        ctor_name.to_owned()
    } else {
        let template_bases = specialized_proc_template_bases(proc_symbols);
        resolve_unqualified_symbol_name(ctor_name, current_ns, &template_bases)?
    };
    let prefix = format!("{resolved_base}.__gen__");
    let mut matches = proc_symbols
        .iter()
        .filter(|name| name.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

pub(crate) fn resolve_specialized_proc_ctor_name(
    ctor_name: &str,
    type_args: &[CallTypeArg],
    current_ns: &str,
    proc_symbols: &HashSet<String>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    if type_args.is_empty() {
        return None;
    }

    let resolved_type_args = resolve_explicit_call_type_args(
        type_args,
        &format!("processor constructor '{}'", ctor_name),
        diag,
        errors,
    )?;

    let resolved_base = if ctor_name.contains("::") {
        ctor_name.to_owned()
    } else {
        let template_bases = specialized_proc_template_bases(proc_symbols);
        resolve_unqualified_symbol_name(ctor_name, current_ns, &template_bases)?
    };

    let specialized = specialized_struct_name(&resolved_base, &resolved_type_args);
    proc_symbols.contains(&specialized).then_some(specialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc_symbols(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn resolve_proc_ctor_symbol_name_prefers_unqualified_match_in_namespace() {
        let proc_symbols = proc_symbols(&["synth::Voice"]);

        assert_eq!(
            resolve_proc_ctor_symbol_name("Voice", "synth", &proc_symbols).as_deref(),
            Some("synth::Voice")
        );
    }

    #[test]
    fn resolve_proc_ctor_symbol_name_accepts_single_specialized_template_match() {
        let proc_symbols = proc_symbols(&["synth::Voice.__gen__f32"]);

        assert_eq!(
            resolve_proc_ctor_symbol_name("Voice", "synth", &proc_symbols).as_deref(),
            Some("synth::Voice.__gen__f32")
        );
    }

    #[test]
    fn resolve_proc_ctor_symbol_name_rejects_ambiguous_specialized_template_match() {
        let proc_symbols = proc_symbols(&["synth::Voice.__gen__f32", "synth::Voice.__gen__f64"]);

        assert_eq!(
            resolve_proc_ctor_symbol_name("Voice", "synth", &proc_symbols),
            None
        );
    }

    #[test]
    fn resolve_specialized_proc_ctor_name_maps_explicit_type_args_to_specialization() {
        let proc_symbols = proc_symbols(&["synth::Voice.__gen__f64"]);
        let mut errors = Vec::new();

        assert_eq!(
            resolve_specialized_proc_ctor_name(
                "Voice",
                &[CallTypeArg::Primitive(PrimitiveType::F64)],
                "synth",
                &proc_symbols,
                DiagCtx::default(),
                &mut errors,
            )
            .as_deref(),
            Some("synth::Voice.__gen__f64")
        );
        assert!(errors.is_empty());
    }
}
