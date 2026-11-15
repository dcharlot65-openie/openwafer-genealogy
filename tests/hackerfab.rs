//! The Hacker Fab adapters on real public files (committed under `hackerfab/raw/` with
//! their licences) and on synthetic files in each format with no public data.
//! Independent checks are computed here from the raw text, not from the adapter.

use openwafer_genealogy::adapters::{hackerfab as hf, sheet, Source};
use openwafer_genealogy::query::{self, JoinPolicy};
use openwafer_genealogy::record::*;
use openwafer_genealogy::validate::{validate, Code};
use std::path::PathBuf;

fn path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}
fn read(rel: &str) -> String {
    std::fs::read_to_string(path(rel)).unwrap()
}
const GITBOOK: &str = "6256296bb0531805a0207048ac86141c17ad72d9";
const DOCS_LICENSE: &str = "CC-BY-SA-4.0 AND CERN-OHL-S-2.0";

fn gitbook(file: &str, origin: Origin) -> Source {
    let mut s = Source::new(
        &format!("https://github.com/hacker-fab/gitbook/blob/{GITBOOK}/{file}"),
        DOCS_LICENSE,
        origin,
    );
    s.version = Some(GITBOOK.into());
    s.retrieved = Some("2026-11-15".into());
    s.license.attribution = Some("Hacker Fab contributors (hacker-fab/gitbook)".into());
    s
}
fn synthetic(format: &str) -> Source {
    let mut s = Source::new(
        &format!("synthetic://{format}"),
        "CC0-1.0",
        Origin::Simulated,
    );
    s.note = Some("Synthetic test input in the format; not data from any fab.".into());
    s
}
fn clean(ds: &Dataset) {
    let r = validate(ds);
    for i in &r.issues {
        eprintln!(
            "  {:?} {:?} {}/{} {}: {}",
            i.severity, i.code, i.table, i.id, i.path, i.message
        );
    }
    assert!(r.ok(), "expected no errors");
}

#[test]
fn four_point_probe_reproduces_the_published_sheet_resistance() {
    let text = read("hackerfab/raw/4pp-150nm-al-glass.csv");
    let src = gitbook(
        ".gitbook/assets/4 point probe data 150nm thick Al.csv",
        Origin::Measured,
    );
    let o = hf::SweepOpts {
        test_id: "hf-cmu/film/al-150nm-glass/4pp".into(),
        material: "hf-cmu/film/al-150nm-glass".into(),
        structure: "collinear 4-point probe".into(),
        thickness: Some(Quantity::unknown_u(
            150.0,
            "nm",
            "stated film thickness; method not given",
        )),
        thickness_ref: Some("hf-cmu/film/recipe/al-15sccm-4e-3hpa-150nm".into()),
    };
    let mut ds = hf::smu_sweep_csv(&text, &src, &o).unwrap();
    // Independent: mean of V/I over the raw rows, times π/ln 2.
    let mut r = Vec::new();
    for l in text.lines().skip(1) {
        let f: Vec<f64> = l
            .split(',')
            .skip(5)
            .filter_map(|s| s.parse().ok())
            .collect();
        r.push(f[1] / f[0]);
    }
    let rs_indep =
        std::f64::consts::PI / std::f64::consts::LN_2 * r.iter().sum::<f64>() / r.len() as f64;
    let e = &ds.electrical[0];
    let rs = e
        .extracted
        .iter()
        .find(|x| x.name == "sheet_resistance")
        .unwrap();
    let rho = e
        .extracted
        .iter()
        .find(|x| x.name == "resistivity")
        .unwrap();
    eprintln!("Rs = {:.5} ohm/sq (independent {rs_indep:.5}; page 0.229), rho = {:.4e} ohm.m (page 3.44e-8), {} readings", rs.quantity.value, rho.quantity.value, r.len());
    assert_eq!(r.len(), 25);
    assert!((rs.quantity.value - rs_indep).abs() < 1e-12);
    assert!(
        (rs.quantity.value - 0.229).abs() / 0.229 < 0.005,
        "the page's 0.229 Ω/sq is reproduced to 0.5%"
    );
    assert!(
        (rho.quantity.value - 3.44e-8).abs() / 3.44e-8 < 0.01,
        "and its 3.44e-8 Ω·m to 1%"
    );
    assert!(
        matches!(rho.quantity.uncertainty, Some(Uncertainty::Unknown { .. })),
        "a nominal thickness gives the resistivity no stated uncertainty"
    );
    // Join it to the transcribed run that made the film.
    let film = sheet::run_sheet(
        &read("hackerfab/transcribed/film-characterization.csv"),
        "hf-film",
        "film characterization",
        DeclaredOrigin::Measured,
    )
    .unwrap();
    ds.extend(film);
    clean(&ds);
    let l = query::lineage(
        &ds,
        "hf-cmu/film/recipe/al-15sccm-4e-3hpa-150nm",
        JoinPolicy::MeasuredOnly,
    )
    .unwrap();
    assert_eq!(l.runs.len(), 1);
    assert_eq!(
        l.runs[0].electrical.len(),
        1,
        "recipe → run → electrical result"
    );
}

#[test]
fn the_transcribed_film_page_joins_recipes_runs_and_metrology() {
    let ds = sheet::run_sheet(
        &read("hackerfab/transcribed/film-characterization.csv"),
        "hf-film",
        "film characterization",
        DeclaredOrigin::Measured,
    )
    .unwrap();
    clean(&ds);
    assert_eq!(
        (ds.units.len(), ds.runs.len(), ds.metrology.len()),
        (11, 11, 10)
    );
    let refl = ds
        .metrology
        .iter()
        .find(|m| m.method == "optical reflectometry")
        .unwrap();
    assert!(
        matches!(refl.points[0].value.uncertainty, Some(Uncertainty::Standard { u, .. }) if (u - 1.8 / 5f64.sqrt()).abs() < 1e-3)
    );
    // Control for the run sheet's own refusal: drop the unit from a header and the value is refused.
    let broken = read("hackerfab/transcribed/film-characterization.csv")
        .replace("meas:rms_roughness [nm]", "meas:rms_roughness");
    let ds2 = sheet::run_sheet(&broken, "x", "x", DeclaredOrigin::Measured).unwrap();
    assert_eq!(
        validate(&ds2).count(Code::MissingUnit),
        7,
        "the seven AFM values lose their unit"
    );
}

#[test]
fn the_published_traveler_is_recipes_only_and_two_steps_have_a_unit_error() {
    let bytes = std::fs::read(path("hackerfab/raw/Sputtered_Gate_NMOS_S26.xlsx")).unwrap();
    let src = gitbook(
        ".gitbook/assets/Sputtered_Gate_NMOS_S26.xlsx",
        Origin::Specified,
    );
    let ds = hf::traveler_xlsx(
        &bytes,
        &src,
        "hf-cmu/sputtered-gate-nmos-s26",
        "Sputtered Gate NMOS S26",
    )
    .unwrap();
    clean(&ds);
    assert_eq!(ds.recipes.len(), 52, "steps 0 to 51");
    assert!(
        ds.runs.is_empty() && ds.metrology.is_empty() && ds.electrical.is_empty(),
        "a traveler is a plan"
    );
    let r = validate(&ds);
    let warn: Vec<&str> = r
        .issues
        .iter()
        .filter(|i| i.code == Code::ScaleSuspect)
        .map(|i| i.id.as_str())
        .collect();
    eprintln!("scale warnings: {warn:?}");
    for i in r.issues.iter().filter(|i| i.code == Code::ScaleSuspect) {
        eprintln!("  {}: {}", i.id, i.message);
    }
    // Steps 28 and 45: "Deposition Rate: 2.5 Å/s" for 150 nm in 60 min and 300 nm in 120 min.
    // 2.5 nm/min closes both; 2.5 Å/s overshoots by exactly the Å/s → nm/min factor, 6.
    assert!(warn.contains(&"hf-cmu/sputtered-gate-nmos-s26/step-28"));
    assert!(warn.contains(&"hf-cmu/sputtered-gate-nmos-s26/step-45"));
    assert!(r
        .issues
        .iter()
        .filter(|i| i.code == Code::ScaleSuspect)
        .all(|i| i.message.contains("Å/s ↔ nm/min")));
    // Step 27 (Al2O3, ".25 Å/s", 20 nm, 80 min) closes as written in nm/min too: 0.25 Å/s × 80 min = 120 nm.
    let s27 = ds
        .recipes
        .iter()
        .find(|x| x.id.ends_with("step-27"))
        .unwrap();
    let rate = s27
        .parameters
        .iter()
        .find(|p| p.name == "deposition_rate")
        .unwrap();
    assert_eq!(rate.quantity.unit.as_deref(), Some("angstrom/s"));
    assert!(warn.contains(&"hf-cmu/sputtered-gate-nmos-s26/step-27"));
    let s0 = ds
        .recipes
        .iter()
        .find(|x| x.id.ends_with("step-00"))
        .unwrap();
    assert!(s0
        .parameters
        .iter()
        .any(|p| p.name == "substrate_thickness"));
}

fn report(g: u32) -> Dataset {
    let text = read(&format!("hackerfab/raw/march-2026-group-{g}-report.md"));
    let src = gitbook(
        &format!(
            "nmos-capabilities/current-fab-capabilities/march-2026-update/group-{g}-report.md"
        ),
        Origin::Measured,
    );
    let o = hf::ReportOpts {
        prefix: format!("hf-cmu/s26-g{g}"),
        group_unit: format!("hf-cmu/s26-g{g}"),
        chips: vec![("Chip 19".into(), "hf-cmu/s26-g1/chip-19".into()), ("Chip 50".into(), "hf-cmu/s26-g1/chip-50".into())],
        site: Some("CMU".into()),
        methods: vec![
            ("Vth".into(), "linear extrapolation of Id–Vgs at peak gm, Vd_ref = 5 V; Vgs 0–6 V in 1 V steps (group 1 report §5.2)".into()),
            ("Ion/Ioff".into(), "Ion / Ioff (group 1 report §5.2)".into()),
            ("Ion".into(), "drain current at Vgs = 6 V, Vds = 5 V (group 1 report §5.2)".into()),
            ("Ioff".into(), "drain current at Vgs = 0 V, Vds = 5 V (group 1 report §5.2)".into()),
            ("Ron".into(), "5 V / Ion (group 1 report §5.2)".into()),
        ],
    };
    hf::report_md(&text, &src, &o).unwrap()
}

#[test]
fn group_reports_give_yields_and_per_size_statistics_that_add_up() {
    let g1 = report(1);
    clean(&g1);
    let y: Vec<(String, u64, u64)> = g1
        .yields
        .iter()
        .map(|y| (y.stage.clone(), y.passed, y.tested))
        .collect();
    eprintln!("group 1 yields: {y:?}");
    assert!(y.contains(&("visual".into(), 900, 952)));
    assert!(y.contains(&("electrical".into(), 94, 100)));
    assert!(
        y.contains(&("electrical".into(), 48, 50)) && y.contains(&("electrical".into(), 46, 50))
    );
    let per_size: Vec<&Summary> = g1
        .summaries
        .iter()
        .filter(|s| s.population.starts_with("NMOS"))
        .collect();
    assert_eq!(
        per_size.len(),
        2 * 7 * 4,
        "two chips × seven sizes × four parameters"
    );
    // Independent cross-table check: per-size counts sum to the chip's functional devices, and
    // the count-weighted per-size means reproduce the pooled chip mean.
    for (chip, functional, pooled_vth) in [
        ("hf-cmu/s26-g1/chip-19", 48u64, 1.26),
        ("hf-cmu/s26-g1/chip-50", 46, 3.43),
    ] {
        let vth: Vec<&&Summary> = per_size
            .iter()
            .filter(|s| s.scope == chip && s.parameter == "Vth")
            .collect();
        let n: u64 = vth.iter().map(|s| s.n).sum();
        let mean = vth.iter().map(|s| s.n as f64 * s.mean).sum::<f64>() / n as f64;
        eprintln!("{chip}: n = {n} (report {functional}), weighted Vth = {mean:.3} V (report {pooled_vth})");
        assert_eq!(n, functional);
        assert!((mean - pooled_vth).abs() < 0.01);
    }
    // Group 2 and 3 yields.
    let (g2, g3) = (report(2), report(3));
    let ey = |d: &Dataset| {
        d.yields
            .iter()
            .filter(|y| y.stage == "electrical")
            .map(|y| (y.passed, y.tested))
            .collect::<Vec<_>>()
    };
    assert_eq!(ey(&g2), vec![(48, 56)]);
    assert_eq!(ey(&g3), vec![(68, 112)]);
    // Pooled electrical yield over the three groups' probed devices.
    let (p, t) = [(94u64, 100u64), (48, 56), (68, 112)]
        .iter()
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    eprintln!(
        "pooled electrical yield, three groups: {p}/{t} = {:.1}%",
        100.0 * p as f64 / t as f64
    );
}

#[test]
fn p901_pressure_log_starts_at_atmosphere_in_torr() {
    let text = read("hackerfab/raw/p901-output.csv");
    let mut src = Source::new("https://github.com/hacker-fab/uw-p901-reader/blob/a1c6426824000ee08fed928dffc36ea01971127a/901p-serial/output.csv", "CERN-OHL-S-2.0", Origin::Measured);
    src.version = Some("a1c6426824000ee08fed928dffc36ea01971127a".into());
    let o = hf::TraceRunOpts {
        run_id: "hf-uw/p901/pumpdown".into(),
        tool: "vacuum chamber with MKS 901P transducer".into(),
        step_name: "pump-down".into(),
        status: RunStatus::Completed,
        material: vec![],
        note: None,
    };
    let ds = hf::p901_csv(&text, &src, &o, "Torr", "the 901P's factory unit; the first reading, 741, is ambient pressure in Torr (988 hPa), not in hPa or Pa").unwrap();
    clean(&ds);
    let t = &ds.traces[0];
    assert_eq!(
        t.values.len(),
        text.lines().filter(|l| !l.trim().is_empty()).count()
    );
    assert_eq!(t.values[0], Some(741.0));
    let min = t
        .values
        .iter()
        .flatten()
        .cloned()
        .fold(f64::INFINITY, f64::min);
    eprintln!(
        "{} samples over {:.0} s, {} Torr to {min:.2e} Torr",
        t.values.len(),
        t.times_s.last().unwrap(),
        t.values[0].unwrap()
    );
}

#[test]
fn impedance_matcher_vswr_below_one_is_flagged_not_dropped() {
    let text = read("hackerfab/raw/impedance-test2-first399.csv");
    let src = gitbook(".gitbook/assets/test2.csv", Origin::Measured);
    let o = hf::TraceRunOpts {
        run_id: "hf-cmu/impedance/test2".into(),
        tool: "CMU RF sputtering chamber with automated impedance matcher".into(),
        step_name: "RF plasma, impedance-matching test".into(),
        status: RunStatus::Faulted,
        material: vec![],
        note: Some("plasma went out after 16 minutes (page)".into()),
    };
    let ds = hf::impedance_csv(&text, &src, &o).unwrap();
    clean(&ds);
    // Independent count from the raw column.
    let below = text
        .lines()
        .skip(1)
        .filter(|l| {
            l.split(',')
                .nth(2)
                .and_then(|s| s.parse::<f64>().ok())
                .is_some_and(|v| v < 1.0)
        })
        .count();
    let vswr = ds.traces.iter().find(|t| t.sensor == "vswr").unwrap();
    eprintln!(
        "{} samples, {below} below VSWR 1, {} flagged",
        vswr.values.len(),
        vswr.flags.len()
    );
    assert_eq!(vswr.values.len(), 399);
    assert_eq!(
        vswr.flags
            .iter()
            .filter(|f| f.code == "physically_impossible")
            .count(),
        below
    );
    assert!(below > 0);
    // Control: strip the flags and the validator reports the impossible samples.
    let mut d2 = ds.clone();
    d2.traces.iter_mut().for_each(|t| t.flags.clear());
    assert!(validate(&d2).has(Code::PhysicalRange));
}

#[test]
fn impedance_matcher_out_of_order_times_and_firmware_resets_are_flagged() {
    // The shape of hacker-fab/gitbook test3.csv: host time steps back 5.4 ms once, and
    // device_millis falls when the matcher restarts.
    let text = "host_time_s,device_millis,vswr,forward_v,reverse_v,motor1_pos_rad,motor2_pos_rad,at_match\n\
                100.000,1000,1.2,nan,nan,1.0,nan,1\n\
                100.500,1500,1.1,nan,nan,1.0,nan,1\n\
                100.4946,1600,1.3,nan,nan,1.0,nan,1\n\
                101.000,20,1.2,nan,nan,1.0,nan,0\n";
    let o = hf::TraceRunOpts {
        run_id: "fx/imp".into(),
        tool: "matcher".into(),
        step_name: "test".into(),
        status: RunStatus::Completed,
        material: vec![],
        note: None,
    };
    let ds = hf::impedance_csv(text, &synthetic("impedance"), &o).unwrap();
    clean(&ds);
    let t = &ds.traces[0];
    assert_eq!(t.times_s, vec![0.0, 0.5, 0.5, 1.0]);
    assert!(t
        .flags
        .iter()
        .any(|f| f.index == 2 && f.code.starts_with("time_out_of_order")));
    assert!(t
        .flags
        .iter()
        .any(|f| f.index == 3 && f.code.starts_with("device_reset")));
}

#[test]
fn data_management_dumpdata_becomes_runs_recipes_metrology_and_observations() {
    let ds = hf::dm_dumpdata(
        &read("templates/dm-dumpdata-example.json"),
        &synthetic("dm-dumpdata"),
        &hf::DmOpts {
            prefix: "fx-dm".into(),
            site: None,
        },
    )
    .unwrap();
    clean(&ds);
    eprintln!("{:?}", ds.counts());
    assert_eq!(ds.units.len(), 2);
    assert_eq!(ds.runs.len(), 4);
    assert_eq!(
        ds.recipes.len(),
        3,
        "two patterning runs with equal settings share one recipe"
    );
    // Two models record a "layer thickness" in Å and in µm: they stay separate parameters.
    let p: Vec<&str> = ds.metrology.iter().map(|m| m.parameter.as_str()).collect();
    assert!(
        p.contains(&"aluminumevaporation_layer_thickness")
            && p.contains(&"glassdeposition_layer_thickness")
    );
    assert!(ds
        .observations
        .iter()
        .any(|o| o.name == "pattern_quality" && o.value == "4"));
    let text = openwafer_genealogy::jsonl::to_jsonl(&ds);
    assert!(
        !text.contains("chip_owner\":") && !text.contains("\"owner"),
        "no person identifiers survive"
    );
    let l = query::lineage(
        &ds,
        &ds.runs[0].recipe.clone().unwrap(),
        JoinPolicy::AllowMixedLabelled,
    )
    .unwrap();
    assert!(!l.runs.is_empty());
}

#[test]
fn data_management_search_csv_matches_the_dumpdata_mapping() {
    let ds = hf::dm_search_csv(
        &read("templates/dm-search-example.csv"),
        &synthetic("dm-search-csv"),
        &hf::DmOpts {
            prefix: "fx-dm".into(),
            site: Some("CMU".into()),
        },
    )
    .unwrap();
    clean(&ds);
    assert_eq!(ds.runs.len(), 2);
    assert!(ds
        .metrology
        .iter()
        .any(|m| m.parameter == "plasmaetch_etch_depth"));
}

#[test]
fn smu_py_pair_carries_the_current_derivation() {
    let ds = hf::smu_pair(
        &read("templates/smu_305_2b1_currents.csv"),
        &read("templates/smu_305_2b1_voltages.csv"),
        &synthetic("smu.py"),
        "fx/smu/305-2b1",
        "fx/chip-305",
        100.0,
    )
    .unwrap();
    let e = &ds.electrical[0];
    assert_eq!(e.curves.len(), 3);
    assert!(e.curves.iter().all(|c| c
        .y
        .derivation
        .as_ref()
        .is_some_and(|d| d.method.contains("100 Ω"))));
    // The chip is not in this dataset: the link must be reported, not assumed.
    assert!(validate(&ds).has(Code::BrokenLink));
}

#[test]
fn furnace_recipe_and_nmos_summary_formats() {
    let ds = hf::furnace_recipe(
        &read("templates/furnace-recipe-example.json"),
        &synthetic("furnace-recipe"),
        "fx/furnace",
        "1",
    )
    .unwrap();
    clean(&ds);
    assert_eq!(ds.recipes[0].parameters.len(), 6);
    let mut base = Dataset::new(Header::new("x", "x", DeclaredOrigin::Simulated));
    base.units.push(MaterialUnit {
        id: "fx/chip-19".into(),
        kind: UnitKind::Chip,
        parent: None,
        label: None,
        site: None,
        substrate: None,
        position: None,
        provenance: synthetic("nmos").prov("test", None),
        license: synthetic("nmos").lic(),
    });
    let n = hf::nmos_summary_csv(
        &read("templates/nmos-summary-example.csv"),
        None,
        &synthetic("nmos-summary"),
        &hf::NmosOpts {
            prefix: "fx".into(),
            chips: vec![("19".into(), "fx/chip-19".into())],
            gate_length_um: vec![(1, 5.0), (2, 10.0)],
            methods: vec![
                ("Vth".into(), "linear extrapolation at peak gm".into()),
                ("I".into(), "read at bias".into()),
                ("R".into(), "V / I".into()),
            ],
            site: None,
        },
    )
    .unwrap();
    base.extend(n);
    clean(&base);
    assert_eq!(base.electrical.len(), 2);
    assert!(base.electrical[0]
        .extracted
        .iter()
        .any(|x| x.name == "Vth_at_Vd_5V" && x.quantity.unit.as_deref() == Some("V")));
}

#[test]
fn the_run_sheet_template_imports() {
    let ds = sheet::run_sheet(
        &read("templates/run-sheet-example.csv"),
        "tpl",
        "template",
        DeclaredOrigin::Simulated,
    )
    .unwrap();
    clean(&ds);
    assert!(!ds.recipes.is_empty() && !ds.runs.is_empty() && !ds.metrology.is_empty());
}
