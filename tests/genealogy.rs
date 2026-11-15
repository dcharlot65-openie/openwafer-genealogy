//! The genealogy schema's verification: round trips, joins, and the validator against seeded
//! defects. Every defect test starts from a fixture that validates clean (the control) and
//! changes exactly one thing.

use openwafer_genealogy::columnar::{self, Cell};
use openwafer_genealogy::jsonl::{from_jsonl, to_jsonl};
use openwafer_genealogy::query::{self, JoinError, JoinPolicy};
use openwafer_genealogy::record::*;
use openwafer_genealogy::units::QuantityKind as K;
use openwafer_genealogy::validate::{refuse, validate, Code};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn prov(origin: Origin, loc: &str) -> Option<Provenance> {
    Some(Provenance {
        origin,
        source: "fixture://tests/genealogy.rs".into(),
        source_version: Some("1".into()),
        locator: Some(loc.into()),
        produced_by: "hand-built test fixture".into(),
        retrieved: Some("2026-11-15".into()),
        note: Some("Hand-built to exercise every table; not data from any fab.".into()),
    })
}
fn lic() -> Option<License> {
    Some(License {
        spdx: "CC-BY-4.0".into(),
        attribution: Some("fixture".into()),
        url: None,
    })
}
fn m() -> Option<Provenance> {
    prov(Origin::Measured, "m")
}
fn u(x: f64) -> Uncertainty {
    Uncertainty::Standard {
        u: x,
        basis: Some("fixture".into()),
    }
}
fn p(name: &str, kind: K, q: Quantity) -> Parameter {
    Parameter {
        name: name.into(),
        kind: Some(kind),
        quantity: q,
    }
}
fn unit(id: &str, kind: UnitKind, parent: Option<&str>) -> MaterialUnit {
    MaterialUnit {
        id: id.into(),
        kind,
        parent: parent.map(str::to_string),
        label: None,
        site: Some("fixture fab".into()),
        substrate: if kind == UnitKind::Wafer {
            Some(Substrate {
                material: "Si".into(),
                dopant_type: Some("p".into()),
                orientation: Some("(100)".into()),
                properties: vec![p("thickness", K::Length, Quantity::setpoint(525.0, "um"))],
            })
        } else {
            None
        },
        position: if kind == UnitKind::Die {
            Some(MapPosition {
                x_index: Some(3),
                y_index: Some(-1),
                x: Some(Quantity::setpoint(1.5, "mm")),
                y: None,
                origin: Some("center".into()),
            })
        } else {
            None
        },
        provenance: m(),
        license: lic(),
    }
}
fn event(
    id: &str,
    op: GenealogyOp,
    inputs: &[&str],
    outputs: &[&str],
    run: Option<&str>,
) -> GenealogyEvent {
    GenealogyEvent {
        id: id.into(),
        op,
        inputs: inputs.iter().map(|s| s.to_string()).collect(),
        outputs: outputs.iter().map(|s| s.to_string()).collect(),
        at: Some("2026-03-01T10:00:00Z".into()),
        run: run.map(str::to_string),
        reason: None,
        provenance: m(),
        license: lic(),
    }
}
fn sputter(id: &str, version: &str, power: f64, supersedes: Option<&str>) -> Recipe {
    Recipe {
        id: id.into(),
        name: "Al sputter".into(),
        version: version.into(),
        supersedes: supersedes.map(str::to_string),
        step_kind: Some("rf_sputter".into()),
        tool_class: Some("RF sputter".into()),
        parameters: vec![
            p("rf_power", K::Power, Quantity::setpoint(power, "W")),
            p("ar_flow", K::GasFlow, Quantity::setpoint(15.0, "sccm")),
            p(
                "chamber_pressure",
                K::Pressure,
                Quantity::setpoint(3e-3, "Torr"),
            ),
            p("film_thickness", K::Length, Quantity::setpoint(150.0, "nm")),
            p(
                "deposition_rate",
                K::Rate,
                Quantity::setpoint(2.5, "nm/min"),
            ),
            p("deposition_time", K::Time, Quantity::setpoint(60.0, "min")),
        ],
        attributes: vec![Attribute {
            name: "target".into(),
            value: "Al 99.999%".into(),
        }],
        provenance: prov(Origin::Specified, "recipe"),
        license: lic(),
    }
}
fn run(id: &str, recipe: &str, material: &str, start: &str, end: &str) -> ProcessRun {
    ProcessRun {
        id: id.into(),
        recipe: Some(recipe.into()),
        tool: "SPUTTER-1".into(),
        chamber: Some("A".into()),
        route: Some("fixture flow".into()),
        step_no: Some(1),
        step_name: Some("metal".into()),
        material: vec![material.into()],
        start: Some(start.into()),
        end: Some(end.into()),
        operator_role: Some("student".into()),
        status: RunStatus::Completed,
        settings: vec![],
        actuals: vec![p(
            "base_pressure",
            K::Pressure,
            Quantity::measured(1.2e-7, "hPa", u(0.1e-7)),
        )],
        attributes: vec![],
        provenance: m(),
        license: lic(),
    }
}
fn thickness(id: &str, material: &str, after: &str, v: f64) -> Metrology {
    Metrology {
        id: id.into(),
        material: material.into(),
        after_run: Some(after.into()),
        method: "XRR".into(),
        tool: Some("XRR-1".into()),
        parameter: "film_thickness".into(),
        kind: Some(K::Length),
        points: vec![
            MetroPoint {
                site: Some("center".into()),
                position: Some(MapPosition {
                    x_index: None,
                    y_index: None,
                    x: Some(Quantity::setpoint(0.0, "mm")),
                    y: Some(Quantity::setpoint(0.0, "mm")),
                    origin: None,
                }),
                value: Quantity::measured(v, "nm", u(0.5)),
            },
            MetroPoint {
                site: Some("edge".into()),
                position: None,
                value: Quantity::measured(v - 2.0, "nm", u(0.5)),
            },
        ],
        conditions: vec![p(
            "x_ray_wavelength",
            K::Length,
            Quantity::setpoint(0.15406, "nm"),
        )],
        attributes: vec![],
        at: Some("2026-03-02".into()),
        provenance: m(),
        license: lic(),
    }
}

/// Every table, every hierarchy operation, every join. Measured, and valid.
fn fixture() -> Dataset {
    let mut ds = Dataset::new(Header::new(
        "fixture",
        "every table",
        DeclaredOrigin::Measured,
    ));
    ds.units = vec![
        unit("fx/lot1", UnitKind::Lot, None),
        unit("fx/lot2", UnitKind::Lot, None),
        unit("fx/lot3", UnitKind::Lot, None),
        unit("fx/w1", UnitKind::Wafer, Some("fx/lot1")),
        unit("fx/w2", UnitKind::Wafer, Some("fx/lot1")),
        unit("fx/w3", UnitKind::Wafer, Some("fx/lot2")),
        unit("fx/w1/c1", UnitKind::Chip, Some("fx/w1")),
        unit("fx/w1/c2", UnitKind::Chip, Some("fx/w1")),
        unit("fx/w1/c1/d1", UnitKind::Die, Some("fx/w1/c1")),
        unit("fx/w1/c1/d1/t1", UnitKind::TestSite, Some("fx/w1/c1/d1")),
    ];
    ds.events = vec![
        event(
            "fx/e/create",
            GenealogyOp::Create,
            &[],
            &["fx/lot1", "fx/w1", "fx/w2"],
            None,
        ),
        event(
            "fx/e/cleave",
            GenealogyOp::Split,
            &["fx/w1"],
            &["fx/w1/c1", "fx/w1/c2"],
            None,
        ),
        event(
            "fx/e/merge",
            GenealogyOp::Merge,
            &["fx/lot1", "fx/lot2"],
            &["fx/lot3"],
            None,
        ),
        event(
            "fx/e/rework",
            GenealogyOp::Rework,
            &["fx/w2"],
            &[],
            Some("fx/run/litho-w2-1"),
        ),
        event("fx/e/scrap", GenealogyOp::Scrap, &["fx/w1/c2"], &[], None),
    ];
    let mut litho = sputter("fx/recipe/litho@1", "1", 0.0, None);
    litho.name = "expose".into();
    litho.step_kind = Some("litho_expose".into());
    litho.parameters = vec![p("exposure_time", K::Time, Quantity::setpoint(8.0, "s"))];
    ds.recipes = vec![
        sputter("fx/recipe/sputter@1", "1", 100.0, None),
        sputter(
            "fx/recipe/sputter@2",
            "2",
            120.0,
            Some("fx/recipe/sputter@1"),
        ),
        litho,
    ];
    let mut l1 = run(
        "fx/run/litho-w2-1",
        "fx/recipe/litho@1",
        "fx/w2",
        "2026-03-01T12:00:00Z",
        "2026-03-01T12:00:08Z",
    );
    let mut l2 = run(
        "fx/run/litho-w2-2",
        "fx/recipe/litho@1",
        "fx/w2",
        "2026-03-01T13:00:00Z",
        "2026-03-01T13:00:08Z",
    );
    for r in [&mut l1, &mut l2] {
        r.tool = "STEPPER-1".into();
        r.step_name = Some("litho".into());
        r.actuals.clear();
    }
    ds.runs = vec![
        run(
            "fx/run/sputter-w1",
            "fx/recipe/sputter@1",
            "fx/w1",
            "2026-03-01T09:00:00Z",
            "2026-03-01T10:00:00Z",
        ),
        run(
            "fx/run/sputter-w2",
            "fx/recipe/sputter@2",
            "fx/w2",
            "2026-03-01T10:30:00Z",
            "2026-03-01T11:30:00-05:00",
        ),
        l1,
        l2,
    ];
    ds.traces = vec![
        Trace {
            id: "fx/run/sputter-w1/vswr".into(),
            run: "fx/run/sputter-w1".into(),
            tool: "SPUTTER-1".into(),
            sensor: "vswr".into(),
            kind: Some(K::Dimensionless),
            unit: Some("1".into()),
            sample_rate_hz: Some(1.0),
            times_s: vec![],
            t0: Some("2026-03-01T09:00:00Z".into()),
            values: vec![Some(1.2), Some(1.1), Some(1.15), None],
            uncertainty: Some(Uncertainty::Unknown {
                reason: "detector not calibrated".into(),
            }),
            calibration: Some(Calibration {
                status: CalStatus::Unknown,
                date: None,
                reference: None,
            }),
            flags: vec![SampleFlag {
                index: 3,
                code: "missing".into(),
            }],
            provenance: m(),
            license: lic(),
        },
        Trace {
            id: "fx/run/sputter-w1/pressure".into(),
            run: "fx/run/sputter-w1".into(),
            tool: "SPUTTER-1".into(),
            sensor: "pirani".into(),
            kind: Some(K::Pressure),
            unit: Some("Torr".into()),
            sample_rate_hz: None,
            times_s: vec![0.0, 0.7, 1.4],
            t0: None,
            values: vec![Some(3.1e-3), Some(3.0e-3), Some(2.9e-3)],
            uncertainty: Some(Uncertainty::Expanded {
                u: 3e-4,
                k: 2.0,
                basis: Some("gauge spec".into()),
            }),
            calibration: Some(Calibration {
                status: CalStatus::Calibrated,
                date: Some("2026-01-10".into()),
                reference: Some("cert 123".into()),
            }),
            flags: vec![],
            provenance: m(),
            license: lic(),
        },
    ];
    ds.metrology = vec![
        thickness("fx/m/thk-w1", "fx/w1", "fx/run/sputter-w1", 148.0),
        thickness("fx/m/thk-w2", "fx/w2", "fx/run/sputter-w2", 151.0),
    ];
    ds.electrical = vec![
        ElectricalTest {
            id: "fx/e/rs-c1".into(),
            material: "fx/w1/c1".into(),
            structure: "collinear 4-point probe".into(),
            structure_params: vec![p("probe_spacing", K::Length, Quantity::setpoint(1.0, "mm"))],
            instrument: Some("SMU".into()),
            bias: vec![Bias {
                terminal: "force".into(),
                mode: BiasMode::Sweep {
                    start: Quantity::setpoint(0.01, "A"),
                    stop: Quantity::setpoint(0.08, "A"),
                    step: None,
                },
            }],
            curves: vec![Curve {
                name: "V(I)".into(),
                x: Axis {
                    name: "I".into(),
                    kind: Some(K::Current),
                    unit: Some("A".into()),
                    values: vec![Some(0.01), Some(0.02)],
                    derivation: None,
                },
                y: Axis {
                    name: "V".into(),
                    kind: Some(K::Voltage),
                    unit: Some("V".into()),
                    values: vec![Some(5e-4), Some(1e-3)],
                    derivation: None,
                },
                fixed: vec![],
                uncertainty: Some(u(1e-6)),
            }],
            extracted: vec![Extracted {
                name: "sheet_resistance".into(),
                kind: Some(K::SheetResistance),
                quantity: Quantity::derived(
                    0.2266,
                    "ohm/sq",
                    u(0.001),
                    "Rs = (π/ln 2)·mean(V/I)",
                    vec!["fx/e/rs-c1#curves[0]".into()],
                ),
            }],
            at: Some("2026-03-03".into()),
            attributes: vec![],
            provenance: m(),
            license: lic(),
        },
        ElectricalTest {
            id: "fx/e/nmos-t1".into(),
            material: "fx/w1/c1/d1/t1".into(),
            structure: "NMOS".into(),
            structure_params: vec![p("gate_length", K::Length, Quantity::setpoint(5.0, "um"))],
            instrument: None,
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
                    mode: BiasMode::Fixed {
                        value: Quantity::setpoint(5.0, "V"),
                    },
                },
            ],
            curves: vec![],
            extracted: vec![Extracted {
                name: "vth".into(),
                kind: Some(K::Voltage),
                quantity: Quantity::derived(
                    1.26,
                    "V",
                    Uncertainty::Unknown {
                        reason: "not reported".into(),
                    },
                    "linear extrapolation at peak gm",
                    vec![],
                ),
            }],
            at: None,
            attributes: vec![],
            provenance: m(),
            license: lic(),
        },
    ];
    ds.summaries = vec![Summary {
        id: "fx/s/vth".into(),
        scope: "fx/w1/c1".into(),
        population: "NMOS L = 5 µm".into(),
        parameter: "vth".into(),
        kind: Some(K::Voltage),
        unit: Some("V".into()),
        n: 1,
        mean: 1.26,
        sd: None,
        min: Some(1.26),
        median: Some(1.26),
        max: Some(1.26),
        method: Some("linear extrapolation at peak gm".into()),
        members: vec!["fx/e/nmos-t1".into()],
        provenance: m(),
        license: lic(),
    }];
    ds.yields = vec![YieldRecord {
        id: "fx/y/c1".into(),
        scope: "fx/w1/c1".into(),
        stage: "electrical".into(),
        criterion: "NMOS I–V".into(),
        tested: 10,
        passed: 9,
        provenance: m(),
        license: lic(),
    }];
    ds.defects = vec![Defect {
        id: "fx/d/blister".into(),
        material: "fx/w2".into(),
        after_run: Some("fx/run/sputter-w2".into()),
        class: "blister".into(),
        method: "optical microscope".into(),
        position: None,
        size: Some(Quantity::unknown_u(20.0, "um", "eyeballed")),
        count: Some(3),
        provenance: m(),
        license: lic(),
    }];
    ds.bin_maps = vec![BinMap {
        id: "fx/b/w1".into(),
        material: "fx/w1".into(),
        after_run: Some("fx/run/sputter-w1".into()),
        bin_defs: vec![
            BinDef {
                code: 1,
                name: "pass".into(),
                pass: true,
            },
            BinDef {
                code: 2,
                name: "fail".into(),
                pass: false,
            },
        ],
        dies: vec![
            DieBin {
                die: Some("fx/w1/c1/d1".into()),
                index: Some(0),
                position: None,
                bin: 1,
            },
            DieBin {
                die: None,
                index: Some(1),
                position: None,
                bin: 2,
            },
        ],
        provenance: m(),
        license: lic(),
    }];
    ds.observations = vec![Observation {
        id: "fx/o/pattern".into(),
        material: "fx/w2".into(),
        after_run: Some("fx/run/litho-w2-2".into()),
        method: "visual inspection".into(),
        name: "pattern_quality".into(),
        value: "4".into(),
        scale: Some("1 (worst) to 5 (best)".into()),
        provenance: m(),
        license: lic(),
    }];
    ds.estimates = vec![Estimate {
        id: "fx/est/rate".into(),
        scope: vec!["fx/w1".into()],
        parameter: "deposition_rate".into(),
        kind: Some(K::Rate),
        quantity: Quantity::derived(
            2.47,
            "nm/min",
            u(0.01),
            "thickness / deposition time",
            vec!["fx/m/thk-w1".into()],
        ),
        provenance: m(),
        license: lic(),
    }];
    ds
}

fn codes(ds: &Dataset) -> BTreeSet<Code> {
    validate(ds).issues.iter().map(|i| i.code).collect()
}

/// The control: the fixture is clean, so every defect below is the only thing wrong.
#[test]
fn the_fixture_validates_with_no_issue_at_all() {
    let ds = fixture();
    let r = validate(&ds);
    for i in &r.issues {
        eprintln!("{i:?}");
    }
    assert!(r.issues.is_empty(), "the control must be clean");
    assert_eq!(
        ds.counts().iter().filter(|c| c.1 == 0).count(),
        0,
        "every table is exercised"
    );
}

// ───────────────────────────── round trips ─────────────────────────────

#[test]
fn jsonl_round_trip_is_exact() {
    let ds = fixture();
    let text = to_jsonl(&ds);
    assert_eq!(from_jsonl(&text).unwrap(), ds);
    assert!(
        from_jsonl("{\"table\":\"runs\",\"record\":{}}").is_err(),
        "a record before the header is refused"
    );
    assert!(
        from_jsonl(&text.replace("\"table\":\"yields\"", "\"table\":\"yeilds\"")).is_err(),
        "an unknown table is refused"
    );
}

#[test]
fn columnar_round_trip_is_exact_and_the_comparison_has_power() {
    let ds = fixture();
    let tables = columnar::flatten(&ds);
    eprintln!(
        "{} tables: {}",
        tables.len(),
        tables
            .iter()
            .map(|t| format!("{}({})", t.name, t.rows.len()))
            .collect::<Vec<_>>()
            .join(" ")
    );
    assert_eq!(columnar::unflatten(&tables).unwrap(), ds);
    // A trace's samples become rows of a child table.
    let samples = tables
        .iter()
        .find(|t| t.name == "traces.values")
        .expect("trace samples table");
    assert_eq!(samples.rows.len(), 7);
    // Negative control: lose one column and the round trip must fail.
    let mut broken = tables.clone();
    let pts = broken
        .iter_mut()
        .find(|t| t.name == "metrology.points")
        .unwrap();
    let j = pts.columns.iter().position(|c| c == "value.unit").unwrap();
    pts.columns.remove(j);
    for r in &mut pts.rows {
        r.remove(j);
    }
    let back = columnar::unflatten(&broken).unwrap();
    assert_ne!(back, ds, "a lost column must not round-trip");
    assert!(
        validate(&back).has(Code::MissingUnit),
        "and the validator sees what was lost"
    );
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("owg-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn csv_round_trip_is_exact_and_tampering_is_caught() {
    let ds = fixture();
    let dir = tmp("csv");
    let m = columnar::write_csv(&ds, &dir).unwrap();
    assert_eq!(columnar::read_csv(&dir).unwrap(), ds);
    let pts = m
        .tables
        .iter()
        .find(|t| t.name == "metrology.points")
        .unwrap();
    assert_eq!(
        pts.units.get("value.value"),
        Some(&vec!["nm".to_string()]),
        "the manifest lists the unit of each value column"
    );
    // Change one byte: the hash catches it.
    let f = dir.join("metrology.points.csv");
    let text = std::fs::read_to_string(&f).unwrap();
    std::fs::write(&f, text.replacen("148.0", "149.0", 1)).unwrap();
    assert!(columnar::read_csv(&dir).unwrap_err().contains("SHA-256"));
    // Fix the hash: the file reads, and the dataset differs, so the round trip is exact, not approximate.
    let mut man: columnar::Manifest =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let b = std::fs::read(&f).unwrap();
    use sha2::Digest;
    let h: String = sha2::Sha256::digest(&b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect();
    man.tables
        .iter_mut()
        .find(|t| t.name == "metrology.points")
        .unwrap()
        .sha256 = h;
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string(&man).unwrap(),
    )
    .unwrap();
    assert_ne!(columnar::read_csv(&dir).unwrap(), ds);
    // An empty string and a missing value stay different.
    let mut ds2 = ds.clone();
    ds2.observations[0].value = String::new();
    ds2.observations[0].scale = None;
    let dir2 = tmp("csv2");
    columnar::write_csv(&ds2, &dir2).unwrap();
    assert_eq!(columnar::read_csv(&dir2).unwrap(), ds2);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[cfg(feature = "parquet")]
#[test]
fn parquet_round_trip_is_exact() {
    let ds = fixture();
    let dir = tmp("parquet");
    let m = columnar::parquet_io::write_parquet(&ds, &dir).unwrap();
    assert!(m.tables.iter().all(|t| dir.join(&t.file).exists()));
    assert_eq!(columnar::parquet_io::read_parquet(&dir).unwrap(), ds);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cells_keep_integers_floats_and_strings_apart() {
    let tables = columnar::flatten(&fixture());
    let y = tables.iter().find(|t| t.name == "yields").unwrap();
    let j = y.columns.iter().position(|c| c == "tested").unwrap();
    assert_eq!(y.rows[0][j], Cell::Int(10));
    let dt = y.dtypes();
    assert_eq!(dt[j], columnar::DType::Int);
}

// ───────────────────────────── joins ─────────────────────────────

#[test]
fn lineage_walks_recipe_run_trace_metrology_electrical() {
    let ds = fixture();
    let l = query::lineage(&ds, "fx/recipe/sputter@1", JoinPolicy::MeasuredOnly).unwrap();
    let ids = |v: Vec<&str>| v.into_iter().map(str::to_string).collect::<Vec<_>>();
    assert_eq!(
        l.recipes.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["fx/recipe/sputter@1", "fx/recipe/sputter@2"],
        "versions join"
    );
    assert_eq!(l.runs.len(), 2);
    let w1 = &l.runs[0];
    assert_eq!(w1.run.id, "fx/run/sputter-w1");
    assert_eq!(
        w1.traces.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        ids(vec!["fx/run/sputter-w1/vswr", "fx/run/sputter-w1/pressure"])
    );
    assert_eq!(
        w1.metrology
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ids(vec!["fx/m/thk-w1"])
    );
    // The wafer was cleaved: the chip, its die and the die's test site inherit the run.
    assert_eq!(
        w1.electrical
            .iter()
            .map(|e| e.id.clone())
            .collect::<Vec<_>>(),
        ids(vec!["fx/e/rs-c1", "fx/e/nmos-t1"])
    );
    assert_eq!(w1.bin_maps.len(), 1);
    assert_eq!(w1.yields.len(), 1);
    let w2 = &l.runs[1];
    assert_eq!(
        w2.metrology
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ids(vec!["fx/m/thk-w2"])
    );
    assert_eq!(w2.defects.len(), 1);
    assert!(w2.electrical.is_empty(), "nothing tested descends from w2");
    assert_eq!(
        l.origins,
        BTreeSet::from([Origin::Measured, Origin::Specified])
    );
}

#[test]
fn history_of_a_transistor_reaches_back_through_the_split() {
    let ds = fixture();
    let h = query::history(&ds, "fx/w1/c1/d1/t1", JoinPolicy::MeasuredOnly).unwrap();
    assert_eq!(
        h.runs.iter().map(|r| r.run.id.as_str()).collect::<Vec<_>>(),
        vec!["fx/run/sputter-w1"]
    );
    assert_eq!(
        h.runs[0]
            .electrical
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fx/e/nmos-t1"]
    );
    // The reworked wafer's history is in time order, both litho passes included.
    let h2 = query::history(&ds, "fx/w2", JoinPolicy::MeasuredOnly).unwrap();
    assert_eq!(
        h2.runs
            .iter()
            .map(|r| r.run.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "fx/run/sputter-w2",
            "fx/run/litho-w2-1",
            "fx/run/litho-w2-2"
        ]
    );
    // A merge makes the merged lot a descendant of both inputs.
    let d = query::descendants(&ds, &["fx/lot2"]);
    assert!(d.contains("fx/lot3") && d.contains("fx/w3"));
}

fn vm_on_w1() -> Metrology {
    let mut vm = thickness("fx/m/vm-w1", "fx/w1", "fx/run/sputter-w1", 149.0);
    vm.method = "virtual metrology".into();
    vm.points.truncate(1);
    vm.points[0].value = Quantity::derived(
        149.0,
        "nm",
        u(1.0),
        "ridge regression on trace features",
        vec!["fx/run/sputter-w1/vswr".into()],
    );
    vm.provenance = prov(Origin::Simulated, "vm");
    vm
}

#[test]
fn a_measured_only_join_refuses_a_simulated_record_and_a_mixed_one_labels_it() {
    let mut ds = fixture();
    ds.header.declared_origin = DeclaredOrigin::Mixed;
    ds.metrology.push(vm_on_w1());
    assert!(
        validate(&ds).ok(),
        "a model's prediction about a real wafer is legal in a MIXED dataset"
    );
    match query::lineage(&ds, "fx/recipe/sputter@1", JoinPolicy::MeasuredOnly) {
        Err(JoinError::SimulatedInMeasuredJoin { records }) => {
            assert_eq!(records, vec!["metrology/fx/m/vm-w1".to_string()])
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    let l = query::lineage(&ds, "fx/recipe/sputter@1", JoinPolicy::AllowMixedLabelled).unwrap();
    assert!(l.origins.contains(&Origin::Simulated) && l.origins.contains(&Origin::Measured));
    // Control: without the simulated record the measured-only join succeeds.
    assert!(query::lineage(&fixture(), "fx/recipe/sputter@1", JoinPolicy::MeasuredOnly).is_ok());
}

// ───────────────────────────── seeded defects ─────────────────────────────

/// Assert the control lacks `code`, the mutated dataset has it, and say what fired.
fn seeded(code: Code, mutate: impl FnOnce(&mut Dataset)) -> Dataset {
    let clean = fixture();
    assert!(
        !codes(&clean).contains(&code),
        "control already has {code:?}"
    );
    let mut ds = clean;
    mutate(&mut ds);
    let r = validate(&ds);
    let hits: Vec<_> = r.issues.iter().filter(|i| i.code == code).collect();
    for h in &hits {
        eprintln!(
            "  {:?} {}/{} {}: {}",
            h.code, h.table, h.id, h.path, h.message
        );
    }
    assert!(
        !hits.is_empty(),
        "seeded {code:?} was not caught; got {:?}",
        r.tally()
    );
    ds
}

#[test]
fn seeded_broken_genealogy_link() {
    seeded(Code::BrokenLink, |d| {
        d.runs[0].material[0] = "fx/no-such-wafer".into()
    });
    seeded(Code::BrokenLink, |d| {
        d.units[6].parent = Some("fx/no-such-wafer".into())
    });
    seeded(Code::BrokenLink, |d| {
        d.events[1].outputs.push("fx/ghost".into())
    });
    seeded(Code::BrokenLink, |d| {
        d.traces[0].run = "fx/run/never".into()
    });
    seeded(Code::GenealogyCycle, |d| {
        d.units[3].parent = Some("fx/w1/c1".into())
    });
    seeded(Code::BadParent, |d| {
        d.units[0].parent = Some("fx/w1".into())
    });
}

#[test]
fn seeded_missing_unit() {
    let ds = seeded(Code::MissingUnit, |d| {
        d.metrology[0].points[0].value.unit = None
    });
    let (kept, refused) = refuse(&ds, &validate(&ds));
    assert_eq!(
        refused,
        vec![("metrology".to_string(), "fx/m/thk-w1".to_string())]
    );
    assert!(
        validate(&kept).ok(),
        "dropping the refused record leaves a valid dataset"
    );
    seeded(Code::MissingUnit, |d| d.traces[1].unit = None);
    seeded(Code::MissingUnit, |d| {
        d.recipes[0].parameters[0].quantity.unit = None
    });
}

#[test]
fn seeded_unit_mismatch_nm_against_um() {
    // Converted correctly, but in a different unit from every other thickness: one column, one unit.
    let ds = seeded(Code::UnitDeclMismatch, |d| {
        let v = &mut d.metrology[1].points[0].value;
        v.value = 0.151;
        v.unit = Some("um".into());
        d.metrology[1].points[1].value.value = 0.149;
        d.metrology[1].points[1].value.unit = Some("um".into());
    });
    assert!(
        !validate(&ds).has(Code::UnitDimension),
        "µm is a length: the error is the mixed column, not the dimension"
    );
    // Mislabelled: the number is in nm, the label says µm. It also differs from its target by 1000×.
    let ds = seeded(Code::UnitDeclMismatch, |d| {
        d.metrology[1].points[0].value.unit = Some("um".into());
        d.metrology[1].points[1].value.unit = Some("um".into());
    });
    let r = validate(&ds);
    let w = r
        .issues
        .iter()
        .find(|i| i.code == Code::ScaleSuspect)
        .expect("the 1000× ratio against the recipe target is flagged");
    assert!(w.message.contains("nm ↔ µm"), "{}", w.message);
    // Wrong dimension altogether.
    seeded(Code::UnitDimension, |d| {
        d.recipes[0].parameters[2].quantity.unit = Some("nm".into())
    });
    seeded(Code::UnknownUnit, |d| {
        d.metrology[0].points[0].value.unit = Some("furlong".into())
    });
}

#[test]
fn seeded_missing_uncertainty_on_a_measured_value() {
    seeded(Code::MissingUncertainty, |d| {
        d.metrology[0].points[0].value.uncertainty = None
    });
    seeded(Code::MissingUncertainty, |d| {
        d.runs[0].actuals[0].quantity.uncertainty = None
    });
    seeded(Code::MissingUncertainty, |d| d.traces[0].uncertainty = None);
    seeded(Code::SetpointAsMeasurement, |d| {
        d.metrology[0].points[0].value.uncertainty = Some(Uncertainty::Setpoint { tolerance: None })
    });
    seeded(Code::NotASetpoint, |d| {
        d.recipes[0].parameters[0].quantity.uncertainty = Some(u(1.0))
    });
    seeded(Code::BadUncertainty, |d| {
        d.metrology[0].points[0].value.uncertainty =
            Some(Uncertainty::Unknown { reason: " ".into() })
    });
}

#[test]
fn seeded_missing_licence_or_provenance() {
    let ds = seeded(Code::MissingLicense, |d| d.observations[0].license = None);
    let (kept, refused) = refuse(&ds, &validate(&ds));
    assert_eq!(
        refused,
        vec![("observations".to_string(), "fx/o/pattern".to_string())]
    );
    assert!(validate(&kept).ok());
    seeded(Code::MissingLicense, |d| {
        d.yields[0].license = Some(License {
            spdx: "".into(),
            attribution: None,
            url: None,
        })
    });
    seeded(Code::MissingProvenance, |d| {
        d.electrical[1].provenance = None
    });
    seeded(Code::EmptyField, |d| {
        d.units[0].provenance.as_mut().unwrap().source = String::new()
    });
    // NOASSERTION is legal, flagged, and dropped by --redistributable-only.
    seeded(Code::NotRedistributable, |d| {
        d.defects[0].license = Some(License {
            spdx: "NOASSERTION".into(),
            attribution: None,
            url: None,
        })
    });
}

#[test]
fn seeded_simulated_record_in_a_measured_join() {
    // The run behind a measured thickness was simulated: real material cannot have been processed by it.
    let ds = seeded(Code::SimulatedInMeasuredJoin, |d| {
        d.runs[1].provenance = prov(Origin::Simulated, "sim")
    });
    assert!(
        validate(&ds).has(Code::OriginMix),
        "and the dataset declared MEASURED holds a SIMULATED record"
    );
    // Declaring the dataset MIXED does not excuse a measured record built on a simulated one.
    let ds = seeded(Code::SimulatedInMeasuredJoin, |d| {
        d.header.declared_origin = DeclaredOrigin::Mixed;
        d.runs[1].provenance = prov(Origin::Simulated, "sim");
    });
    assert!(!validate(&ds).has(Code::OriginMix));
    // A simulated record in a measured dataset, even unlinked.
    seeded(Code::OriginMix, |d| d.metrology.push(vm_on_w1()));
}

#[test]
fn seeded_methods_traces_counts_and_times() {
    seeded(Code::MissingMethod, |d| {
        d.estimates[0].quantity.derivation = None
    });
    seeded(Code::MissingMethod, |d| {
        d.electrical[1].extracted[0].quantity.derivation = None
    });
    seeded(Code::MissingMethod, |d| d.summaries[0].method = None);
    seeded(Code::MissingSampleRate, |d| {
        d.traces[0].sample_rate_hz = None
    });
    seeded(Code::MissingSampleRate, |d| {
        d.traces[1].sample_rate_hz = Some(10.0)
    });
    seeded(Code::TraceShape, |d| {
        d.traces[1].times_s = vec![0.0, 1.4, 0.7]
    });
    seeded(Code::MissingCalibration, |d| d.traces[0].calibration = None);
    seeded(Code::MissingCalibration, |d| {
        d.traces[1].calibration.as_mut().unwrap().date = None
    });
    seeded(Code::YieldCount, |d| d.yields[0].passed = 11);
    seeded(Code::SummaryCount, |d| d.summaries[0].n = 3);
    seeded(Code::TimeOrder, |d| {
        d.runs[0].end = Some("2026-03-01T08:59:59Z".into())
    });
    seeded(Code::BadTimestamp, |d| {
        d.runs[0].start = Some("1 March 2026".into())
    });
    seeded(Code::NonFinite, |d| {
        d.metrology[0].points[0].value.value = f64::NAN
    });
    seeded(Code::DuplicateId, |d| d.defects.push(d.defects[0].clone()));
    seeded(Code::PhysicalRange, |d| d.traces[0].values[1] = Some(0.8));
    seeded(Code::PhysicalRange, |d| {
        d.recipes[0].parameters[2].quantity.value = -1.0
    });
    // Deposition rate × time that does not close on the stated thickness, by the Å/s ↔ nm/min factor.
    let ds = seeded(Code::ScaleSuspect, |d| {
        d.recipes[0].parameters[4].quantity.unit = Some("angstrom/s".into())
    });
    assert!(validate(&ds)
        .issues
        .iter()
        .any(|i| i.message.contains("Å/s ↔ nm/min")));
}

// ───────────────────────────── schema export ─────────────────────────────

#[test]
fn the_committed_json_schema_matches_the_types() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/owg-0.1.0.schema.json");
    let generated =
        serde_json::to_string_pretty(&openwafer_genealogy::json_schema()).unwrap() + "\n";
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "schema/owg-0.1.0.schema.json is stale: regenerate with `owg schema --out {}`",
        path.display()
    );
    assert!(generated.contains("\"Provenance\"") && generated.contains("\"Uncertainty\""));
    assert_eq!(SCHEMA_VERSION, "0.1.0");
}
