//! `owg …`: validate, import, export, merge and query genealogy datasets.
//!
//! A site turns its logs into records with `import <adapter>`, checks them with `validate`,
//! and publishes them with `export --format csv|parquet|jsonl`. See `CONTRIBUTING.md`.

use clap::{Args, Parser, Subcommand, ValueEnum};
use openwafer_genealogy::adapters::{self, hackerfab as hf, Source};
use openwafer_genealogy::columnar;
use openwafer_genealogy::jsonl;
use openwafer_genealogy::query::{self, JoinPolicy};
use openwafer_genealogy::record::*;
use openwafer_genealogy::validate::{self, Severity};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "owg",
    version,
    about = "The open genealogy schema: recipes, runs, traces, metrology, electrical results"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Jsonl,
    Csv,
    Parquet,
}

#[derive(Clone, Copy, ValueEnum)]
enum OriginArg {
    Measured,
    Specified,
    Simulated,
}

#[derive(Clone, Copy, ValueEnum)]
enum Declared {
    Measured,
    Simulated,
    Mixed,
}

#[derive(Clone, Copy, ValueEnum)]
enum Adapter {
    /// Hacker Fab data-management `manage.py dumpdata` JSON.
    DmDumpdata,
    /// Hacker Fab data-management portal search CSV.
    DmSearchCsv,
    /// smu.py `_currents.csv` and `_voltages.csv` (two inputs).
    SmuPair,
    /// SMU sweep export (`_Id,_Iteration,…,<name> (<unit>)`), e.g. a four-point probe.
    SmuSweep,
    /// impedance-matcher telemetry CSV.
    Impedance,
    /// MKS 901P reader log (`t_s,pressure`).
    P901,
    /// tube-furnace recipe JSON.
    FurnaceRecipe,
    /// process traveler (.xlsx or .csv).
    Traveler,
    /// NMOS characterization report (markdown).
    ReportMd,
    /// nmos_summary.csv (+ optional per-gate-voltage CSV).
    NmosSummary,
    /// A run sheet exported as CSV (see CONTRIBUTING.md); provenance and licence come from its columns.
    RunSheet,
}

#[derive(Args)]
struct ImportArgs {
    adapter: Adapter,
    /// Input files.
    inputs: Vec<PathBuf>,
    #[arg(long)]
    out: PathBuf,
    /// Id prefix for the records (for example `hf-cmu`).
    #[arg(long, default_value = "site")]
    prefix: String,
    /// Source URL, ideally pinned to a commit.
    #[arg(long)]
    url: Option<String>,
    /// Commit, release or file hash of the source.
    #[arg(long)]
    commit: Option<String>,
    #[arg(long)]
    retrieved: Option<String>,
    /// SPDX licence expression of the source. Required: a record without a licence is refused.
    #[arg(long)]
    license: Option<String>,
    #[arg(long)]
    attribution: Option<String>,
    #[arg(long, value_enum, default_value = "measured")]
    origin: OriginArg,
    #[arg(long)]
    site: Option<String>,
    #[arg(long)]
    note: Option<String>,
    /// Material unit the data belongs to.
    #[arg(long)]
    material: Option<String>,
    #[arg(long)]
    test_id: Option<String>,
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    tool: Option<String>,
    #[arg(long)]
    step: Option<String>,
    #[arg(long, default_value = "completed")]
    status: String,
    /// Unit of an input that does not record one (P901 pressure).
    #[arg(long)]
    unit: Option<String>,
    /// Why that unit is right.
    #[arg(long)]
    unit_basis: Option<String>,
    /// Film thickness for resistivity, as "150 nm".
    #[arg(long)]
    thickness: Option<String>,
    #[arg(long)]
    thickness_ref: Option<String>,
    #[arg(long, default_value_t = 100.0)]
    r_drain: f64,
    /// Traveler or recipe name/version.
    #[arg(long)]
    route: Option<String>,
    /// "Chip 19=hf-cmu/chip-19" (report) or "19=hf-cmu/chip-19" (nmos summary); repeatable.
    #[arg(long)]
    chip: Vec<String>,
    #[arg(long)]
    group_unit: Option<String>,
    /// "Vth=linear extrapolation…"; repeatable.
    #[arg(long)]
    method: Vec<String>,
    /// "1=5" (NMOS number = gate length in µm); repeatable.
    #[arg(long)]
    gate_length: Vec<String>,
    /// Append to this dataset instead of starting a new one.
    #[arg(long)]
    merge_into: Option<PathBuf>,
    /// Drop records with record-level errors instead of refusing the whole import.
    #[arg(long)]
    drop_invalid: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Validate a dataset (JSON Lines file, or a CSV/Parquet export directory).
    Validate {
        paths: Vec<PathBuf>,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Convert a site's files into validated records.
    Import(Box<ImportArgs>),
    /// Write a dataset as JSON Lines, CSV + manifest, or Parquet + manifest.
    Export {
        input: PathBuf,
        #[arg(long, value_enum)]
        format: Format,
        #[arg(long)]
        out: PathBuf,
        /// Keep only records whose licence allows redistribution.
        #[arg(long)]
        redistributable_only: bool,
    },
    /// Concatenate datasets.
    Merge {
        inputs: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        title: String,
        #[arg(long, value_enum)]
        declared: Declared,
        #[arg(long)]
        description: Option<String>,
    },
    /// Print the JSON Schema.
    Schema {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Walk recipe → run → trace → metrology → electrical result.
    Lineage {
        input: PathBuf,
        #[arg(long)]
        recipe: String,
        #[arg(long)]
        allow_mixed: bool,
    },
    /// Every run and measurement behind one material unit.
    History {
        input: PathBuf,
        #[arg(long)]
        unit: String,
        #[arg(long)]
        allow_mixed: bool,
    },
    /// Record counts per table.
    Stats { input: PathBuf },
}

fn load(p: &Path) -> Result<Dataset, String> {
    if p.is_dir() {
        let m = std::fs::read_to_string(p.join("manifest.json"))
            .map_err(|e| format!("{}: {e}", p.display()))?;
        if m.contains("\"format\": \"parquet\"") {
            #[cfg(feature = "parquet")]
            return columnar::parquet_io::read_parquet(p);
            #[cfg(not(feature = "parquet"))]
            return Err("this build has no Parquet support (build with --features parquet)".into());
        }
        return columnar::read_csv(p);
    }
    jsonl::from_jsonl(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?)
}

fn print_report(r: &validate::Report, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(r).unwrap());
        return;
    }
    for i in &r.issues {
        let s = if i.severity == Severity::Error {
            "error"
        } else {
            "warning"
        };
        println!(
            "{s} [{:?}] {}/{} {}: {}",
            i.code, i.table, i.id, i.path, i.message
        );
    }
    let (e, w) = (r.errors().count(), r.warnings().count());
    println!("{e} errors, {w} warnings");
}

fn kv(list: &[String], sep: char) -> Vec<(String, String)> {
    list.iter()
        .filter_map(|s| {
            s.split_once(sep)
                .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
        })
        .collect()
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn import(a: &ImportArgs) -> Result<Dataset, String> {
    if let Adapter::RunSheet = a.adapter {
        let first = a.inputs.first().ok_or("no input file")?;
        let declared = match a.origin {
            OriginArg::Simulated => DeclaredOrigin::Simulated,
            _ => DeclaredOrigin::Measured,
        };
        return adapters::sheet::run_sheet(
            &read(first)?,
            &a.prefix,
            a.route.as_deref().unwrap_or("run sheet"),
            declared,
        );
    }
    let license = a.license.clone().ok_or("--license is required: a record without a licence is refused (use NOASSERTION if it is unknown)")?;
    let origin = match a.origin {
        OriginArg::Measured => Origin::Measured,
        OriginArg::Specified => Origin::Specified,
        OriginArg::Simulated => Origin::Simulated,
    };
    let first = a.inputs.first().ok_or("no input file")?;
    let mut src = Source::new(
        a.url.as_deref().unwrap_or(&first.display().to_string()),
        &license,
        origin,
    );
    src.version = a.commit.clone();
    src.retrieved = a.retrieved.clone();
    src.license.attribution = a.attribution.clone();
    src.site = a.site.clone();
    src.note = a.note.clone();
    let status = match a.status.as_str() {
        "completed" => RunStatus::Completed,
        "aborted" => RunStatus::Aborted,
        "faulted" => RunStatus::Faulted,
        _ => RunStatus::Unknown,
    };
    let name = |x: &Option<String>, what: &str| {
        x.clone()
            .ok_or(format!("--{what} is required for this adapter"))
    };
    let run_opts = || -> Result<hf::TraceRunOpts, String> {
        Ok(hf::TraceRunOpts {
            run_id: name(&a.run_id, "run-id")?,
            tool: name(&a.tool, "tool")?,
            step_name: a.step.clone().unwrap_or_else(|| "run".into()),
            status,
            material: a.material.iter().cloned().collect(),
            note: a.note.clone(),
        })
    };
    let dm = hf::DmOpts {
        prefix: a.prefix.clone(),
        site: a.site.clone(),
    };
    match a.adapter {
        Adapter::DmDumpdata => hf::dm_dumpdata(&read(first)?, &src, &dm),
        Adapter::DmSearchCsv => hf::dm_search_csv(&read(first)?, &src, &dm),
        Adapter::SmuPair => {
            let v = a
                .inputs
                .get(1)
                .ok_or("smu-pair needs the currents file and the voltages file")?;
            hf::smu_pair(
                &read(first)?,
                &read(v)?,
                &src,
                &name(&a.test_id, "test-id")?,
                &name(&a.material, "material")?,
                a.r_drain,
            )
        }
        Adapter::SmuSweep => {
            let thickness = match &a.thickness {
                Some(t) => {
                    let (v, u) =
                        adapters::number_unit(t).ok_or("--thickness must read like '150 nm'")?;
                    Some(Quantity::unknown_u(
                        v,
                        &u,
                        "thickness given without an uncertainty",
                    ))
                }
                None => None,
            };
            let o = hf::SweepOpts {
                test_id: name(&a.test_id, "test-id")?,
                material: name(&a.material, "material")?,
                structure: a
                    .step
                    .clone()
                    .unwrap_or_else(|| "collinear 4-point probe".into()),
                thickness,
                thickness_ref: a.thickness_ref.clone(),
            };
            hf::smu_sweep_csv(&read(first)?, &src, &o)
        }
        Adapter::Impedance => hf::impedance_csv(&read(first)?, &src, &run_opts()?),
        Adapter::P901 => {
            let unit = name(&a.unit, "unit")?;
            let basis = name(&a.unit_basis, "unit-basis")?;
            hf::p901_csv(&read(first)?, &src, &run_opts()?, &unit, &basis)
        }
        Adapter::FurnaceRecipe => hf::furnace_recipe(
            &read(first)?,
            &src,
            &format!("{}/furnace-recipe", a.prefix),
            a.commit.as_deref().unwrap_or("unversioned"),
        ),
        Adapter::Traveler => {
            let route = a.route.clone().unwrap_or_else(|| "traveler".into());
            if first
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("xlsx"))
            {
                hf::traveler_xlsx(
                    &std::fs::read(first).map_err(|e| e.to_string())?,
                    &src,
                    &a.prefix,
                    &route,
                )
            } else {
                let rows: Vec<Vec<String>> = columnar::parse_csv(&read(first)?)?
                    .into_iter()
                    .map(|r| r.into_iter().map(|f| f.0).collect())
                    .collect();
                hf::traveler(&rows, &src, &a.prefix, &route)
            }
        }
        Adapter::ReportMd => hf::report_md(
            &read(first)?,
            &src,
            &hf::ReportOpts {
                prefix: a.prefix.clone(),
                group_unit: name(&a.group_unit, "group-unit")?,
                chips: kv(&a.chip, '='),
                site: a.site.clone(),
                methods: kv(&a.method, '='),
            },
        ),
        Adapter::RunSheet => unreachable!("handled above"),
        Adapter::NmosSummary => {
            let per_gate = match a.inputs.get(1) {
                Some(p) => Some(read(p)?),
                None => None,
            };
            let gl = kv(&a.gate_length, '=')
                .into_iter()
                .filter_map(|(k, v)| Some((k.parse().ok()?, v.parse().ok()?)))
                .collect();
            hf::nmos_summary_csv(
                &read(first)?,
                per_gate.as_deref(),
                &src,
                &hf::NmosOpts {
                    prefix: a.prefix.clone(),
                    chips: kv(&a.chip, '='),
                    gate_length_um: gl,
                    methods: kv(&a.method, '='),
                    site: a.site.clone(),
                },
            )
        }
    }
}

fn run() -> Result<bool, String> {
    let cmd = Cli::parse().cmd;
    match cmd {
        Cmd::Validate { paths, json } => {
            let mut ok = true;
            for p in &paths {
                let ds = load(p)?;
                let r = validate::validate(&ds);
                if !json {
                    println!("{}: {} records", p.display(), ds.len());
                }
                print_report(&r, json);
                ok &= r.ok();
            }
            Ok(ok)
        }
        Cmd::Import(a) => {
            let mut ds = import(&a)?;
            if let Some(m) = &a.merge_into {
                let mut base = load(m)?;
                base.extend(ds);
                ds = base;
            }
            let mut r = validate::validate(&ds);
            if !r.ok() && a.drop_invalid {
                let (kept, refused) = validate::refuse(&ds, &r);
                for (t, id) in &refused {
                    eprintln!("refused {t}/{id}");
                }
                ds = kept;
                r = validate::validate(&ds);
            }
            print_report(&r, false);
            if !r.ok() {
                eprintln!("not written: fix the errors, or pass --drop-invalid to drop records with record-level errors");
                return Ok(false);
            }
            std::fs::write(&a.out, jsonl::to_jsonl(&ds)).map_err(|e| e.to_string())?;
            println!("wrote {} records to {}", ds.len(), a.out.display());
            Ok(true)
        }
        Cmd::Export {
            input,
            format,
            out,
            redistributable_only,
        } => {
            let mut ds = load(&input)?;
            let r = validate::validate(&ds);
            if !r.ok() {
                print_report(&r, false);
                return Err("refusing to export a dataset with errors".into());
            }
            if redistributable_only {
                let keep = |l: &Option<License>| {
                    l.as_ref()
                        .is_some_and(|l| validate::redistributable(&l.spdx))
                };
                let before = ds.len();
                ds.units.retain(|x| keep(&x.license));
                ds.events.retain(|x| keep(&x.license));
                ds.recipes.retain(|x| keep(&x.license));
                ds.runs.retain(|x| keep(&x.license));
                ds.traces.retain(|x| keep(&x.license));
                ds.metrology.retain(|x| keep(&x.license));
                ds.electrical.retain(|x| keep(&x.license));
                ds.summaries.retain(|x| keep(&x.license));
                ds.yields.retain(|x| keep(&x.license));
                ds.defects.retain(|x| keep(&x.license));
                ds.bin_maps.retain(|x| keep(&x.license));
                ds.observations.retain(|x| keep(&x.license));
                ds.estimates.retain(|x| keep(&x.license));
                println!(
                    "kept {} of {before} records with a redistributable licence",
                    ds.len()
                );
                let r2 = validate::validate(&ds);
                if !r2.ok() {
                    print_report(&r2, false);
                    return Err("dropping non-redistributable records broke links: export the full dataset privately instead".into());
                }
            }
            match format {
                Format::Jsonl => {
                    std::fs::write(&out, jsonl::to_jsonl(&ds)).map_err(|e| e.to_string())?
                }
                Format::Csv => {
                    let m = columnar::write_csv(&ds, &out)?;
                    println!("{} tables", m.tables.len());
                }
                Format::Parquet => {
                    #[cfg(feature = "parquet")]
                    {
                        let m = columnar::parquet_io::write_parquet(&ds, &out)?;
                        println!("{} tables", m.tables.len());
                    }
                    #[cfg(not(feature = "parquet"))]
                    return Err(
                        "this build has no Parquet support (build with --features parquet)".into(),
                    );
                }
            }
            println!("wrote {}", out.display());
            Ok(true)
        }
        Cmd::Merge {
            inputs,
            out,
            id,
            title,
            declared,
            description,
        } => {
            let d = match declared {
                Declared::Measured => DeclaredOrigin::Measured,
                Declared::Simulated => DeclaredOrigin::Simulated,
                Declared::Mixed => DeclaredOrigin::Mixed,
            };
            let mut h = Header::new(&id, &title, d);
            h.description = description;
            let mut ds = Dataset::new(h);
            for p in &inputs {
                ds.extend(load(p)?);
            }
            let r = validate::validate(&ds);
            print_report(&r, false);
            if !r.ok() {
                return Ok(false);
            }
            std::fs::write(&out, jsonl::to_jsonl(&ds)).map_err(|e| e.to_string())?;
            println!("wrote {} records to {}", ds.len(), out.display());
            Ok(true)
        }
        Cmd::Schema { out } => {
            let s =
                serde_json::to_string_pretty(&openwafer_genealogy::json_schema()).unwrap() + "\n";
            match out {
                Some(p) => std::fs::write(p, s).map_err(|e| e.to_string())?,
                None => print!("{s}"),
            }
            Ok(true)
        }
        Cmd::Lineage {
            input,
            recipe,
            allow_mixed,
        } => {
            let ds = load(&input)?;
            let pol = if allow_mixed {
                JoinPolicy::AllowMixedLabelled
            } else {
                JoinPolicy::MeasuredOnly
            };
            let l = query::lineage(&ds, &recipe, pol).map_err(|e| e.to_string())?;
            println!(
                "recipes: {}",
                l.recipes
                    .iter()
                    .map(|r| format!("{}@{}", r.id, r.version))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!("origins: {:?}", l.origins);
            for rl in &l.runs {
                println!(
                    "run {} on {}: {} units reached, {} traces, {} metrology, {} electrical, {} observations, {} bin maps, {} yields",
                    rl.run.id,
                    rl.run.tool,
                    rl.reach.len(),
                    rl.traces.len(),
                    rl.metrology.len(),
                    rl.electrical.len(),
                    rl.observations.len(),
                    rl.bin_maps.len(),
                    rl.yields.len()
                );
            }
            Ok(true)
        }
        Cmd::History {
            input,
            unit,
            allow_mixed,
        } => {
            let ds = load(&input)?;
            let pol = if allow_mixed {
                JoinPolicy::AllowMixedLabelled
            } else {
                JoinPolicy::MeasuredOnly
            };
            let l = query::history(&ds, &unit, pol).map_err(|e| e.to_string())?;
            println!("origins: {:?}", l.origins);
            for rl in &l.runs {
                println!(
                    "{} {} ({}): {} metrology, {} electrical",
                    rl.run.start.as_deref().unwrap_or("?"),
                    rl.run.id,
                    rl.run.recipe.as_deref().unwrap_or("-"),
                    rl.metrology.len(),
                    rl.electrical.len()
                );
            }
            Ok(true)
        }
        Cmd::Stats { input } => {
            let ds = load(&input)?;
            println!(
                "{} ({}, {:?})",
                ds.header.dataset_id, ds.header.title, ds.header.declared_origin
            );
            for (t, n) in ds.counts() {
                println!("{t:14} {n}");
            }
            Ok(true)
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
