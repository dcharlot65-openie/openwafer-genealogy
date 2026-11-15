//! JSON Lines interchange: one record per line, `{"table": ..., "record": {...}}`, header first.
//!
//! Lines can be concatenated, streamed and diffed, and a site can append records without
//! rewriting a file. Reading is permissive about content (a record with a missing unit still
//! parses, so the validator can say exactly what is missing) and strict about structure (an
//! unknown table or a line that is not JSON is an error with its line number).

use super::record::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
struct Line<T> {
    table: String,
    record: T,
}

fn line<T: Serialize>(table: &str, r: &T) -> String {
    serde_json::to_string(&Line {
        table: table.to_string(),
        record: r,
    })
    .expect("records serialize")
}

/// Write a dataset as JSON Lines.
pub fn to_jsonl(ds: &Dataset) -> String {
    let mut out = String::new();
    let mut push = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    push(line("header", &ds.header));
    macro_rules! table {
        ($f:ident) => {
            for r in &ds.$f {
                push(line(stringify!($f), r));
            }
        };
    }
    table!(units);
    table!(events);
    table!(recipes);
    table!(runs);
    table!(traces);
    table!(metrology);
    table!(electrical);
    table!(summaries);
    table!(yields);
    table!(defects);
    table!(bin_maps);
    table!(observations);
    table!(estimates);
    out
}

/// Read JSON Lines. The first non-empty line must be the header.
pub fn from_jsonl(text: &str) -> Result<Dataset, String> {
    let mut ds: Option<Dataset> = None;
    for (n, raw) in text.lines().enumerate() {
        let n = n + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let v: Line<Value> = serde_json::from_str(raw)
            .map_err(|e| format!("line {n}: not a JSON Lines record: {e}"))?;
        let rec = v.record;
        let bad = |e: serde_json::Error| format!("line {n} ({}): {e}", v.table);
        if v.table == "header" {
            if ds.is_some() {
                return Err(format!("line {n}: a second header"));
            }
            ds = Some(Dataset::new(serde_json::from_value(rec).map_err(bad)?));
            continue;
        }
        let d = ds
            .as_mut()
            .ok_or_else(|| format!("line {n}: a record before the header"))?;
        match v.table.as_str() {
            "units" => d.units.push(serde_json::from_value(rec).map_err(bad)?),
            "events" => d.events.push(serde_json::from_value(rec).map_err(bad)?),
            "recipes" => d.recipes.push(serde_json::from_value(rec).map_err(bad)?),
            "runs" => d.runs.push(serde_json::from_value(rec).map_err(bad)?),
            "traces" => d.traces.push(serde_json::from_value(rec).map_err(bad)?),
            "metrology" => d.metrology.push(serde_json::from_value(rec).map_err(bad)?),
            "electrical" => d.electrical.push(serde_json::from_value(rec).map_err(bad)?),
            "summaries" => d.summaries.push(serde_json::from_value(rec).map_err(bad)?),
            "yields" => d.yields.push(serde_json::from_value(rec).map_err(bad)?),
            "defects" => d.defects.push(serde_json::from_value(rec).map_err(bad)?),
            "bin_maps" => d.bin_maps.push(serde_json::from_value(rec).map_err(bad)?),
            "observations" => d
                .observations
                .push(serde_json::from_value(rec).map_err(bad)?),
            "estimates" => d.estimates.push(serde_json::from_value(rec).map_err(bad)?),
            other => return Err(format!("line {n}: unknown table '{other}'")),
        }
    }
    ds.ok_or_else(|| "no header line".to_string())
}
