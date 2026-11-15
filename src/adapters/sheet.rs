//! The run sheet: a spreadsheet layout any site can keep, one row per run and its results.
//!
//! Export the sheet as CSV and `owg import run-sheet` turns it into records.
//! Units live in the column headers, so a column cannot change unit halfway down.
//!
//! | Column | Meaning |
//! |---|---|
//! | `unit` | material unit id (required) |
//! | `unit_kind`, `parent`, `substrate`, `site` | its level (`chip` by default), parent id, starting material, fab site |
//! | `recipe`, `recipe_name`, `recipe_version`, `step_kind`, `tool_class` | the recipe this run follows |
//! | `run`, `tool`, `step`, `start`, `end`, `operator_role`, `status` | the run (RFC 3339 times, a role, never a name) |
//! | `set:<name> [<unit>]` | a setpoint: a recipe parameter when `recipe` is given, else a run setting |
//! | `rattr:<name>`, `attr:<name>` | recipe and run attributes (text) |
//! | `meas:<p> [<unit>]` | a measured value of parameter `<p>` on `unit`, after `run` |
//! | `u:<p>`, `k:<p>`, `u_basis:<p>`, `u_reason:<p>` | its standard (or expanded, with `k`) uncertainty and basis; with no `u`, `u_reason` says why it is unknown |
//! | `method:<p>`, `mtool:<p>`, `cond:<p>:<name> [<unit>]` | how it was measured, on what, under which conditions |
//! | `source`, `source_version`, `locator`, `license`, `attribution`, `origin`, `note` | provenance and licence for every record the row makes |
//!
//! A value whose header has no `[unit]` is imported without a unit and the validator refuses
//! it. A recipe defined on two rows with different setpoints is an import error: give the
//! second a new `recipe_version` and id.

use super::{kind_for, slug};
use crate::record::*;

fn split_header(h: &str) -> (String, Option<String>) {
    let h = h.trim();
    match (h.rfind('['), h.ends_with(']')) {
        (Some(i), true) => (
            h[..i].trim().to_string(),
            Some(super::unit_alias(&h[i + 1..h.len() - 1])),
        ),
        _ => (h.to_string(), None),
    }
}

fn kind_of(s: &str) -> Result<UnitKind, String> {
    Ok(match s.trim().to_lowercase().as_str() {
        "" | "chip" => UnitKind::Chip,
        "lot" => UnitKind::Lot,
        "wafer" => UnitKind::Wafer,
        "substrate" => UnitKind::Substrate,
        "die" => UnitKind::Die,
        "test_site" | "site" | "device" => UnitKind::TestSite,
        o => return Err(format!("unknown unit_kind '{o}'")),
    })
}

/// Parse a run sheet (CSV text) into a dataset declared `declared`.
pub fn run_sheet(
    text: &str,
    dataset_id: &str,
    title: &str,
    declared: DeclaredOrigin,
) -> Result<Dataset, String> {
    let recs = crate::columnar::parse_csv(text)?;
    let mut it = recs
        .into_iter()
        .filter(|r| !r.iter().all(|f| f.0.trim().is_empty()));
    let header: Vec<String> = it
        .next()
        .ok_or("empty sheet")?
        .into_iter()
        .map(|f| f.0.trim().to_string())
        .collect();
    let cols: Vec<(String, Option<String>)> = header.iter().map(|h| split_header(h)).collect();
    let mut ds = Dataset::new(Header::new(dataset_id, title, declared));
    for (n, r) in it.enumerate() {
        let line = n + 2;
        let cell = |name: &str| -> Option<String> {
            cols.iter()
                .position(|c| c.0 == name)
                .and_then(|j| r.get(j))
                .map(|f| f.0.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let req = |name: &str| cell(name).ok_or(format!("row {line}: '{name}' is required"));
        let origin = match cell("origin")
            .as_deref()
            .unwrap_or("measured")
            .to_lowercase()
            .as_str()
        {
            "measured" => Origin::Measured,
            "specified" => Origin::Specified,
            "simulated" => Origin::Simulated,
            o => return Err(format!("row {line}: origin '{o}'")),
        };
        let prov = |o: Origin| -> Option<Provenance> {
            cell("source").map(|s| Provenance {
                origin: o,
                source: s,
                source_version: cell("source_version"),
                locator: cell("locator").or(Some(format!("sheet row {line}"))),
                produced_by: format!("owg {} sheet::run_sheet", env!("CARGO_PKG_VERSION")),
                retrieved: cell("retrieved"),
                note: cell("note"),
            })
        };
        let lic = || {
            cell("license").map(|s| License {
                spdx: s,
                attribution: cell("attribution"),
                url: None,
            })
        };
        let unit = req("unit")?;
        if !ds.units.iter().any(|u| u.id == unit) {
            ds.units.push(MaterialUnit {
                id: unit.clone(),
                kind: kind_of(cell("unit_kind").as_deref().unwrap_or(""))?,
                parent: cell("parent"),
                label: cell("unit_label"),
                site: cell("site"),
                substrate: cell("substrate").map(|m| Substrate {
                    material: m,
                    dopant_type: None,
                    orientation: None,
                    properties: vec![],
                }),
                position: None,
                provenance: prov(origin),
                license: lic(),
            });
        }
        // Setpoints and attributes.
        let mut sets = Vec::new();
        let mut rattrs = Vec::new();
        let mut attrs = Vec::new();
        for (j, (name, unit_h)) in cols.iter().enumerate() {
            let Some(v) = r.get(j).map(|f| f.0.trim()).filter(|s| !s.is_empty()) else {
                continue;
            };
            if let Some(p) = name.strip_prefix("set:") {
                let value: f64 = v
                    .parse()
                    .map_err(|_| format!("row {line}: {name} = '{v}' is not a number"))?;
                sets.push(Parameter {
                    name: p.to_string(),
                    kind: unit_h.as_deref().and_then(|u| kind_for(p, u)),
                    quantity: Quantity {
                        value,
                        unit: unit_h.clone(),
                        uncertainty: Some(Uncertainty::Setpoint { tolerance: None }),
                        derivation: None,
                    },
                });
            } else if let Some(p) = name.strip_prefix("rattr:") {
                rattrs.push(Attribute {
                    name: p.into(),
                    value: v.into(),
                });
            } else if let Some(p) = name.strip_prefix("attr:") {
                attrs.push(Attribute {
                    name: p.into(),
                    value: v.into(),
                });
            }
        }
        let recipe = cell("recipe");
        if let Some(rid) = &recipe {
            match ds.recipes.iter().find(|x| &x.id == rid) {
                Some(existing) if existing.parameters != sets => {
                    return Err(format!("row {line}: recipe '{rid}' was defined with other setpoints on an earlier row; give this one a new id and recipe_version"))
                }
                Some(_) => {}
                None => ds.recipes.push(Recipe {
                    id: rid.clone(),
                    name: cell("recipe_name").unwrap_or_else(|| rid.clone()),
                    version: cell("recipe_version").unwrap_or_else(|| "1".into()),
                    supersedes: cell("supersedes"),
                    step_kind: cell("step_kind"),
                    tool_class: cell("tool_class"),
                    parameters: sets.clone(),
                    attributes: rattrs,
                    provenance: prov(if origin == Origin::Simulated { Origin::Simulated } else { Origin::Specified }),
                    license: lic(),
                }),
            }
        }
        let run = cell("run");
        if let Some(run_id) = &run {
            if !ds.runs.iter().any(|x| &x.id == run_id) {
                let status = match cell("status")
                    .as_deref()
                    .unwrap_or("unknown")
                    .to_lowercase()
                    .as_str()
                {
                    "completed" => RunStatus::Completed,
                    "aborted" => RunStatus::Aborted,
                    "faulted" => RunStatus::Faulted,
                    _ => RunStatus::Unknown,
                };
                ds.runs.push(ProcessRun {
                    id: run_id.clone(),
                    recipe: recipe.clone(),
                    tool: cell("tool").unwrap_or_else(|| "not recorded".into()),
                    chamber: cell("chamber"),
                    route: cell("route"),
                    step_no: cell("step_no").and_then(|s| s.parse().ok()),
                    step_name: cell("step"),
                    material: vec![unit.clone()],
                    start: cell("start"),
                    end: cell("end"),
                    operator_role: cell("operator_role"),
                    status,
                    settings: if recipe.is_none() {
                        sets.clone()
                    } else {
                        vec![]
                    },
                    actuals: vec![],
                    attributes: attrs,
                    provenance: prov(origin),
                    license: lic(),
                });
            }
        }
        // Measurements.
        for (name, unit_h) in &cols {
            let Some(p) = name.strip_prefix("meas:") else {
                continue;
            };
            let Some(raw) = cell(name) else { continue };
            let value: f64 = raw
                .parse()
                .map_err(|_| format!("row {line}: {name} = '{raw}' is not a number"))?;
            let u = cell(&format!("u:{p}"))
                .map(|s| {
                    s.parse::<f64>()
                        .map_err(|_| format!("row {line}: u:{p} = '{s}'"))
                })
                .transpose()?;
            let k = cell(&format!("k:{p}")).and_then(|s| s.parse::<f64>().ok());
            let basis = cell(&format!("u_basis:{p}"));
            let uncertainty = match (u, k) {
                (Some(u), Some(k)) => Some(Uncertainty::Expanded { u, k, basis }),
                (Some(u), None) => Some(Uncertainty::Standard { u, basis }),
                (None, _) => {
                    cell(&format!("u_reason:{p}")).map(|reason| Uncertainty::Unknown { reason })
                }
            };
            let mut conditions = Vec::new();
            for (cn, cu) in &cols {
                let Some(rest) = cn.strip_prefix(&format!("cond:{p}:")) else {
                    continue;
                };
                let Some(cv) = cell(cn) else { continue };
                let v: f64 = cv
                    .parse()
                    .map_err(|_| format!("row {line}: {cn} = '{cv}'"))?;
                conditions.push(Parameter {
                    name: rest.into(),
                    kind: cu.as_deref().and_then(|u| kind_for(rest, u)),
                    quantity: Quantity {
                        value: v,
                        unit: cu.clone(),
                        uncertainty: Some(Uncertainty::Setpoint { tolerance: None }),
                        derivation: None,
                    },
                });
            }
            ds.metrology.push(Metrology {
                id: format!(
                    "{}/{}",
                    run.clone().unwrap_or_else(|| unit.clone()),
                    slug(p)
                ),
                material: unit.clone(),
                after_run: run.clone(),
                method: cell(&format!("method:{p}")).unwrap_or_else(|| "not recorded".into()),
                tool: cell(&format!("mtool:{p}")),
                parameter: p.to_string(),
                kind: unit_h.as_deref().and_then(|u| kind_for(p, u)),
                points: vec![MetroPoint {
                    site: None,
                    position: None,
                    value: Quantity {
                        value,
                        unit: unit_h.clone(),
                        uncertainty,
                        derivation: None,
                    },
                }],
                conditions,
                attributes: vec![],
                at: cell("measured_at"),
                provenance: prov(origin),
                license: lic(),
            });
        }
    }
    Ok(ds)
}

/// Column names of a run sheet with the given setpoints and measurements, for a template.
pub fn template_header(sets: &[(&str, &str)], meas: &[(&str, &str)]) -> String {
    let mut h: Vec<String> = [
        "unit",
        "unit_kind",
        "parent",
        "substrate",
        "site",
        "recipe",
        "recipe_name",
        "recipe_version",
        "step_kind",
        "tool_class",
        "run",
        "tool",
        "step",
        "start",
        "end",
        "operator_role",
        "status",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (n, u) in sets {
        h.push(format!("set:{n} [{u}]"));
    }
    for (n, u) in meas {
        h.push(format!("meas:{n} [{u}]"));
        h.push(format!("u:{n}"));
        h.push(format!("u_basis:{n}"));
        h.push(format!("u_reason:{n}"));
        h.push(format!("method:{n}"));
    }
    h.extend(
        [
            "source",
            "source_version",
            "locator",
            "license",
            "attribution",
            "origin",
            "note",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    h.join(",")
}
