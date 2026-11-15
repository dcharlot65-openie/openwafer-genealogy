//! Genealogy queries: walk recipe → run → trace → metrology → electrical result.
//!
//! The walk follows material, not names. A run processed some units; everything below them
//! in the hierarchy (chips cleaved from a wafer, dies on a chip, test sites on a die) and
//! everything a split or merge made from them inherits that run. So an electrical test on a
//! die joins every run that touched the die's wafer, which is what "this transistor's
//! history" means.
//!
//! Every result is labelled with the origins it touched. Under [`JoinPolicy::MeasuredOnly`]
//! a walk that reaches a SIMULATED record fails instead of returning a mixed answer.

use super::record::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinPolicy {
    /// Fail if any joined record is SIMULATED.
    MeasuredOnly,
    /// Return everything, labelled with the set of origins in the result.
    AllowMixedLabelled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinError {
    NoSuchRecipe(String),
    NoSuchUnit(String),
    /// The walk reached SIMULATED records while the policy asked for measured data only.
    SimulatedInMeasuredJoin {
        records: Vec<String>,
    },
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinError::NoSuchRecipe(r) => write!(f, "no recipe '{r}'"),
            JoinError::NoSuchUnit(u) => write!(f, "no material unit '{u}'"),
            JoinError::SimulatedInMeasuredJoin { records } => {
                write!(
                    f,
                    "a measured-only join reached {} SIMULATED record(s): {}",
                    records.len(),
                    records.join(", ")
                )
            }
        }
    }
}

/// Everything joined to one run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunLineage<'a> {
    pub run: &'a ProcessRun,
    /// The units the run processed and everything descended from them.
    pub reach: Vec<String>,
    pub traces: Vec<&'a Trace>,
    pub metrology: Vec<&'a Metrology>,
    pub electrical: Vec<&'a ElectricalTest>,
    pub observations: Vec<&'a Observation>,
    pub defects: Vec<&'a Defect>,
    pub bin_maps: Vec<&'a BinMap>,
    pub yields: Vec<&'a YieldRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lineage<'a> {
    pub recipes: Vec<&'a Recipe>,
    pub runs: Vec<RunLineage<'a>>,
    pub origins: BTreeSet<Origin>,
}

fn origin(p: &Option<Provenance>) -> Option<Origin> {
    p.as_ref().map(|p| p.origin)
}

/// Children of every unit: by parent link, and by split and merge events.
pub fn children(ds: &Dataset) -> BTreeMap<&str, Vec<&str>> {
    let mut m: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for u in &ds.units {
        if let Some(p) = &u.parent {
            m.entry(p.as_str()).or_default().push(u.id.as_str());
        }
    }
    for e in &ds.events {
        if matches!(e.op, GenealogyOp::Split | GenealogyOp::Merge) {
            for i in &e.inputs {
                for o in &e.outputs {
                    m.entry(i.as_str()).or_default().push(o.as_str());
                }
            }
        }
    }
    m
}

/// A unit and everything descended from it.
pub fn descendants<'a>(ds: &'a Dataset, roots: &[&'a str]) -> BTreeSet<&'a str> {
    let ch = children(ds);
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut q: VecDeque<&str> = roots.iter().copied().collect();
    while let Some(u) = q.pop_front() {
        if seen.insert(u) {
            if let Some(cs) = ch.get(u) {
                q.extend(cs.iter().copied());
            }
        }
    }
    seen
}

/// A unit and every unit it came from: parents, and the inputs of the splits and merges
/// that produced it.
pub fn ancestors<'a>(ds: &'a Dataset, unit: &'a str) -> BTreeSet<&'a str> {
    let mut parents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for u in &ds.units {
        if let Some(p) = &u.parent {
            parents.entry(u.id.as_str()).or_default().push(p.as_str());
        }
    }
    for e in &ds.events {
        if matches!(e.op, GenealogyOp::Split | GenealogyOp::Merge) {
            for o in &e.outputs {
                for i in &e.inputs {
                    parents.entry(o.as_str()).or_default().push(i.as_str());
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut q = VecDeque::from([unit]);
    while let Some(u) = q.pop_front() {
        if seen.insert(u) {
            if let Some(ps) = parents.get(u) {
                q.extend(ps.iter().copied());
            }
        }
    }
    seen
}

fn run_lineage<'a>(ds: &'a Dataset, run: &'a ProcessRun) -> RunLineage<'a> {
    let roots: Vec<&str> = run.material.iter().map(String::as_str).collect();
    let reach = descendants(ds, &roots);
    let on = |material: &str, after: &Option<String>| match after {
        Some(r) => r == &run.id,
        None => reach.contains(material),
    };
    RunLineage {
        run,
        reach: reach.iter().map(|s| s.to_string()).collect(),
        traces: ds.traces.iter().filter(|t| t.run == run.id).collect(),
        metrology: ds
            .metrology
            .iter()
            .filter(|m| on(&m.material, &m.after_run))
            .collect(),
        electrical: ds
            .electrical
            .iter()
            .filter(|e| reach.contains(e.material.as_str()))
            .collect(),
        observations: ds
            .observations
            .iter()
            .filter(|o| on(&o.material, &o.after_run))
            .collect(),
        defects: ds
            .defects
            .iter()
            .filter(|d| on(&d.material, &d.after_run))
            .collect(),
        bin_maps: ds
            .bin_maps
            .iter()
            .filter(|b| on(&b.material, &b.after_run))
            .collect(),
        yields: ds
            .yields
            .iter()
            .filter(|y| reach.contains(y.scope.as_str()))
            .collect(),
    }
}

fn label<'a>(
    recipes: &[&'a Recipe],
    runs: &[RunLineage<'a>],
    policy: JoinPolicy,
) -> Result<BTreeSet<Origin>, JoinError> {
    let mut origins = BTreeSet::new();
    let mut simulated = Vec::new();
    let mut note = |table: &str, id: &str, p: &Option<Provenance>| {
        if let Some(o) = origin(p) {
            origins.insert(o);
            if o == Origin::Simulated {
                simulated.push(format!("{table}/{id}"));
            }
        }
    };
    for r in recipes {
        note("recipes", &r.id, &r.provenance);
    }
    for rl in runs {
        note("runs", &rl.run.id, &rl.run.provenance);
        for t in &rl.traces {
            note("traces", &t.id, &t.provenance);
        }
        for m in &rl.metrology {
            note("metrology", &m.id, &m.provenance);
        }
        for e in &rl.electrical {
            note("electrical", &e.id, &e.provenance);
        }
        for o in &rl.observations {
            note("observations", &o.id, &o.provenance);
        }
        for d in &rl.defects {
            note("defects", &d.id, &d.provenance);
        }
        for b in &rl.bin_maps {
            note("bin_maps", &b.id, &b.provenance);
        }
        for y in &rl.yields {
            note("yields", &y.id, &y.provenance);
        }
    }
    if policy == JoinPolicy::MeasuredOnly && !simulated.is_empty() {
        simulated.sort();
        simulated.dedup();
        return Err(JoinError::SimulatedInMeasuredJoin { records: simulated });
    }
    Ok(origins)
}

/// Every run of a recipe (and of the versions it supersedes or that supersede it), with the
/// traces, metrology and electrical results joined to each.
pub fn lineage<'a>(
    ds: &'a Dataset,
    recipe_id: &str,
    policy: JoinPolicy,
) -> Result<Lineage<'a>, JoinError> {
    let root = ds
        .recipes
        .iter()
        .find(|r| r.id == recipe_id)
        .ok_or_else(|| JoinError::NoSuchRecipe(recipe_id.into()))?;
    // The version family: follow `supersedes` both ways.
    let mut family: BTreeSet<&str> = BTreeSet::from([root.id.as_str()]);
    loop {
        let before = family.len();
        for r in &ds.recipes {
            let linked = r.supersedes.as_deref().is_some_and(|s| family.contains(s));
            let back = family.iter().any(|f| {
                ds.recipes
                    .iter()
                    .any(|x| x.id == *f && x.supersedes.as_deref() == Some(r.id.as_str()))
            });
            if linked || back {
                family.insert(r.id.as_str());
            }
        }
        if family.len() == before {
            break;
        }
    }
    let recipes: Vec<&Recipe> = ds
        .recipes
        .iter()
        .filter(|r| family.contains(r.id.as_str()))
        .collect();
    let runs: Vec<RunLineage> = ds
        .runs
        .iter()
        .filter(|r| r.recipe.as_deref().is_some_and(|x| family.contains(x)))
        .map(|r| run_lineage(ds, r))
        .collect();
    let origins = label(&recipes, &runs, policy)?;
    Ok(Lineage {
        recipes,
        runs,
        origins,
    })
}

/// One unit's history: every run that processed it or anything it came from, in time order,
/// each with what was measured.
pub fn history<'a>(
    ds: &'a Dataset,
    unit: &str,
    policy: JoinPolicy,
) -> Result<Lineage<'a>, JoinError> {
    let u = ds
        .units
        .iter()
        .find(|x| x.id == unit)
        .ok_or_else(|| JoinError::NoSuchUnit(unit.into()))?;
    let anc = ancestors(ds, u.id.as_str());
    let mut runs: Vec<&ProcessRun> = ds
        .runs
        .iter()
        .filter(|r| r.material.iter().any(|m| anc.contains(m.as_str())))
        .collect();
    runs.sort_by(|a, b| {
        let t = |r: &ProcessRun| {
            r.start
                .as_deref()
                .and_then(super::validate::parse_time)
                .unwrap_or(f64::INFINITY)
        };
        t(a).partial_cmp(&t(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    let mut recipe_ids: BTreeSet<&str> = BTreeSet::new();
    for r in &runs {
        if let Some(x) = &r.recipe {
            recipe_ids.insert(x);
        }
    }
    let recipes: Vec<&Recipe> = ds
        .recipes
        .iter()
        .filter(|r| recipe_ids.contains(r.id.as_str()))
        .collect();
    let desc = descendants(ds, &[u.id.as_str()]);
    let runs: Vec<RunLineage> = runs
        .into_iter()
        .map(|r| {
            let mut rl = run_lineage(ds, r);
            // Keep only what concerns this unit: measurements on it or below it.
            rl.metrology.retain(|m| desc.contains(m.material.as_str()));
            rl.electrical.retain(|e| desc.contains(e.material.as_str()));
            rl.observations
                .retain(|o| desc.contains(o.material.as_str()));
            rl.defects.retain(|d| desc.contains(d.material.as_str()));
            rl.bin_maps.retain(|b| desc.contains(b.material.as_str()));
            rl.yields
                .retain(|y| desc.contains(y.scope.as_str()) || anc.contains(y.scope.as_str()));
            rl
        })
        .collect();
    let origins = label(&recipes, &runs, policy)?;
    Ok(Lineage {
        recipes,
        runs,
        origins,
    })
}
