//! Contract spec integrity check (DR-0032).
//!
//! A Rust type alias such as `pub type Bps = u32` used inside a
//! `#[contracttype]` or `#[contractevent]` is recorded in the generated spec as
//! a user-defined type reference named after the alias, with no matching
//! definition. In-host tests never notice, but the Stellar CLI fails to parse
//! the argument ("Missing Entry Bps"). This crate walks every type reference in
//! a spec and reports each `Udt` name that the same spec does not define.

use std::collections::BTreeSet;

use stellar_xdr::curr::{ScSpecEntry, ScSpecTypeDef, ScSpecUdtUnionCaseV0};

/// One user-defined type reference with no definition in the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dangling {
    /// The undefined type name, e.g. `Bps`.
    pub name: String,
    /// Where it is referenced, e.g. `struct IndexParams.trigger_ratio_bps`.
    pub site: String,
}

/// Returns every dangling user-defined type reference in `entries`, in spec
/// order. An empty result means the spec is self-contained.
pub fn dangling_udts(entries: &[ScSpecEntry]) -> Vec<Dangling> {
    let defined: BTreeSet<String> = entries.iter().filter_map(defined_name).collect();
    let mut out = Vec::new();
    for entry in entries {
        for (site, ty) in type_refs(entry) {
            walk(ty, &mut |name| {
                if !defined.contains(name) {
                    out.push(Dangling {
                        name: name.to_string(),
                        site: site.clone(),
                    });
                }
            });
        }
    }
    out
}

/// Reads the spec from a contract wasm and checks it.
pub fn check_wasm(wasm: &[u8]) -> Result<Vec<Dangling>, soroban_spec::read::FromWasmError> {
    Ok(dangling_udts(&soroban_spec::read::from_wasm(wasm)?))
}

fn defined_name(entry: &ScSpecEntry) -> Option<String> {
    match entry {
        ScSpecEntry::UdtStructV0(s) => Some(s.name.to_utf8_string_lossy()),
        ScSpecEntry::UdtUnionV0(u) => Some(u.name.to_utf8_string_lossy()),
        ScSpecEntry::UdtEnumV0(e) => Some(e.name.to_utf8_string_lossy()),
        ScSpecEntry::UdtErrorEnumV0(e) => Some(e.name.to_utf8_string_lossy()),
        ScSpecEntry::FunctionV0(_) | ScSpecEntry::EventV0(_) => None,
    }
}

/// Every top level type position in an entry, labelled for the report.
fn type_refs(entry: &ScSpecEntry) -> Vec<(String, &ScSpecTypeDef)> {
    match entry {
        ScSpecEntry::FunctionV0(f) => {
            let name = f.name.0.to_utf8_string_lossy();
            let inputs = f.inputs.iter().map(|i| {
                let site = format!("fn {name}({})", i.name.to_utf8_string_lossy());
                (site, &i.type_)
            });
            let outputs = f
                .outputs
                .iter()
                .map(|o| (format!("fn {name} -> output"), o));
            inputs.chain(outputs).collect()
        }
        ScSpecEntry::UdtStructV0(s) => {
            let name = s.name.to_utf8_string_lossy();
            s.fields
                .iter()
                .map(|f| {
                    (
                        format!("struct {name}.{}", f.name.to_utf8_string_lossy()),
                        &f.type_,
                    )
                })
                .collect()
        }
        ScSpecEntry::UdtUnionV0(u) => {
            let name = u.name.to_utf8_string_lossy();
            u.cases
                .iter()
                .filter_map(|c| match c {
                    ScSpecUdtUnionCaseV0::TupleV0(t) => Some(t),
                    ScSpecUdtUnionCaseV0::VoidV0(_) => None,
                })
                .flat_map(|t| {
                    let case = format!("union {name}::{}", t.name.to_utf8_string_lossy());
                    t.type_.iter().map(move |ty| (case.clone(), ty))
                })
                .collect()
        }
        ScSpecEntry::EventV0(e) => {
            let name = e.name.0.to_utf8_string_lossy();
            e.params
                .iter()
                .map(|p| {
                    (
                        format!("event {name}.{}", p.name.to_utf8_string_lossy()),
                        &p.type_,
                    )
                })
                .collect()
        }
        ScSpecEntry::UdtEnumV0(_) | ScSpecEntry::UdtErrorEnumV0(_) => Vec::new(),
    }
}

/// Calls `on_udt` for every `Udt` name nested anywhere inside `ty`.
fn walk(ty: &ScSpecTypeDef, on_udt: &mut impl FnMut(&str)) {
    match ty {
        ScSpecTypeDef::Udt(u) => on_udt(&u.name.to_utf8_string_lossy()),
        ScSpecTypeDef::Option(o) => walk(&o.value_type, on_udt),
        ScSpecTypeDef::Result(r) => {
            walk(&r.ok_type, on_udt);
            walk(&r.error_type, on_udt);
        }
        ScSpecTypeDef::Vec(v) => walk(&v.element_type, on_udt),
        ScSpecTypeDef::Map(m) => {
            walk(&m.key_type, on_udt);
            walk(&m.value_type, on_udt);
        }
        ScSpecTypeDef::Tuple(t) => t.value_types.iter().for_each(|v| walk(v, on_udt)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        ScSpecFunctionInputV0, ScSpecFunctionV0, ScSpecTypeOption, ScSpecTypeUdt,
        ScSpecUdtStructFieldV0, ScSpecUdtStructV0, ScSpecUdtUnionCaseTupleV0, ScSpecUdtUnionV0,
    };

    fn udt(name: &str) -> ScSpecTypeDef {
        ScSpecTypeDef::Udt(ScSpecTypeUdt {
            name: name.try_into().unwrap(),
        })
    }

    fn field(name: &str, type_: ScSpecTypeDef) -> ScSpecUdtStructFieldV0 {
        ScSpecUdtStructFieldV0 {
            doc: "".try_into().unwrap(),
            name: name.try_into().unwrap(),
            type_,
        }
    }

    fn strukt(name: &str, fields: Vec<ScSpecUdtStructFieldV0>) -> ScSpecEntry {
        ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
            doc: "".try_into().unwrap(),
            lib: "".try_into().unwrap(),
            name: name.try_into().unwrap(),
            fields: fields.try_into().unwrap(),
        })
    }

    fn create_index(param: ScSpecTypeDef) -> ScSpecEntry {
        ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
            doc: "".try_into().unwrap(),
            name: "create_index".try_into().unwrap(),
            inputs: vec![ScSpecFunctionInputV0 {
                doc: "".try_into().unwrap(),
                name: "params".try_into().unwrap(),
                type_: param,
            }]
            .try_into()
            .unwrap(),
            outputs: vec![ScSpecTypeDef::U64].try_into().unwrap(),
        })
    }

    #[test]
    fn self_contained_spec_passes() {
        let spec = vec![
            strukt(
                "IndexParams",
                vec![field("trigger_ratio_bps", ScSpecTypeDef::U32)],
            ),
            create_index(udt("IndexParams")),
        ];
        assert!(dangling_udts(&spec).is_empty());
    }

    /// The pre DR-0032 trigger-engine shape: `trigger_ratio_bps: Bps` where
    /// `Bps` is a type alias that the spec never defines.
    #[test]
    fn alias_field_is_reported() {
        let spec = vec![
            strukt("IndexParams", vec![field("trigger_ratio_bps", udt("Bps"))]),
            create_index(udt("IndexParams")),
        ];
        assert_eq!(
            dangling_udts(&spec),
            vec![Dangling {
                name: "Bps".into(),
                site: "struct IndexParams.trigger_ratio_bps".into(),
            }]
        );
    }

    #[test]
    fn nested_and_union_references_are_reported() {
        let spec = vec![
            ScSpecEntry::UdtUnionV0(ScSpecUdtUnionV0 {
                doc: "".try_into().unwrap(),
                lib: "".try_into().unwrap(),
                name: "TriggerStatus".try_into().unwrap(),
                cases: vec![ScSpecUdtUnionCaseV0::TupleV0(ScSpecUdtUnionCaseTupleV0 {
                    doc: "".try_into().unwrap(),
                    name: "Finalized".try_into().unwrap(),
                    type_: vec![ScSpecTypeDef::U64, udt("Bps")].try_into().unwrap(),
                })]
                .try_into()
                .unwrap(),
            }),
            create_index(ScSpecTypeDef::Option(Box::new(ScSpecTypeOption {
                value_type: Box::new(udt("Missing")),
            }))),
        ];
        let names: Vec<_> = dangling_udts(&spec).into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["Bps", "Missing"]);
    }
}
