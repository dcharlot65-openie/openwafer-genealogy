//! Adapters for the formats Hacker Fab's open tools and documents emit.
//!
//! Each function names the repository and file that defines its format. Inventory and
//! licences are in `hackerfab/SOURCES.md`.
//!
//! | Adapter | Format | Defined by |
//! |---|---|---|
//! | [`dm_dumpdata`] | Django `dumpdata` JSON of the data-management database | `hacker-fab/data-management` `data_management/models.py` |
//! | [`dm_search_csv`] | the portal's search export (tables separated by blank lines) | `data_management/views.py` `create_csv_include_process` |
//! | [`smu_pair`] | `{chip}_{device}_currents.csv` + `_voltages.csv` | `hacker-fab/smu` `smu.py` |
//! | [`smu_sweep_csv`] | SMU sweep export, `_Id,_Iteration,_Segment,_Elapsed,_DateTime,<name> (<unit>)…` | the four-point-probe file in `hacker-fab/gitbook` |
//! | [`impedance_csv`] | impedance-matcher telemetry | `hacker-fab/impedance-matcher` `firmware/telemetry.py` |
//! | [`p901_csv`] | MKS 901P reader log, `t_s,pressure` | `hacker-fab/uw-p901-reader` `901p-serial/main.py` |
//! | [`furnace_recipe`] | tube-furnace ramp/hold recipe JSON | `hacker-fab/uw-tube-furnace_pid` `furnace/recipe.py` |
//! | [`traveler`] | process traveler sheet: Step #, Step Name, Tool Name, Parameters (`Key: value unit` lines), Comments | `Sputtered_Gate_NMOS_S26.xlsx` in `hacker-fab/gitbook` |
//! | [`report_md`] | NMOS characterization reports: `N_x = …` yield lines and `<name> ± σ (<unit>)` tables | `nmos-capabilities/…/march-2026-update/group-*-report.md` |
//! | [`nmos_summary_csv`] | per-device extraction output `nmos_summary.csv` (+ per-gate-voltage file) | linked from the group-1 report |

use super::{kind_for, number_unit, rfc3339, setpoint, slug, snake, Source};
use crate::record::*;
use crate::units::QuantityKind;
use serde_json::Value;
use std::collections::BTreeMap;

// ───────────────────────────── SMU sweep export (four-point probe) ─────────────────────────────

/// Options for [`smu_sweep_csv`].
#[derive(Debug, Clone)]
pub struct SweepOpts {
    pub test_id: String,
    /// The material unit the probe sat on.
    pub material: String,
    /// "collinear 4-point probe".
    pub structure: String,
    /// Film thickness, to derive resistivity from sheet resistance. Optional.
    pub thickness: Option<Quantity>,
    /// Id of the record the thickness came from, for the derivation's inputs.
    pub thickness_ref: Option<String>,
}

fn header_name_unit(h: &str) -> (String, Option<String>) {
    let h = h.trim();
    if let (Some(a), true) = (h.rfind('('), h.ends_with(')')) {
        let unit = super::unit_alias(&h[a + 1..h.len() - 1]);
        return (h[..a].trim().to_string(), Some(unit));
    }
    (h.to_string(), None)
}

/// An SMU sweep exported with `_Id,_Iteration,_Segment,_Elapsed,_DateTime` and then
/// `<name> (<unit>)` columns. For a four-point-probe file (`Source I (A)`, `Measured V (V)`)
/// it also extracts sheet resistance `Rs = (π/ln 2)·mean(V/I)` (thin film, probe spacing much
/// smaller than the sample, insulating substrate: no correction factor), with a type-A
/// standard uncertainty over the readings, and resistivity `ρ = Rs·t` when a thickness is given.
pub fn smu_sweep_csv(text: &str, src: &Source, o: &SweepOpts) -> Result<Dataset, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines
        .next()
        .ok_or("empty file")?
        .trim_start_matches('\u{feff}')
        .split(',')
        .map(str::to_string)
        .collect();
    let cols: Vec<(String, Option<String>)> = header.iter().map(|h| header_name_unit(h)).collect();
    let idx = |name: &str| cols.iter().position(|c| c.0 == name);
    let mut data: Vec<Vec<String>> = Vec::new();
    for l in lines {
        data.push(l.split(',').map(|s| s.trim().to_string()).collect());
    }
    let num = |row: &Vec<String>, j: usize| row.get(j).and_then(|s| s.parse::<f64>().ok());
    let (ji, jv) = (
        cols.iter().position(|c| {
            c.0.to_lowercase().contains("source i") || c.0.to_lowercase().starts_with("current")
        }),
        cols.iter().position(|c| {
            c.0.to_lowercase().contains("measured v") || c.0.to_lowercase().starts_with("voltage")
        }),
    );
    let date = idx("_DateTime").and_then(|j| data.first().and_then(|r| r.get(j).cloned()));
    let mut ds = src.dataset(&format!("{}-ds", o.test_id), "SMU sweep");
    let mut curves = Vec::new();
    let mut extracted = Vec::new();
    if let (Some(ji), Some(jv)) = (ji, jv) {
        let (iu, vu) = (cols[ji].1.clone(), cols[jv].1.clone());
        let xs: Vec<Option<f64>> = data.iter().map(|r| num(r, ji)).collect();
        let ys: Vec<Option<f64>> = data.iter().map(|r| num(r, jv)).collect();
        curves.push(Curve {
            name: "V(I)".into(),
            x: Axis {
                name: cols[ji].0.clone(),
                kind: Some(QuantityKind::Current),
                unit: iu.clone(),
                values: xs.clone(),
                derivation: None,
            },
            y: Axis {
                name: cols[jv].0.clone(),
                kind: Some(QuantityKind::Voltage),
                unit: vu.clone(),
                values: ys.clone(),
                derivation: None,
            },
            fixed: vec![],
            uncertainty: Some(Uncertainty::Unknown {
                reason: "the instrument's accuracy is not recorded in the export".into(),
            }),
        });
        // Sheet resistance from V/I in SI.
        let to_si = |v: f64, u: &Option<String>| {
            u.as_deref()
                .and_then(|u| {
                    crate::units::convert(v, u, if u.contains('V') { "V" } else { "A" })
                })
                .unwrap_or(v)
        };
        let r: Vec<f64> = xs
            .iter()
            .zip(&ys)
            .filter_map(|(i, v)| Some(to_si((*v)?, &vu) / to_si((*i)?, &iu)))
            .filter(|x| x.is_finite())
            .collect();
        if r.len() >= 2 && iu.is_some() && vu.is_some() {
            let k = std::f64::consts::PI / std::f64::consts::LN_2;
            let n = r.len() as f64;
            let mean = r.iter().sum::<f64>() / n;
            let sd = (r.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
            let rs = k * mean;
            let u_rs = k * sd / n.sqrt();
            let rs_q = Quantity::derived(
                rs,
                "ohm/sq",
                Uncertainty::Standard {
                    u: u_rs,
                    basis: Some(format!("type A: s.d. of the {} V/I readings / √n, times π/ln 2; probe-spacing and edge corrections not applied", r.len())),
                },
                "Rs = (π/ln 2)·mean(V/I), collinear four-point probe on a thin film over an insulating substrate much wider than the probe spacing (no correction factor)",
                vec![format!("{}#curves[0]", o.test_id)],
            );
            extracted.push(Extracted {
                name: "sheet_resistance".into(),
                kind: Some(QuantityKind::SheetResistance),
                quantity: rs_q,
            });
            if let Some(t) = &o.thickness {
                if let Some(t_m) = t
                    .unit
                    .as_deref()
                    .and_then(|u| crate::units::convert(t.value, u, "m"))
                {
                    let mut inputs = vec![format!("{}#extracted[sheet_resistance]", o.test_id)];
                    if let Some(r) = &o.thickness_ref {
                        inputs.push(r.clone());
                    }
                    let unc = match &t.uncertainty {
                        Some(Uncertainty::Standard { u, .. }) => {
                            let ut = crate::units::convert(
                                *u,
                                t.unit.as_deref().unwrap_or("m"),
                                "m",
                            )
                            .unwrap_or(f64::NAN);
                            let rel = ((u_rs / rs).powi(2) + (ut / t_m).powi(2)).sqrt();
                            Uncertainty::Standard {
                                u: rel * rs * t_m,
                                basis: Some("Rs and thickness uncertainties in quadrature".into()),
                            }
                        }
                        _ => Uncertainty::Unknown {
                            reason: "the film thickness has no stated uncertainty".into(),
                        },
                    };
                    extracted.push(Extracted {
                        name: "resistivity".into(),
                        kind: Some(QuantityKind::Resistivity),
                        quantity: Quantity::derived(rs * t_m, "ohm.m", unc, "ρ = Rs · t", inputs),
                    });
                }
            }
        }
    }
    let mut attributes = vec![Attribute {
        name: "export_columns".into(),
        value: header.join(","),
    }];
    if let Some(d) = &date {
        attributes.push(Attribute {
            name: "local_time_first_reading".into(),
            value: format!("{d} (time zone not recorded)"),
        });
    }
    ds.electrical.push(ElectricalTest {
        id: o.test_id.clone(),
        material: o.material.clone(),
        structure: o.structure.clone(),
        structure_params: vec![],
        instrument: Some("source-measure unit (model not recorded in the export)".into()),
        bias: vec![],
        curves,
        extracted,
        at: date
            .as_deref()
            .and_then(|d| d.get(..10))
            .map(str::to_string),
        attributes,
        provenance: src.prov(
            "hackerfab::smu_sweep_csv",
            Some(format!("{} data rows", data.len())),
        ),
        license: src.lic(),
    });
    Ok(ds)
}

// ───────────────────────────── impedance-matcher telemetry ─────────────────────────────

#[derive(Debug, Clone)]
pub struct TraceRunOpts {
    pub run_id: String,
    pub tool: String,
    pub step_name: String,
    pub status: RunStatus,
    pub material: Vec<String>,
    pub note: Option<String>,
}

/// `host_time_s,device_millis,vswr,forward_v,reverse_v,motor1_pos_rad,motor2_pos_rad,at_match`
/// from `impedance-matcher/firmware/live.py`. One run and one trace per sensor column, with
/// irregular sample times from `host_time_s`. `nan` becomes a missing sample. A VSWR below 1
/// is physically impossible (VSWR = (V_f + V_r)/(V_f − V_r) ≥ 1) and is flagged, not dropped.
pub fn impedance_csv(text: &str, src: &Source, o: &TraceRunOpts) -> Result<Dataset, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<&str> = lines
        .next()
        .ok_or("empty file")?
        .split(',')
        .map(str::trim)
        .collect();
    let want = [
        "host_time_s",
        "device_millis",
        "vswr",
        "forward_v",
        "reverse_v",
        "motor1_pos_rad",
        "motor2_pos_rad",
        "at_match",
    ];
    if header != want {
        return Err(format!(
            "not an impedance-matcher telemetry file: header {header:?}"
        ));
    }
    let mut cols: Vec<Vec<Option<f64>>> = vec![Vec::new(); 8];
    for (n, l) in lines.enumerate() {
        let f: Vec<&str> = l.split(',').collect();
        if f.len() != 8 {
            return Err(format!("row {}: {} fields", n + 2, f.len()));
        }
        for (j, s) in f.iter().enumerate() {
            cols[j].push(s.trim().parse::<f64>().ok().filter(|x| x.is_finite()));
        }
    }
    let t: Vec<f64> = cols[0].iter().map(|x| x.unwrap_or(f64::NAN)).collect();
    if t.is_empty() || t.iter().any(|x| !x.is_finite()) {
        return Err("host_time_s has missing values".into());
    }
    let t0 = t[0];
    // Row-level flags, applied to every channel. A host time earlier than the previous one is
    // clamped to it (sample times must not go backwards) and flagged with the original value;
    // a drop in device_millis means the matcher's firmware restarted mid-run.
    let mut row_flags: Vec<SampleFlag> = Vec::new();
    let mut times: Vec<f64> = Vec::with_capacity(t.len());
    for (i, x) in t.iter().enumerate() {
        let rel = ((x - t0) * 1e6).round() / 1e6;
        match times.last().copied() {
            Some(prev) if rel < prev => {
                row_flags.push(SampleFlag { index: i as u64, code: format!("time_out_of_order: host_time_s {x} is earlier than the previous sample; clamped") });
                times.push(prev);
            }
            _ => times.push(rel),
        }
        if i > 0 {
            if let (Some(a), Some(b)) = (cols[1][i - 1], cols[1][i]) {
                if b < a {
                    row_flags.push(SampleFlag {
                        index: i as u64,
                        code: format!("device_reset: device_millis fell from {a} to {b}"),
                    });
                }
            }
        }
    }
    let mut ds = src.dataset(
        &format!("{}-ds", o.run_id),
        "RF sputter impedance-matcher telemetry",
    );
    ds.runs.push(ProcessRun {
        id: o.run_id.clone(),
        recipe: None,
        tool: o.tool.clone(),
        chamber: None,
        route: None,
        step_no: None,
        step_name: Some(o.step_name.clone()),
        material: o.material.clone(),
        start: Some(rfc3339(t0)),
        end: Some(rfc3339(*t.last().unwrap())),
        operator_role: None,
        status: o.status,
        settings: vec![],
        actuals: vec![],
        attributes: o
            .note
            .iter()
            .map(|n| Attribute {
                name: "note".into(),
                value: n.clone(),
            })
            .collect(),
        provenance: src.prov("hackerfab::impedance_csv", None),
        license: src.lic(),
    });
    let channels = [
        (2, "vswr", QuantityKind::Dimensionless, "1"),
        (3, "forward_v", QuantityKind::Voltage, "V"),
        (4, "reverse_v", QuantityKind::Voltage, "V"),
        (5, "motor1_pos", QuantityKind::Angle, "rad"),
        (6, "motor2_pos", QuantityKind::Angle, "rad"),
        (7, "at_match", QuantityKind::Dimensionless, "1"),
    ];
    for (j, name, kind, unit) in channels {
        let values = cols[j].clone();
        let mut flags = row_flags.clone();
        if name == "vswr" {
            flags.extend(
                values
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| matches!(v, Some(x) if *x < 1.0))
                    .map(|(i, _)| SampleFlag {
                        index: i as u64,
                        code: "physically_impossible".into(),
                    }),
            );
            flags.sort_by_key(|f| f.index);
        }
        ds.traces.push(Trace {
            id: format!("{}/{name}", o.run_id),
            run: o.run_id.clone(),
            tool: o.tool.clone(),
            sensor: name.into(),
            kind: Some(kind),
            unit: Some(unit.into()),
            sample_rate_hz: None,
            times_s: times.clone(),
            t0: Some(rfc3339(t0)),
            values,
            uncertainty: Some(Uncertainty::Unknown { reason: "detector and ADC calibration not recorded; firmware averages 300 ADC readings per sample".into() }),
            calibration: Some(Calibration { status: CalStatus::Unknown, date: None, reference: None }),
            flags,
            provenance: src.prov("hackerfab::impedance_csv", Some(format!("column {}", want[j]))),
            license: src.lic(),
        });
    }
    Ok(ds)
}

// ───────────────────────────── MKS 901P pressure log ─────────────────────────────

/// `t_s,pressure` rows from `uw-p901-reader/901p-serial/main.py` (query `PR3`). The file holds
/// no unit and no wall time: `unit` is the caller's statement, and `unit_basis` says why.
pub fn p901_csv(
    text: &str,
    src: &Source,
    o: &TraceRunOpts,
    unit: &str,
    unit_basis: &str,
) -> Result<Dataset, String> {
    let (mut times, mut values) = (Vec::new(), Vec::new());
    for (n, l) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let mut f = l.split(',');
        let (Some(a), Some(b)) = (f.next(), f.next()) else {
            return Err(format!("row {}: expected t,pressure", n + 1));
        };
        times.push(
            a.trim()
                .parse::<f64>()
                .map_err(|_| format!("row {}: time '{a}'", n + 1))?,
        );
        values.push(b.trim().parse::<f64>().ok().filter(|x| x.is_finite()));
    }
    let mut ds = src.dataset(
        &format!("{}-ds", o.run_id),
        "Vacuum pressure log (MKS 901P)",
    );
    ds.runs.push(ProcessRun {
        id: o.run_id.clone(),
        recipe: None,
        tool: o.tool.clone(),
        chamber: None,
        route: None,
        step_no: None,
        step_name: Some(o.step_name.clone()),
        material: o.material.clone(),
        start: None,
        end: None,
        operator_role: None,
        status: o.status,
        settings: vec![],
        actuals: vec![],
        attributes: o
            .note
            .iter()
            .map(|n| Attribute {
                name: "note".into(),
                value: n.clone(),
            })
            .collect(),
        provenance: src.prov("hackerfab::p901_csv", None),
        license: src.lic(),
    });
    ds.traces.push(Trace {
        id: format!("{}/pressure", o.run_id),
        run: o.run_id.clone(),
        tool: o.tool.clone(),
        sensor: "MKS 901P combined reading (PR3)".into(),
        kind: Some(QuantityKind::Pressure),
        unit: Some(unit.into()),
        sample_rate_hz: None,
        times_s: times,
        t0: None,
        values,
        uncertainty: Some(Uncertainty::Unknown {
            reason: "transducer accuracy and calibration not recorded with the log".into(),
        }),
        calibration: Some(Calibration {
            status: CalStatus::Unknown,
            date: None,
            reference: None,
        }),
        flags: vec![],
        provenance: src.prov_note(
            "hackerfab::p901_csv",
            Some("all rows".into()),
            &format!("Unit: {unit_basis}"),
        ),
        license: src.lic(),
    });
    Ok(ds)
}

// ───────────────────────────── tube-furnace recipe ─────────────────────────────

/// `{"name": ..., "segments": [{"target": °C, "ramp_rate": °C/min, "hold_min": min}]}` from
/// `uw-tube-furnace_pid/furnace/recipe.py`. Ramp rates are temperature differences per time,
/// so they are written in K/min (a °C step equals a K step).
pub fn furnace_recipe(
    text: &str,
    src: &Source,
    id: &str,
    version: &str,
) -> Result<Dataset, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("recipe")
        .to_string();
    let segs = v
        .get("segments")
        .and_then(Value::as_array)
        .ok_or("no segments")?;
    let mut params = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let f = |k: &str| s.get(k).and_then(Value::as_f64);
        let target = f("target").ok_or(format!("segment {i}: no target"))?;
        let ramp = f("ramp_rate")
            .ok_or(format!("segment {i}: no ramp_rate"))?
            .abs();
        let hold = f("hold_min").unwrap_or(0.0).max(0.0);
        params.push(setpoint(
            &format!("segment_{i}_target_temperature"),
            target,
            "degC",
        ));
        params.push(Parameter {
            name: format!("segment_{i}_ramp_rate"),
            kind: None,
            quantity: Quantity::setpoint(ramp, "K/min"),
        });
        params.push(setpoint(&format!("segment_{i}_hold_time"), hold, "min"));
    }
    let mut ds = src.dataset(&format!("{id}-ds"), "Tube furnace recipe");
    ds.recipes.push(Recipe {
        id: id.into(),
        name,
        version: version.into(),
        supersedes: None,
        step_kind: Some("furnace_anneal".into()),
        tool_class: Some("tube furnace (DB-4824 PID)".into()),
        parameters: params,
        attributes: vec![],
        provenance: src.prov("hackerfab::furnace_recipe", None),
        license: src.lic(),
    });
    Ok(ds)
}

// ───────────────────────────── process traveler ─────────────────────────────

fn step_kind(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    Some(match () {
        _ if n.contains("sputter") => "rf_sputter",
        _ if n.contains("spin resist") => "resist_coat",
        _ if n.contains("spin-on dopant") => "spin_on_dopant",
        _ if n.contains("spin") => "spin_coat",
        _ if n.contains("bake") => "bake",
        _ if n.contains("lithography") || n.contains("expos") => "litho_expose",
        _ if n.contains("develop") => "develop",
        _ if n.contains("strip") => "resist_strip",
        _ if n.contains("wet-etch") || n.contains("wet etch") => "wet_etch",
        _ if n.contains("rca") || n.contains("clean") => "clean",
        _ if n.contains("diffusion") => "diffusion",
        _ if n.contains("substrate") => "substrate",
        _ => return None,
    })
}

/// A traveler sheet: a header row with `Step #`, `Step Name`, `Tool Name`, `Parameters`,
/// `Comments`; each step's parameters are `Key: value unit` lines. Every step becomes a
/// SPECIFIED recipe; numeric values become setpoints with units, everything else an attribute.
/// A traveler is a plan: it yields recipes and no runs.
pub fn traveler(
    rows: &[Vec<String>],
    src: &Source,
    prefix: &str,
    route: &str,
) -> Result<Dataset, String> {
    let h = rows
        .iter()
        .position(|r| r.iter().any(|c| c.trim() == "Step #"))
        .ok_or("no 'Step #' header row")?;
    let col = |name: &str| rows[h].iter().position(|c| c.trim() == name);
    let (cn, cname, ctool, cpar, ccom) = (
        col("Step #").unwrap(),
        col("Step Name"),
        col("Tool Name"),
        col("Parameters"),
        col("Comments"),
    );
    let mut ds = src.dataset(&format!("{prefix}-ds"), route);
    let version = src.version.clone().unwrap_or_else(|| "unversioned".into());
    for (k, r) in rows.iter().enumerate().skip(h + 1) {
        let get = |j: Option<usize>| {
            j.and_then(|j| r.get(j))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        let Ok(step) = get(Some(cn)).parse::<u32>() else {
            continue;
        };
        let name = get(cname);
        let mut params = Vec::new();
        let mut attrs = Vec::new();
        let is_substrate = step_kind(&name) == Some("substrate");
        for line in get(cpar).lines().map(str::trim).filter(|l| !l.is_empty()) {
            if is_substrate {
                attrs.push(Attribute {
                    name: "substrate".into(),
                    value: line.into(),
                });
                if let Some((v, u)) = line.rsplit_once(':').and_then(|(_, t)| number_unit(t)) {
                    if kind_for("thickness", &u) == Some(QuantityKind::Length) {
                        params.push(Parameter {
                            name: "substrate_thickness".into(),
                            kind: Some(QuantityKind::Length),
                            quantity: Quantity::setpoint(v, &u),
                        });
                    }
                }
                continue;
            }
            let Some((key, val)) = line.split_once(':') else {
                attrs.push(Attribute {
                    name: "parameter_text".into(),
                    value: line.into(),
                });
                continue;
            };
            let (key, val) = (key.trim(), val.trim());
            let pname = snake(key);
            match number_unit(val) {
                // A whole-value number with a known unit (or none, for a pure number).
                Some((v, u))
                    if !val.contains(" : ")
                        && crate::units::parse(if u.is_empty() { "1" } else { &u })
                            .is_some() =>
                {
                    let unit = if u.is_empty() { "1".to_string() } else { u };
                    params.push(Parameter {
                        name: pname.clone(),
                        kind: kind_for(&pname, &unit),
                        quantity: Quantity::setpoint(v, &unit),
                    });
                }
                _ => attrs.push(Attribute {
                    name: pname,
                    value: val.into(),
                }),
            }
        }
        let comment = get(ccom);
        if !comment.is_empty() {
            attrs.push(Attribute {
                name: "comment".into(),
                value: comment,
            });
        }
        let tool = get(ctool);
        ds.recipes.push(Recipe {
            id: format!("{prefix}/step-{step:02}"),
            name: format!("{route} step {step}: {name}"),
            version: version.clone(),
            supersedes: None,
            step_kind: step_kind(&name).map(str::to_string),
            tool_class: if tool.is_empty() { None } else { Some(tool) },
            parameters: params,
            attributes: attrs,
            provenance: src.prov("hackerfab::traveler", Some(format!("sheet row {}", k + 1))),
            license: src.lic(),
        });
    }
    Ok(ds)
}

/// [`traveler`] from an .xlsx file.
pub fn traveler_xlsx(
    bytes: &[u8],
    src: &Source,
    prefix: &str,
    route: &str,
) -> Result<Dataset, String> {
    traveler(&super::xlsx::first_sheet(bytes)?, src, prefix, route)
}

// ───────────────────────────── NMOS characterization reports ─────────────────────────────

#[derive(Debug, Clone)]
pub struct ReportOpts {
    pub prefix: String,
    /// The unit id everything in the report belongs to when no chip is named (the group's run).
    pub group_unit: String,
    /// "Chip 19" → unit id. Units are created for each (kind Chip, parent `group_unit`).
    pub chips: Vec<(String, String)>,
    pub site: Option<String>,
    /// How each parameter was extracted, by its name in the table ("Vth", "Ion", ...).
    pub methods: Vec<(String, String)>,
}

fn md_cells(line: &str) -> Vec<String> {
    let t = line.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|').map(|c| c.trim().replace("\\_", "_")).collect()
}

/// "1.40 ± 0.64" or "1,214 ± 733 Ω" → (mean, sd, unit in the cell if any).
fn pm(cell: &str) -> Option<(f64, f64, String)> {
    let (a, b) = cell.split_once('±')?;
    let mean: f64 = a.trim().replace(',', "").parse().ok()?;
    let b = b.trim().replace(',', "");
    let (sd, unit) = number_unit(&b)?;
    Some((mean, sd, unit))
}

fn method_for(methods: &[(String, String)], param: &str) -> Option<String> {
    methods
        .iter()
        .find(|(k, _)| param.to_lowercase().starts_with(&k.to_lowercase()))
        .map(|(_, m)| m.clone())
}

/// Yield statements (`N_visual_pass = 900 … = 900 / 952`, `N_functional = 94 N_probed = 100`)
/// and tables of population statistics whose headers read `<name> ± σ (<unit>)` or whose
/// cells read `mean ± sd unit`. The nearest preceding line naming "Chip <n>" sets the scope.
pub fn report_md(text: &str, src: &Source, o: &ReportOpts) -> Result<Dataset, String> {
    let mut ds = src.dataset(&format!("{}-ds", o.prefix), "NMOS characterization report");
    let unit = |id: &str, kind: UnitKind, parent: Option<&str>, label: &str| MaterialUnit {
        id: id.into(),
        kind,
        parent: parent.map(str::to_string),
        label: Some(label.into()),
        site: o.site.clone(),
        substrate: None,
        position: None,
        provenance: src.prov("hackerfab::report_md", None),
        license: src.lic(),
    };
    ds.units.push(unit(
        &o.group_unit,
        UnitKind::Lot,
        None,
        "characterization group (devices from the chips below)",
    ));
    for (name, id) in &o.chips {
        ds.units
            .push(unit(id, UnitKind::Chip, Some(&o.group_unit), name));
    }
    let chip_of = |s: &str| -> Option<String> {
        o.chips
            .iter()
            .find(|(n, _)| {
                s.match_indices(n.as_str())
                    .any(|(i, _)| !s[i + n.len()..].starts_with(|c: char| c.is_ascii_digit()))
            })
            .map(|(_, id)| id.clone())
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut scope = o.group_unit.clone();
    let mut n_yield = 0;
    let mut k = 0;
    while k < lines.len() {
        let line = lines[k].replace("\\_", "_");
        let loc = format!("line {}", k + 1);
        // Yield lines.
        if let Some(i) = line.find("Yield_visual") {
            if let Some((a, b)) = line[i..]
                .split_once('=')
                .and_then(|(_, r)| r.split_once('/'))
            {
                let pass = a.trim().parse::<u64>().ok();
                let tested = b
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse::<u64>().ok());
                if let (Some(p), Some(t)) = (pass, tested) {
                    n_yield += 1;
                    ds.yields.push(YieldRecord {
                        id: format!("{}/yield-visual-{n_yield}", o.prefix),
                        scope: o.group_unit.clone(),
                        stage: "visual".into(),
                        criterion: "continuous gate, defined and accessible source/drain contacts, no breaks, misalignment or residue bridging (microscope)".into(),
                        tested: t,
                        passed: p,
                        provenance: src.prov("hackerfab::report_md", Some(loc.clone())),
                        license: src.lic(),
                    });
                }
            }
        }
        if let (Some(i), Some(j)) = (line.find("N_functional ="), line.find("N_probed =")) {
            let f = line[i + 14..]
                .split_whitespace()
                .next()
                .and_then(|x| x.parse::<u64>().ok());
            let p = line[j + 10..]
                .split_whitespace()
                .next()
                .and_then(|x| x.parse::<u64>().ok());
            if let (Some(f), Some(p)) = (f, p) {
                n_yield += 1;
                ds.yields.push(YieldRecord {
                    id: format!("{}/yield-electrical-{n_yield}", o.prefix),
                    scope: o.group_unit.clone(),
                    stage: "electrical".into(),
                    criterion: "recognizable NMOS I–V: drain current rising with gate voltage, cutoff-to-saturation transition, extractable parameters".into(),
                    tested: p,
                    passed: f,
                    provenance: src.prov("hackerfab::report_md", Some(loc.clone())),
                    license: src.lic(),
                });
            }
        }
        if !line.trim_start().starts_with('|') {
            if let Some(c) = chip_of(&line) {
                scope = c;
            }
            k += 1;
            continue;
        }
        // A table: header, separator, rows.
        let head = md_cells(&line);
        let start = k;
        k += 2;
        let mut body = Vec::new();
        while k < lines.len() && lines[k].trim_start().starts_with('|') {
            body.push((k, md_cells(lines[k])));
            k += 1;
        }
        // Form 1: per-size table with "<name> ± σ (<unit>)" headers.
        let quantity_col = head.iter().position(|h| h.eq_ignore_ascii_case("Quantity"));
        let pm_cols: Vec<(usize, String, String)> = head
            .iter()
            .enumerate()
            .filter_map(|(j, h)| {
                let (name, rest) = h.split_once('±')?;
                let unit = rest
                    .split_once('(')
                    .map(|(_, u)| u.trim_end_matches(')').trim().to_string())?;
                Some((j, name.trim().to_string(), super::unit_alias(&unit)))
            })
            .collect();
        if let (Some(qc), false) = (quantity_col, pm_cols.is_empty()) {
            for (row_k, cells) in &body {
                let Some(n) = cells.get(qc).and_then(|x| x.parse::<u64>().ok()) else {
                    continue;
                };
                let label: Vec<String> = head
                    .iter()
                    .zip(cells)
                    .take(qc)
                    .map(|(h, c)| format!("{h} {c}"))
                    .collect();
                for (j, name, unit) in &pm_cols {
                    let Some((mean, sd, _)) = cells.get(*j).and_then(|c| pm(c)) else {
                        continue;
                    };
                    ds.summaries.push(Summary {
                        id: format!(
                            "{}/{}/{}/{}",
                            o.prefix,
                            slug(&scope),
                            slug(&label.join(" ")),
                            snake(name)
                        ),
                        scope: scope.clone(),
                        population: format!("{} (functional devices)", label.join(", ")),
                        parameter: name.clone(),
                        kind: kind_for(name, unit),
                        unit: Some(unit.clone()),
                        n,
                        mean,
                        sd: Some(sd),
                        min: None,
                        median: None,
                        max: None,
                        method: method_for(&o.methods, name),
                        members: vec![],
                        provenance: src.prov(
                            "hackerfab::report_md",
                            Some(format!(
                                "table at line {}, row line {}",
                                start + 1,
                                row_k + 1
                            )),
                        ),
                        license: src.lic(),
                    });
                }
            }
            continue;
        }
        // Form 2: "Parameter | Chip 19 | Chip 50" with "mean ± sd unit" cells.
        let chip_cols: Vec<(usize, String)> = head
            .iter()
            .enumerate()
            .filter_map(|(j, h)| chip_of(h).map(|id| (j, id)))
            .collect();
        if !chip_cols.is_empty() {
            let probed_row = body.iter().find(|(_, c)| {
                c.first()
                    .is_some_and(|x| x.to_lowercase().starts_with("devices probed"))
            });
            let functional_row = body.iter().find(|(_, c)| {
                c.first()
                    .is_some_and(|x| x.to_lowercase().starts_with("functional devices"))
            });
            for (j, chip) in &chip_cols {
                if let (Some((pk, p)), Some((_, f))) = (probed_row, functional_row) {
                    if let (Some(t), Some(pass)) = (
                        p.get(*j).and_then(|x| x.parse::<u64>().ok()),
                        f.get(*j).and_then(|x| x.parse::<u64>().ok()),
                    ) {
                        ds.yields.push(YieldRecord {
                            id: format!("{}/{}/yield-electrical", o.prefix, slug(chip)),
                            scope: chip.clone(),
                            stage: "electrical".into(),
                            criterion: "recognizable NMOS I–V (see report section 7.3)".into(),
                            tested: t,
                            passed: pass,
                            provenance: src.prov(
                                "hackerfab::report_md",
                                Some(format!("table at line {}, row line {}", start + 1, pk + 1)),
                            ),
                            license: src.lic(),
                        });
                    }
                }
                for (row_k, cells) in &body {
                    let Some(param) = cells.first() else { continue };
                    let Some((mean, sd, unit)) = cells.get(*j).and_then(|c| pm(c)) else {
                        continue;
                    };
                    let pname = param.split('(').next().unwrap_or(param).trim().to_string();
                    let unit = if unit.is_empty() {
                        "1".to_string()
                    } else {
                        unit
                    };
                    let n = functional_row
                        .and_then(|(_, f)| f.get(*j))
                        .and_then(|x| x.parse::<u64>().ok())
                        .unwrap_or(0);
                    ds.summaries.push(Summary {
                        id: format!("{}/{}/all-sizes/{}", o.prefix, slug(chip), snake(&pname)),
                        scope: chip.clone(),
                        population: "all functional devices, all gate lengths pooled".into(),
                        parameter: pname.clone(),
                        kind: kind_for(&pname, &unit),
                        unit: Some(unit),
                        n,
                        mean,
                        sd: Some(sd),
                        min: None,
                        median: None,
                        max: None,
                        method: method_for(&o.methods, &pname),
                        members: vec![],
                        provenance: src.prov(
                            "hackerfab::report_md",
                            Some(format!(
                                "table at line {}, row line {}",
                                start + 1,
                                row_k + 1
                            )),
                        ),
                        license: src.lic(),
                    });
                }
            }
        }
    }
    Ok(ds)
}

// ───────────────────────────── per-device extraction output ─────────────────────────────

#[derive(Debug, Clone)]
pub struct NmosOpts {
    pub prefix: String,
    /// chip number in the file → unit id (must exist in the dataset this is merged into).
    pub chips: Vec<(String, String)>,
    /// NMOS number → gate length in µm.
    pub gate_length_um: Vec<(u32, f64)>,
    pub methods: Vec<(String, String)>,
    pub site: Option<String>,
}

fn suffix_unit(col: &str) -> Option<(String, String, Option<f64>)> {
    // Ion_A, Ron_ohm, Vth_Vd5V (Vth at Vd = 5 V), Vd_ref_V, Ion_Ioff_ratio.
    if let Some(rest) = col.strip_prefix("Vth_Vd") {
        let (v, u) = number_unit(rest)?;
        let vd = crate::units::convert(v, &u, "V")?;
        return Some(("Vth".into(), "V".into(), Some(vd)));
    }
    let (name, unit) = col.rsplit_once('_')?;
    let unit = match unit {
        "A" => "A",
        "V" => "V",
        "ohm" => "ohm",
        "ratio" => "1",
        _ => return None,
    };
    Some((name.to_string(), unit.to_string(), None))
}

/// `nmos_summary.csv` (one row per functional device: chip, nmos, pattern, fet_size, filename,
/// Vth at four drain biases, Vd_ref_V, Ion_A, Ioff_A, Ron_ohm, Roff_ohm, Ion_Ioff_ratio) and,
/// optionally, `nmos_summary_per_gate_voltage.csv` (chip, fet_size, GateV_V, DrainI_at_Vd_A,
/// R_at_Vd_ohm, GateI_at_Vd_A) as Id–Vg curves at the reference drain bias.
pub fn nmos_summary_csv(
    summary: &str,
    per_gate: Option<&str>,
    src: &Source,
    o: &NmosOpts,
) -> Result<Dataset, String> {
    let mut ds = src.dataset(&format!("{}-ds", o.prefix), "Per-device NMOS extraction");
    let mut lines = summary.lines().filter(|l| !l.trim().is_empty());
    let head: Vec<String> = lines
        .next()
        .ok_or("empty summary")?
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let ix = |n: &str| head.iter().position(|h| h == n);
    let (c_chip, c_nmos, c_pat, c_fet) = (
        ix("chip").ok_or("no chip column")?,
        ix("nmos"),
        ix("pattern"),
        ix("fet_size"),
    );
    // (chip, fet_size) → [(gate V, drain A, gate A)]
    type Points = Vec<(f64, Option<f64>, Option<f64>)>;
    let mut curves: BTreeMap<(String, String), Points> = BTreeMap::new();
    let mut vd_ref_of: BTreeMap<(String, String), f64> = BTreeMap::new();
    if let Some(pg) = per_gate {
        let mut it = pg.lines().filter(|l| !l.trim().is_empty());
        let h: Vec<String> = it
            .next()
            .ok_or("empty per-gate file")?
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let p = |n: &str| h.iter().position(|x| x == n);
        let (a, b, g, i, gi) = (
            p("chip"),
            p("fet_size"),
            p("GateV_V"),
            p("DrainI_at_Vd_A"),
            p("GateI_at_Vd_A"),
        );
        let (Some(a), Some(b), Some(g), Some(i)) = (a, b, g, i) else {
            return Err("per-gate file lacks chip,fet_size,GateV_V,DrainI_at_Vd_A".into());
        };
        for l in it {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            let num = |j: usize| f.get(j).and_then(|s| s.parse::<f64>().ok());
            if let Some(vg) = num(g) {
                curves
                    .entry((f[a].to_string(), f[b].to_string()))
                    .or_default()
                    .push((vg, num(i), gi.and_then(num)));
            }
        }
    }
    for (row_n, l) in lines.enumerate() {
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        let chip_no = f.get(c_chip).copied().unwrap_or("");
        let Some(chip) = o
            .chips
            .iter()
            .find(|(n, _)| n == chip_no)
            .map(|(_, id)| id.clone())
        else {
            continue;
        };
        let fet = c_fet.and_then(|j| f.get(j)).copied().unwrap_or("");
        let nmos: Option<u32> = c_nmos.and_then(|j| f.get(j)).and_then(|s| s.parse().ok());
        let dev = format!("{chip}/{}", slug(fet));
        ds.units.push(MaterialUnit {
            id: dev.clone(),
            kind: UnitKind::TestSite,
            parent: Some(chip.clone()),
            label: Some(format!(
                "{fet} (pattern {})",
                c_pat.and_then(|j| f.get(j)).copied().unwrap_or("?")
            )),
            site: o.site.clone(),
            substrate: None,
            position: None,
            provenance: src.prov(
                "hackerfab::nmos_summary_csv",
                Some(format!("row {}", row_n + 2)),
            ),
            license: src.lic(),
        });
        let mut extracted = Vec::new();
        let mut vd_ref = None;
        for (j, h) in head.iter().enumerate() {
            let Some((name, unit, vd)) = suffix_unit(h) else {
                continue;
            };
            let Some(v) = f
                .get(j)
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|x| x.is_finite())
            else {
                continue;
            };
            if name == "Vd_ref" {
                vd_ref = Some(v);
                continue;
            }
            let pname = match vd {
                Some(vd) => format!("Vth_at_Vd_{vd}V"),
                None => name.clone(),
            };
            let method = method_for(&o.methods, &name).unwrap_or_else(|| "not stated".into());
            extracted.push(Extracted {
                name: pname,
                kind: kind_for(&name, &unit),
                quantity: Quantity::derived(
                    v,
                    &unit,
                    Uncertainty::Unknown {
                        reason: "the extraction reports no uncertainty".into(),
                    },
                    &method,
                    vec![],
                ),
            });
        }
        if let Some(v) = vd_ref {
            vd_ref_of.insert((chip_no.to_string(), fet.to_string()), v);
        }
        let mut params = vec![];
        if let Some(l) = nmos
            .and_then(|n| o.gate_length_um.iter().find(|(k, _)| *k == n))
            .map(|x| x.1)
        {
            params.push(Parameter {
                name: "gate_length".into(),
                kind: Some(QuantityKind::Length),
                quantity: Quantity::setpoint(l, "um"),
            });
        }
        let mut cvs = Vec::new();
        if let Some(pts) = curves.get(&(chip_no.to_string(), fet.to_string())) {
            let vd = vd_ref_of
                .get(&(chip_no.to_string(), fet.to_string()))
                .copied();
            cvs.push(Curve {
                name: "Id(Vg) at Vd_ref".into(),
                x: Axis {
                    name: "gate voltage".into(),
                    kind: Some(QuantityKind::Voltage),
                    unit: Some("V".into()),
                    values: pts.iter().map(|p| Some(p.0)).collect(),
                    derivation: None,
                },
                y: Axis {
                    name: "drain current".into(),
                    kind: Some(QuantityKind::Current),
                    unit: Some("A".into()),
                    values: pts.iter().map(|p| p.1).collect(),
                    derivation: None,
                },
                fixed: vd
                    .map(|v| {
                        vec![Parameter {
                            name: "drain_voltage".into(),
                            kind: Some(QuantityKind::Voltage),
                            quantity: Quantity::setpoint(v, "V"),
                        }]
                    })
                    .unwrap_or_default(),
                uncertainty: Some(Uncertainty::Unknown {
                    reason: "SMU accuracy not recorded".into(),
                }),
            });
        }
        let filename = head
            .iter()
            .position(|h| h == "filename")
            .and_then(|j| f.get(j))
            .copied()
            .unwrap_or("");
        ds.electrical.push(ElectricalTest {
            id: format!("{dev}/iv"),
            material: dev.clone(),
            structure: "NMOS transistor".into(),
            structure_params: params,
            instrument: Some("Keithley source-measure unit".into()),
            bias: vec![
                Bias {
                    terminal: "gate".into(),
                    mode: BiasMode::Sweep {
                        start: Quantity::setpoint(0.0, "V"),
                        stop: Quantity::setpoint(6.0, "V"),
                        step: Some(Quantity::setpoint(1.0, "V")),
                    },
                },
                Bias {
                    terminal: "drain".into(),
                    mode: BiasMode::Sweep {
                        start: Quantity::setpoint(0.0, "V"),
                        stop: Quantity::setpoint(10.0, "V"),
                        step: None,
                    },
                },
                Bias {
                    terminal: "source".into(),
                    mode: BiasMode::Fixed {
                        value: Quantity::setpoint(0.0, "V"),
                    },
                },
            ],
            curves: cvs,
            extracted,
            at: None,
            attributes: vec![Attribute {
                name: "raw_file".into(),
                value: filename.into(),
            }],
            provenance: src.prov(
                "hackerfab::nmos_summary_csv",
                Some(format!("row {}", row_n + 2)),
            ),
            license: src.lic(),
        });
    }
    Ok(ds)
}

// ───────────────────────────── smu.py current/voltage pair ─────────────────────────────

/// `{chip}_{device}_currents.csv` and `_voltages.csv` from `hacker-fab/smu/smu.py`: a header
/// row of gate voltages, then one row of samples per gate voltage. The currents are not
/// measured directly: smu.py divides the voltage across a series drain resistor by its value
/// (100 Ω by default), so the current axis carries that derivation.
pub fn smu_pair(
    currents: &str,
    voltages: &str,
    src: &Source,
    test_id: &str,
    material: &str,
    r_drain_ohm: f64,
) -> Result<Dataset, String> {
    let rows = |t: &str| -> Vec<Vec<Option<f64>>> {
        t.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.split(',').map(|s| s.trim().parse::<f64>().ok()).collect())
            .collect()
    };
    let (ci, vi) = (rows(currents), rows(voltages));
    if ci.is_empty() || vi.is_empty() || ci[0] != vi[0] {
        return Err("the two files must start with the same gate-voltage header".into());
    }
    let vg = &ci[0];
    if ci.len() != vi.len() || ci.len() != vg.len() + 1 {
        return Err(format!(
            "{} gate voltages but {} current rows and {} voltage rows",
            vg.len(),
            ci.len() - 1,
            vi.len() - 1
        ));
    }
    let mut curves = Vec::new();
    for (k, g) in vg.iter().enumerate() {
        let g = g.ok_or("a gate voltage is not a number")?;
        curves.push(Curve {
            name: format!("Id(Vds) at Vg = {g} V"),
            x: Axis { name: "drain-source voltage".into(), kind: Some(QuantityKind::Voltage), unit: Some("V".into()), values: vi[k + 1].clone(), derivation: None },
            y: Axis {
                name: "drain current".into(),
                kind: Some(QuantityKind::Current),
                unit: Some("A".into()),
                values: ci[k + 1].clone(),
                derivation: Some(Derivation {
                    method: format!("Id = V(series drain resistor) / R, R = {r_drain_ohm} Ω (smu.py, Analog Discovery 3 scope channel 1)"),
                    inputs: vec![],
                }),
            },
            fixed: vec![Parameter { name: "gate_voltage".into(), kind: Some(QuantityKind::Voltage), quantity: Quantity::setpoint(g, "V") }],
            uncertainty: Some(Uncertainty::Unknown { reason: "scope accuracy and resistor tolerance not recorded".into() }),
        });
    }
    let mut ds = src.dataset(&format!("{test_id}-ds"), "smu.py output");
    ds.electrical.push(ElectricalTest {
        id: test_id.into(),
        material: material.into(),
        structure: "MOSFET".into(),
        structure_params: vec![Parameter {
            name: "series_drain_resistor".into(),
            kind: Some(QuantityKind::Resistance),
            quantity: Quantity::setpoint(r_drain_ohm, "ohm"),
        }],
        instrument: Some("Digilent Analog Discovery 3 via hacker-fab/smu smu.py".into()),
        bias: vec![],
        curves,
        extracted: vec![],
        at: None,
        attributes: vec![Attribute {
            name: "drain_drive".into(),
            value: "wavegen ch1 sine, 5 V offset, 5 V amplitude, 100 Hz".into(),
        }],
        provenance: src.prov("hackerfab::smu_pair", None),
        license: src.lic(),
    });
    Ok(ds)
}

// ───────────────────────────── data-management database ─────────────────────────────

#[derive(Clone, Copy)]
enum Role {
    /// A setpoint with this unit.
    Set(&'static str),
    /// A value read during the run (pressure before start, room humidity).
    Act(&'static str),
    /// A metrology result: unit, method.
    Metric(&'static str, &'static str),
    /// A qualitative result with this scale.
    Obs(&'static str),
    Attr,
}

use Role::*;

/// The data-management models (`data_management/models.py`, commit ae5436a), field suffix →
/// role. Units are the ones in each field's `verbose_name`.
const DM: &[(&str, &[(&str, Role)])] = &[
    (
        "aluminumetch",
        &[
            ("temp", Set("degC")),
            ("duration", Set("s")),
            ("stir_rpm", Set("rpm")),
            ("metric_alum_etch_depth", Metric("nm", "not recorded")),
            ("metric_photoresist_peeling", Obs("Y/N")),
            ("metric_aluminum_peeling", Obs("Y/N")),
        ],
    ),
    (
        "aluminumevaporation",
        &[
            ("temp", Set("degC")),
            ("duration", Set("s")),
            ("pressure_before_start_seq", Act("uTorr")),
            ("pressure_before_evaporation", Act("uTorr")),
            ("metric_layer_thickness", Metric("angstrom", "profilometer")),
            (
                "metric_layer_thick_qcm",
                Metric("angstrom", "quartz crystal microbalance"),
            ),
            (
                "metric_deposition_rate",
                Metric("angstrom/s", "not recorded"),
            ),
        ],
    ),
    (
        "glassdeposition",
        &[
            ("glass_type", Attr),
            ("cleaning_step", Attr),
            ("days_glass_at_room_temp", Act("d")),
            ("prebake_temp", Set("degC")),
            ("prebake_duration", Set("s")),
            ("amount_drops", Set("1")),
            ("spin_rpm", Set("rpm")),
            ("spin_duration", Set("s")),
            ("bake_temp", Set("degC")),
            ("bake_duration", Set("s")),
            ("humidity", Act("%")),
            ("metric_layer_thickness", Metric("um", "not recorded")),
            ("metric_cracking", Obs("Y/N")),
            ("metric_particles", Obs("Y/N")),
        ],
    ),
    (
        "diffusion",
        &[("temp", Set("degC")), ("duration", Set("s"))],
    ),
    (
        "hfoxideetch",
        &[
            ("temp", Set("degC")),
            ("duration", Set("s")),
            ("metric_oxide_etch_depth", Metric("nm", "not recorded")),
        ],
    ),
    (
        "kohetch",
        &[
            ("concentration", Set("%")),
            ("temp", Set("degC")),
            ("duration", Set("s")),
            ("rpm", Set("rpm")),
        ],
    ),
    (
        "nickelplating",
        &[
            ("solution", Attr),
            ("temp", Set("degC")),
            ("duration", Set("s")),
            ("sensitizer", Attr),
            ("activator", Attr),
            ("surfactant", Attr),
        ],
    ),
    (
        "patterning",
        &[
            ("underlying_material", Attr),
            ("cleaning_step", Attr),
            ("hdms_prebake_temp", Set("degC")),
            ("hdms_prebake_duration", Set("s")),
            ("hdms_spin_rpm", Set("rpm")),
            ("hdms_spin_duration", Set("s")),
            ("hdms_bake_temp", Set("degC")),
            ("hdms_bake_duration", Set("s")),
            ("photoresist_spin_rpm", Set("rpm")),
            ("photoresist_spin_duration", Set("s")),
            ("photoresist_bake_temp", Set("degC")),
            ("photoresist_bake_duration", Set("s")),
            ("exposure_pattern", Attr),
            ("exposure_duration", Set("ms")),
            ("develop_duration", Set("s")),
            ("metric_pattern_quality", Obs("1 (worst) to 5 (best)")),
            (
                "metric_development",
                Obs("well-developed / underdeveloped / overdeveloped"),
            ),
            (
                "metric_contaminants",
                Obs("1 (little to none) to 3 (significant)"),
            ),
        ],
    ),
    (
        "plasmaclean",
        &[
            ("o2_flow", Set("sccm")),
            ("rf_power", Set("W")),
            ("clean_duration", Set("s")),
            ("metric_contaminants", Obs("free text")),
        ],
    ),
    (
        "plasmaetch",
        &[
            ("o2_flow", Set("sccm")),
            ("sf6_flow", Set("sccm")),
            ("rf_power", Set("W")),
            ("etch_duration", Set("s")),
            ("etch_depth", Metric("nm", "not recorded")),
        ],
    ),
];

#[derive(Debug, Clone)]
pub struct DmOpts {
    pub prefix: String,
    pub site: Option<String>,
}

fn sval(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

fn dm_time(s: &str) -> Option<String> {
    let t = s.trim().replace(' ', "T");
    if crate::validate::parse_time(&t).is_some() {
        return Some(t);
    }
    // Django writes "+00:00" or "Z"; a naive time has no zone and cannot be made RFC 3339.
    None
}

/// One process row (model name lower-case, field map) into run, recipe, metrology, observations.
fn dm_row(
    ds: &mut Dataset,
    src: &Source,
    o: &DmOpts,
    model: &str,
    pk: &str,
    fields: &BTreeMap<String, Value>,
    locator: String,
) {
    let Some((_, map)) = DM.iter().find(|(m, _)| *m == model) else {
        return;
    };
    let chip = fields
        .get("chip_number")
        .or_else(|| fields.get("chip_number_id"))
        .and_then(sval);
    let Some(chip) = chip else { return };
    let chip_id = format!("{}/chip-{chip}", o.prefix);
    let run_id = format!("{}/{model}/{pk}", o.prefix);
    // Field names carry the model's CamelCase prefix: AluminumEtch_temp.
    let get = |suffix: &str| {
        fields
            .iter()
            .find(|(k, _)| k.to_lowercase() == format!("{model}_{suffix}"))
            .map(|x| x.1)
    };
    let mut settings = Vec::new();
    let mut actuals = Vec::new();
    let mut attrs = Vec::new();
    for (suffix, role) in map.iter() {
        let Some(raw) = get(suffix).and_then(sval) else {
            continue;
        };
        let name = suffix.trim_start_matches("metric_").to_string();
        // Two models record a "layer_thickness" in different units (Å, µm): different films,
        // so different parameters.
        let metro_name = format!("{model}_{name}");
        let parsed = raw
            .parse::<f64>()
            .ok()
            .or_else(|| number_unit(&raw).filter(|(_, u)| u.is_empty()).map(|x| x.0));
        match (role, parsed) {
            (Set(u), Some(v)) => settings.push(setpoint(&name, v, u)),
            (Act(u), Some(v)) => actuals.push(Parameter {
                name: name.clone(),
                kind: kind_for(&name, u),
                quantity: Quantity::unknown_u(
                    v,
                    u,
                    "the data-management schema records no uncertainty",
                ),
            }),
            (Metric(u, method), Some(v)) => ds.metrology.push(Metrology {
                id: format!("{run_id}/{name}"),
                material: chip_id.clone(),
                after_run: Some(run_id.clone()),
                method: method.to_string(),
                tool: None,
                parameter: metro_name.clone(),
                kind: kind_for(&name, u),
                points: vec![MetroPoint {
                    site: None,
                    position: None,
                    value: Quantity::unknown_u(
                        v,
                        u,
                        "the data-management schema records no uncertainty",
                    ),
                }],
                conditions: vec![],
                attributes: vec![],
                at: None,
                provenance: src.prov("hackerfab::dm", Some(format!("{locator} field {suffix}"))),
                license: src.lic(),
            }),
            (Obs(scale), _) => ds.observations.push(Observation {
                id: format!("{run_id}/{name}"),
                material: chip_id.clone(),
                after_run: Some(run_id.clone()),
                method: "visual inspection (operator)".into(),
                name: name.clone(),
                value: raw.clone(),
                scale: Some(scale.to_string()),
                provenance: src.prov("hackerfab::dm", Some(format!("{locator} field {suffix}"))),
                license: src.lic(),
            }),
            // Not a number where a number belongs: kept as text, never coerced.
            (Set(_) | Act(_) | Metric(..) | Attr, _) => attrs.push(Attribute {
                name: if matches!(role, Attr) {
                    name
                } else {
                    format!("{name}_unparsed")
                },
                value: raw,
            }),
        }
    }
    if let Some(n) = get("notes").and_then(sval) {
        attrs.push(Attribute {
            name: "notes".into(),
            value: n,
        });
    }
    if let Some(p) = fields.get("picture").and_then(sval) {
        attrs.push(Attribute {
            name: "picture_file".into(),
            value: p,
        });
    }
    let start = get("step_time").and_then(sval);
    let start_rfc = start.as_deref().and_then(dm_time);
    if let (Some(s), None) = (&start, &start_rfc) {
        attrs.push(Attribute {
            name: "step_time_unparsed".into(),
            value: s.clone(),
        });
    }
    // A recipe per distinct setting set: the same parameters on another chip reuse it.
    let key = serde_json::to_string(&settings).unwrap_or_default();
    let h = key.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
    });
    let recipe_id = format!("{}/recipe/{model}/{h:016x}", o.prefix);
    if !ds.recipes.iter().any(|r| r.id == recipe_id) {
        ds.recipes.push(Recipe {
            id: recipe_id.clone(),
            name: format!("{model} (as recorded in data-management)"),
            version: format!("{h:016x}"),
            supersedes: None,
            step_kind: Some(model.into()),
            tool_class: None,
            parameters: settings,
            attributes: vec![],
            provenance: src.prov_note(
                "hackerfab::dm",
                Some(locator.clone()),
                "Recipe reconstructed from the parameters an operator entered for a run.",
            ),
            license: src.lic(),
        });
    }
    ds.runs.push(ProcessRun {
        id: run_id,
        recipe: Some(recipe_id),
        tool: format!("{model} (tool id not recorded)"),
        chamber: None,
        route: None,
        step_no: None,
        step_name: Some(model.into()),
        material: vec![chip_id],
        start: start_rfc,
        end: None,
        operator_role: Some("not recorded (chip_owner user id dropped)".into()),
        status: RunStatus::Unknown,
        settings: vec![],
        actuals,
        attributes: attrs,
        provenance: src.prov("hackerfab::dm", Some(locator)),
        license: src.lic(),
    });
}

fn dm_chip(
    ds: &mut Dataset,
    src: &Source,
    o: &DmOpts,
    chip: &str,
    fields: &BTreeMap<String, Value>,
    locator: String,
) {
    let id = format!("{}/chip-{chip}", o.prefix);
    if ds.units.iter().any(|u| u.id == id) {
        return;
    }
    let substrate = fields
        .get("starting_material")
        .and_then(sval)
        .map(|m| Substrate {
            material: m,
            dopant_type: None,
            orientation: None,
            properties: vec![],
        });
    let site = fields
        .get("university")
        .and_then(sval)
        .or_else(|| o.site.clone());
    ds.units.push(MaterialUnit {
        id: id.clone(),
        kind: UnitKind::Chip,
        parent: None,
        label: Some(format!("chip {chip}")),
        site,
        substrate,
        position: None,
        provenance: src.prov("hackerfab::dm", Some(locator.clone())),
        license: src.lic(),
    });
    if let Some(t) = fields
        .get("creation_time")
        .and_then(sval)
        .and_then(|t| dm_time(&t))
    {
        ds.events.push(GenealogyEvent {
            id: format!("{id}/create"),
            op: GenealogyOp::Create,
            inputs: vec![],
            outputs: vec![id],
            at: Some(t),
            run: None,
            reason: None,
            provenance: src.prov("hackerfab::dm", Some(locator)),
            license: src.lic(),
        });
    }
}

/// A Django `python manage.py dumpdata data_management` export: a JSON array of
/// `{"model": "data_management.<model>", "pk": ..., "fields": {...}}`. Chips become units,
/// each process row a run with a recipe of its setpoints, `*_metric_*` numbers metrology,
/// grades and yes/no answers observations. `chip_owner` (a user id) is dropped: the schema
/// records roles, never people.
pub fn dm_dumpdata(text: &str, src: &Source, o: &DmOpts) -> Result<Dataset, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let rows = v.as_array().ok_or("dumpdata output is a JSON array")?;
    let mut ds = src.dataset(
        &format!("{}-ds", o.prefix),
        "Hacker Fab data-management export",
    );
    // Chips first, so process rows can reference them.
    for (i, r) in rows.iter().enumerate() {
        let model = r
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let fields: BTreeMap<String, Value> = r
            .get("fields")
            .and_then(Value::as_object)
            .map(|m| m.clone().into_iter().collect())
            .unwrap_or_default();
        if model.ends_with(".chiplist") {
            let pk = r.get("pk").and_then(sval).unwrap_or_default();
            dm_chip(
                &mut ds,
                src,
                o,
                &pk,
                &fields,
                format!("element {i} ({model} pk {pk})"),
            );
        }
    }
    for (i, r) in rows.iter().enumerate() {
        let model = r
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let short = model.rsplit('.').next().unwrap_or("").to_string();
        let pk = r.get("pk").and_then(sval).unwrap_or_else(|| i.to_string());
        let fields: BTreeMap<String, Value> = r
            .get("fields")
            .and_then(Value::as_object)
            .map(|m| m.clone().into_iter().collect())
            .unwrap_or_default();
        if let Some(chip) = fields.get("chip_number").and_then(sval) {
            if !ds
                .units
                .iter()
                .any(|u| u.id == format!("{}/chip-{chip}", o.prefix))
            {
                dm_chip(
                    &mut ds,
                    src,
                    o,
                    &chip,
                    &BTreeMap::new(),
                    format!("element {i} (referenced chip)"),
                );
            }
        }
        dm_row(
            &mut ds,
            src,
            o,
            &short,
            &pk,
            &fields,
            format!("element {i} ({model} pk {pk})"),
        );
    }
    Ok(ds)
}

/// The portal's search CSV (`views.py` `create_csv_include_process`): tables separated by blank
/// lines, each with a header of model field names and a `process` column.
pub fn dm_search_csv(text: &str, src: &Source, o: &DmOpts) -> Result<Dataset, String> {
    let recs = crate::columnar::parse_csv(text)?;
    let mut ds = src.dataset(
        &format!("{}-ds", o.prefix),
        "Hacker Fab data-management search export",
    );
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<(String, String, BTreeMap<String, Value>, usize)> = Vec::new();
    for (n, r) in recs.iter().enumerate() {
        if r.iter().all(|f| f.0.trim().is_empty()) {
            header = None;
            continue;
        }
        match &header {
            None => header = Some(r.iter().map(|f| f.0.trim().to_string()).collect()),
            Some(h) => {
                let m: BTreeMap<String, Value> = h
                    .iter()
                    .cloned()
                    .zip(r.iter().map(|f| Value::String(f.0.clone())))
                    .collect();
                let process = m
                    .get("process")
                    .and_then(sval)
                    .unwrap_or_default()
                    .to_lowercase();
                let pk = m.get("id").and_then(sval).unwrap_or_else(|| n.to_string());
                rows.push((process, pk, m, n + 1));
            }
        }
    }
    for (process, pk, m, line) in &rows {
        let chip = m
            .get("chip_number_id")
            .or_else(|| m.get("chip_number"))
            .and_then(sval);
        if let Some(c) = chip {
            let fields = if process == "chiplist" {
                m.clone()
            } else {
                BTreeMap::new()
            };
            dm_chip(&mut ds, src, o, &c, &fields, format!("CSV line {line}"));
        }
        if process != "chiplist" {
            dm_row(&mut ds, src, o, process, pk, m, format!("CSV line {line}"));
        }
    }
    Ok(ds)
}
