//! Genealogy — which tool, which recipe, which moment touched this wafer.
//!
//! This is the record that pays for an MES. A lot fails at electrical test, six weeks and
//! 300 operations after the damage was done. The question is never "what is wrong with this
//! lot" — it is **"which tool did this, and what else did it touch?"** Both halves matter:
//! the first stops the bleeding, the second is containment, and without a genealogy neither
//! is answerable at all.
//!
//! # Commonality analysis, and why the naive query is wrong
//!
//! The obvious approach — *"find the tools common to every failing lot"* — does not work,
//! and fails in the direction that wastes the most time. In a fab with one lithography
//! cell, **every** lot goes through it, so LITHO-1 is common to all the failures and is
//! also completely innocent. The naive query returns it at the top of the list, along with
//! every other well-utilized tool.
//!
//! The question is not what the failures share; it is what the failures share **and the
//! passing lots avoid**. [`Genealogy::commonality`] scores each tool as
//!
//! ```text
//!   score = (fraction of FAILING lots it touched) − (fraction of PASSING lots it touched)
//! ```
//!
//! A tool that touched every failure and no passer scores **+1.0** — the culprit. A tool
//! everything runs through scores **0.0** — it explains nothing, however common it is. The
//! contrast against the passing lots is what carries the information, and
//! [`tests::commonality_finds_the_culprit_and_ignores_the_tool_everything_uses`] holds it
//! to exactly that.
//!
//! # The open genealogy schema
//!
//! [`Event`] is the MES's own minimal log. The submodules below are the schema a fab can
//! publish: [`record`] holds the versioned record types (material hierarchy with splits,
//! merges and rework; recipes; runs; tool traces; metrology; electrical tests; yield and
//! defects), every one with provenance, licence, units and uncertainty. [`validate`] enforces
//! the rules, [`query`] walks recipe → run → trace → metrology → electrical result, [`jsonl`]
//! and [`columnar`] read and write the interchange and training forms, and [`adapters`] turn
//! existing fab files into records. The written specification is
//! `SCHEMA.md`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub mod adapters;
pub mod columnar;
pub mod jsonl;
pub mod query;
pub mod record;
pub mod units;
pub mod validate;

/// The JSON Schema (draft 2020-12) of a [`record::Dataset`], generated from the Rust types,
/// so the published schema cannot drift from the code.
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(record::Dataset)).expect("the schema serializes")
}

pub type LotId = String;
pub type ToolId = String;

/// One operation, on one lot, on one tool, at one moment. The atom of traceability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Simulated seconds since epoch. Never a wall clock — a twin must be replayable.
    pub t_s: u64,
    pub lot: LotId,
    /// Step number within the route.
    pub step: u32,
    pub tool: ToolId,
    pub recipe: String,
    /// Any metrology captured at this step.
    #[serde(default)]
    pub readings: Vec<f64>,
}

/// A tool's score in a commonality analysis. Sorted by `score`, best suspect first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suspect {
    pub tool: ToolId,
    pub failing_hits: usize,
    pub passing_hits: usize,
    /// `failing_fraction − passing_fraction`, in `[−1, 1]`. +1 = touched every failure and
    /// no passer. 0 = touched everything equally, and therefore explains nothing.
    pub score: f64,
}

/// The append-only operation log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Genealogy {
    pub events: Vec<Event>,
}

impl Genealogy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, e: Event) {
        self.events.push(e);
    }

    /// Every tool that touched a lot, in order of first contact.
    pub fn tools_touching(&self, lot: &str) -> Vec<ToolId> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for e in self.events.iter().filter(|e| e.lot == lot) {
            if seen.insert(e.tool.clone()) {
                out.push(e.tool.clone());
            }
        }
        out
    }

    /// **Containment.** Every lot a tool touched in a window — who else is exposed.
    ///
    /// The other half of the answer: once a tool is implicated, this is the blast radius,
    /// and it is the query that decides how much material is at risk.
    pub fn lots_touched_by(&self, tool: &str, from_s: u64, to_s: u64) -> Vec<LotId> {
        let mut out: BTreeSet<LotId> = BTreeSet::new();
        for e in &self.events {
            if e.tool == tool && e.t_s >= from_s && e.t_s <= to_s {
                out.insert(e.lot.clone());
            }
        }
        out.into_iter().collect()
    }

    /// The full route a lot actually took — as-run, not as-planned.
    pub fn history(&self, lot: &str) -> Vec<&Event> {
        let mut v: Vec<&Event> = self.events.iter().filter(|e| e.lot == lot).collect();
        v.sort_by_key(|e| e.t_s);
        v
    }

    /// **Commonality analysis.** Rank tools by how well they separate failing lots from
    /// passing ones. See the module docs: the contrast is the whole point — a tool common
    /// to every failure is worthless as evidence if it is also common to every success.
    pub fn commonality(&self, failing: &[LotId], passing: &[LotId]) -> Vec<Suspect> {
        let touched = |lots: &[LotId]| -> BTreeMap<ToolId, usize> {
            let mut m: BTreeMap<ToolId, usize> = BTreeMap::new();
            for lot in lots {
                for t in self.tools_touching(lot) {
                    *m.entry(t).or_default() += 1;
                }
            }
            m
        };
        let f = touched(failing);
        let p = touched(passing);

        let (nf, np) = (failing.len().max(1) as f64, passing.len().max(1) as f64);
        let mut out: Vec<Suspect> = f
            .keys()
            .chain(p.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|tool| {
                let fh = f.get(tool).copied().unwrap_or(0);
                let ph = p.get(tool).copied().unwrap_or(0);
                Suspect {
                    tool: tool.clone(),
                    failing_hits: fh,
                    passing_hits: ph,
                    score: fh as f64 / nf - ph as f64 / np,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap()
                .then(a.tool.cmp(&b.tool))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small fab history: every lot goes through LITHO-1 (one lithography cell), then
    /// splits across two etchers, then a shared metrology tool. ETCH-B is quietly broken.
    fn history() -> (Genealogy, Vec<LotId>, Vec<LotId>) {
        let mut g = Genealogy::new();
        let mut failing = Vec::new();
        let mut passing = Vec::new();
        for i in 0..8 {
            let lot = format!("LOT{i:02}");
            let bad = i % 2 == 0; // even lots take the broken etcher
            let etch = if bad { "ETCH-B" } else { "ETCH-A" };
            let t = i as u64 * 1000;
            g.record(Event {
                t_s: t,
                lot: lot.clone(),
                step: 1,
                tool: "LITHO-1".into(),
                recipe: "resist_193i".into(),
                readings: vec![],
            });
            g.record(Event {
                t_s: t + 100,
                lot: lot.clone(),
                step: 2,
                tool: etch.into(),
                recipe: "poly_etch".into(),
                readings: vec![],
            });
            g.record(Event {
                t_s: t + 200,
                lot: lot.clone(),
                step: 3,
                tool: "CD-SEM".into(),
                recipe: "cd_measure".into(),
                readings: vec![if bad { 55.0 } else { 45.0 }],
            });
            if bad {
                failing.push(lot)
            } else {
                passing.push(lot)
            }
        }
        (g, failing, passing)
    }

    #[test]
    fn commonality_finds_the_culprit_and_ignores_the_tool_everything_uses() {
        // ★ The query the MES exists for, and the one the naive version gets wrong.
        //
        // LITHO-1 and CD-SEM are common to *every* failing lot — a "what do the failures
        // share?" search ranks them top. They are also common to every passing lot, so they
        // explain nothing. Only ETCH-B separates the two populations.
        let (g, failing, passing) = history();
        let ranked = g.commonality(&failing, &passing);
        for s in &ranked {
            eprintln!(
                "  {:8} fail {}/{}  pass {}/{}  score {:+.2}",
                s.tool,
                s.failing_hits,
                failing.len(),
                s.passing_hits,
                passing.len(),
                s.score
            );
        }

        let top = &ranked[0];
        assert_eq!(
            top.tool, "ETCH-B",
            "the culprit must rank first, got {ranked:?}"
        );
        assert!(
            (top.score - 1.0).abs() < 1e-9,
            "a perfect separator must score +1.0"
        );

        // The tools everything runs through must score exactly zero — present in every
        // failure, and therefore evidence of nothing.
        for name in ["LITHO-1", "CD-SEM"] {
            let s = ranked.iter().find(|s| s.tool == name).unwrap();
            assert_eq!(
                s.failing_hits,
                failing.len(),
                "{name} did touch every failure"
            );
            assert!(
                s.score.abs() < 1e-9,
                "{name} touches everything and must score 0, got {:+.3}",
                s.score
            );
        }
        // And the innocent etcher must rank last — it is anti-correlated with failure.
        assert_eq!(ranked.last().unwrap().tool, "ETCH-A");
    }

    #[test]
    fn containment_finds_everything_the_bad_tool_touched() {
        // The second half: once ETCH-B is implicated, what else is at risk? This is the
        // query that sizes the recall, and it must not miss a lot.
        let (g, failing, _) = history();
        let exposed = g.lots_touched_by("ETCH-B", 0, u64::MAX);
        eprintln!("  ETCH-B exposed: {exposed:?}");
        assert_eq!(
            exposed, failing,
            "containment must return exactly the exposed lots"
        );

        // Windowed: only the first two lots, by time.
        let early = g.lots_touched_by("ETCH-B", 0, 2500);
        assert_eq!(
            early,
            vec!["LOT00", "LOT02"],
            "the window must bound the blast radius"
        );
    }

    #[test]
    fn a_lots_as_run_history_is_ordered_and_complete() {
        let (g, _, _) = history();
        let h = g.history("LOT00");
        assert_eq!(h.len(), 3, "every operation must be recorded");
        assert!(
            h.windows(2).all(|w| w[0].t_s <= w[1].t_s),
            "history must be time-ordered"
        );
        assert_eq!(
            g.tools_touching("LOT00"),
            vec!["LITHO-1", "ETCH-B", "CD-SEM"]
        );
    }

    #[test]
    fn commonality_with_no_passing_lots_still_ranks_but_cannot_discriminate() {
        // The honest degenerate case: with nothing to contrast against, *every* tool the
        // failures share scores 1.0 and the analysis has no power. Worth pinning, because
        // silently returning a confident answer here would be the worst behaviour —
        // "all the failures used LITHO-1" is true and useless.
        let (g, failing, _) = history();
        let ranked = g.commonality(&failing, &[]);
        let perfect: Vec<&str> = ranked
            .iter()
            .filter(|s| s.score >= 1.0)
            .map(|s| s.tool.as_str())
            .collect();
        assert!(
            perfect.len() > 1,
            "with no passing lots the analysis cannot single anything out, got {perfect:?}"
        );
    }
}
