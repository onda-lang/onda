//! Keep runtime default dependencies as a call graph, rather than an expanded tree.
use super::*;

struct DefaultSignature {
    params: Vec<String>,
    defaults: Vec<Option<Expr>>,
}

pub(super) struct RuntimeDefaults<'a> {
    signatures: HashMap<String, DefaultSignature>,
    structs: &'a HashMap<String, Vec<TypedStructField>>,
}

impl<'a> RuntimeDefaults<'a> {
    pub(super) fn new(
        defs: &[TypedFunction],
        structs: &'a HashMap<String, Vec<TypedStructField>>,
    ) -> Self {
        Self {
            signatures: defs
                .iter()
                .filter(|def| def.param_defaults.iter().any(Option::is_some))
                .map(|def| {
                    (
                        def.name.clone(),
                        DefaultSignature {
                            params: def.params.clone(),
                            defaults: def.param_defaults.clone(),
                        },
                    )
                })
                .collect(),
            structs,
        }
    }

    pub(super) fn expand(&self, expr: &mut Expr, errors: &mut Vec<Diagnostic>) {
        expr.visit_mut(|expr| {
            crate::data_construction::expand_constructor_defaults(expr, self.structs);
            let Expr::UserCall {
                name, args, loc, ..
            } = expr
            else {
                return true;
            };
            let Some(signature) = self.signatures.get(name) else {
                return true;
            };
            let resolved = resolve_call_args_at(
                args,
                &signature.params,
                &signature.defaults,
                signature.params.first().is_some_and(|name| name == "self"),
                false,
                &format!("function '{name}' call"),
                (*loc).into(),
                errors,
            );
            let omitted = resolved
                .iter()
                .enumerate()
                .filter_map(|(index, value)| value.is_none().then_some(index))
                .collect::<Vec<_>>();
            for index in omitted {
                if let Some(default) = &signature.defaults[index] {
                    args.push(CallArg {
                        name: Some(signature.params[index].clone()),
                        expr: default.clone(),
                    });
                }
            }
            true
        });
    }
}

/// A default containing nested omitted runtime defaults gets one parameterless
/// helper. Leaf defaults stay inline and fold normally. Helpers use the ordinary
/// caller-context worklist: sharing code never shares execution or changes
/// argument evaluation order.
pub(super) fn share_runtime_defaults(defs: &mut Vec<TypedFunction>) {
    let indices = defs
        .iter()
        .enumerate()
        .map(|(index, def)| (def.name.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut shared = Vec::new();
    for (owner, def) in defs.iter().enumerate() {
        for (parameter, default) in def.param_defaults.iter().enumerate() {
            let Some(default) = default else { continue };
            if default.walk().any(|expr| {
                let Expr::UserCall {
                    name, args, loc, ..
                } = expr
                else {
                    return false;
                };
                let Some(&index) = indices.get(name.as_str()) else {
                    return false;
                };
                let callee = &defs[index];
                if !callee.param_defaults.iter().any(Option::is_some) {
                    return false;
                }
                let supplied = resolve_call_args_at(
                    args,
                    &callee.params,
                    &callee.param_defaults,
                    callee.params.first().is_some_and(|name| name == "self"),
                    false,
                    "function call",
                    (*loc).into(),
                    &mut Vec::new(),
                );
                supplied
                    .iter()
                    .zip(&callee.param_defaults)
                    .any(|(supplied, default)| supplied.is_none() && default.is_some())
            }) {
                shared.push((owner, parameter));
            }
        }
    }
    let mut helpers = Vec::with_capacity(shared.len());
    for (owner, index) in shared {
        let def = &mut defs[owner];
        let expr = def.param_defaults[index].as_mut().unwrap();
        let return_ty = def.param_kinds[index]
            .default_return_type()
            .expect("default parameter types were validated");
        let name = format!("{}.__onda_default_{index}", def.name);
        let loc = expr.loc();
        let body = vec![Stmt::Return {
            loc: loc.span(),
            expr: std::mem::replace(
                expr,
                Expr::UserCall {
                    loc: loc.span(),
                    name: name.clone(),
                    args: Vec::new(),
                    type_args: Vec::new(),
                },
            ),
        }];
        helpers.push(TypedFunction {
            name,
            compile_context: def.compile_context,
            runtime_context: false,
            publishes_print: false,
            method_of: None,
            type_params: Vec::new(),
            params: Vec::new(),
            param_defaults: Vec::new(),
            param_kinds: Vec::new(),
            readonly_data_params: HashSet::new(),
            integer_range_params: HashMap::new(),
            return_ty,
            returns_value: true,
            body,
        });
    }
    defs.extend(helpers);
}
