//! The validator: every rule the schema states, checked deterministically.
//!
//! It never repairs a record and never guesses. It reports, per record, what is wrong and
//! where, and [`refuse`] drops the records that carry a record-level error. The rules:
//!
//! * **Provenance and licence on every record.** Missing either is an error.
//! * **Every link resolves**, parents have an allowed kind, and the hierarchy has no cycle.
//! * **Units.** Every quantity has a unit the unit table knows, whose dimension fits the
//!   declared kind. And one parameter has one unit across the dataset: a thickness in `nm`
//!   on one wafer and in `um` on the next is an error even though both are lengths, because
//!   a training column that mixes them is silently wrong by 1000×. Convert at import.
//! * **Uncertainty.** A measured value carries an uncertainty or an explicit `unknown` with a
//!   reason. A recipe parameter is a setpoint; a measurement is not.
//! * **Methods.** A derived value names its method. An extracted electrical parameter is a
//!   derived value, always.
//! * **Origins never mix silently.** A dataset declared MEASURED holds no SIMULATED record,
//!   and a MEASURED or SPECIFIED record never links to a SIMULATED one, whatever the
//!   dataset declares: real material cannot have been processed by a simulated run.
//!
//! Warnings flag what is legal but physically suspicious: values outside what physics allows
//! (VSWR below 1, negative absolute temperature) and relations that do not close
//! (deposited thickness against rate × time), with the unit factor the mismatch matches.

use super::record::*;
use super::units::{self, QuantityKind};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    SchemaMismatch,
    EmptyId,
    DuplicateId,
    MissingProvenance,
    MissingLicense,
    EmptyField,
    NotRedistributable,
    BrokenLink,
    BadParent,
    GenealogyCycle,
    MissingUnit,
    UnknownUnit,
    UnitDimension,
    UnitDeclMismatch,
    MissingUncertainty,
    SetpointAsMeasurement,
    NotASetpoint,
    BadUncertainty,
    NonFinite,
    MissingMethod,
    MissingSampleRate,
    TraceShape,
    MissingCalibration,
    BadTimestamp,
    TimeOrder,
    OriginMix,
    SimulatedInMeasuredJoin,
    YieldCount,
    SummaryCount,
    PhysicalRange,
    ScaleSuspect,
}

impl Code {
    pub fn severity(self) -> Severity {
        match self {
            Code::NotRedistributable | Code::PhysicalRange | Code::ScaleSuspect => {
                Severity::Warning
            }
            _ => Severity::Error,
        }
    }
    /// Errors that belong to one record: [`refuse`] drops the record. Link and origin errors
    /// are dataset errors and refuse the import as a whole.
    pub fn record_level(self) -> bool {
        matches!(
            self,
            Code::EmptyId
                | Code::MissingProvenance
                | Code::MissingLicense
                | Code::EmptyField
                | Code::MissingUnit
                | Code::UnknownUnit
                | Code::UnitDimension
                | Code::MissingUncertainty
                | Code::SetpointAsMeasurement
                | Code::NotASetpoint
                | Code::BadUncertainty
                | Code::NonFinite
                | Code::MissingMethod
                | Code::MissingSampleRate
                | Code::TraceShape
                | Code::MissingCalibration
                | Code::BadTimestamp
                | Code::TimeOrder
                | Code::YieldCount
                | Code::SummaryCount
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Issue {
    pub severity: Severity,
    pub code: Code,
    pub table: &'static str,
    pub id: String,
    /// Field path inside the record.
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error)
    }
    pub fn warnings(&self) -> impl Iterator<Item = &Issue> {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
    }
    pub fn ok(&self) -> bool {
        self.errors().next().is_none()
    }
    pub fn has(&self, code: Code) -> bool {
        self.issues.iter().any(|i| i.code == code)
    }
    pub fn count(&self, code: Code) -> usize {
        self.issues.iter().filter(|i| i.code == code).count()
    }
    /// Issue counts by code, for summaries.
    pub fn tally(&self) -> BTreeMap<Code, usize> {
        let mut m = BTreeMap::new();
        for i in &self.issues {
            *m.entry(i.code).or_default() += 1;
        }
        m
    }
}

struct Ctx<'a> {
    ds: &'a Dataset,
    out: Vec<Issue>,
    /// (unit-consistency key) → (first unit seen, normalized, where).
    decl: HashMap<String, (String, String)>,
    origin: HashMap<(&'static str, &'a str), Origin>,
}

impl<'a> Ctx<'a> {
    fn push(&mut self, code: Code, table: &'static str, id: &str, path: &str, message: String) {
        self.out.push(Issue {
            severity: code.severity(),
            code,
            table,
            id: id.to_string(),
            path: path.to_string(),
            message,
        });
    }
}

/// What a quantity is allowed to be.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// A recipe parameter or as-run setting: uncertainty must be `setpoint`.
    Setpoint,
    /// A measured value: uncertainty required and not `setpoint`.
    Measured,
    /// A derived value: like Measured, and a derivation is required.
    Derived,
    /// Either (nominal substrate properties, design coordinates, bias conditions).
    Any,
}

/// Licence ids whose terms allow redistribution with attribution (and share-alike where the
/// licence says so). Anything else, including `NOASSERTION`, is treated as not redistributable.
const REDISTRIBUTABLE: &[&str] = &[
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "CC-BY-3.0",
    "CC-BY-SA-3.0",
    "ODbL-1.0",
    "ODC-By-1.0",
    "PDDL-1.0",
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "MPL-2.0",
    "CERN-OHL-S-2.0",
    "CERN-OHL-W-2.0",
    "CERN-OHL-P-2.0",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
];

/// Can a record under this SPDX expression be redistributed? `A AND B` needs both, `A OR B`
/// needs either. Parenthesised expressions are not parsed and count as no.
pub fn redistributable(spdx: &str) -> bool {
    let s = spdx.trim();
    if s.is_empty() || s.contains('(') {
        return false;
    }
    s.split(" OR ").any(|alt| {
        alt.split(" AND ")
            .all(|id| REDISTRIBUTABLE.contains(&id.trim()))
    })
}

/// Seconds since the Unix epoch for an RFC 3339 time or a `YYYY-MM-DD` date.
pub fn parse_time(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let num = |a: usize, n: usize| -> Option<i64> {
        let t = s.get(a..a + n)?;
        if !t.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        t.parse().ok()
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    if b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
    {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    if s.len() == 10 {
        return Some(days as f64 * 86_400.0);
    }
    if !matches!(b.get(10), Some(b'T') | Some(b't') | Some(b' '))
        || b.get(13) != Some(&b':')
        || b.get(16) != Some(&b':')
    {
        return None;
    }
    let (h, mi, sec) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut frac = 0.0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        frac = format!("0.{}", &s[start..i]).parse().ok()?;
    }
    let off = match b.get(i) {
        Some(b'Z') | Some(b'z') if i + 1 == b.len() => 0,
        Some(c @ (b'+' | b'-')) if i + 6 == b.len() && b[i + 3] == b':' => {
            let oh = num(i + 1, 2)?;
            let om = num(i + 4, 2)?;
            let o = oh * 3600 + om * 60;
            if *c == b'+' {
                o
            } else {
                -o
            }
        }
        _ => return None,
    };
    Some(days as f64 * 86_400.0 + (h * 3600 + mi * 60 + sec - off) as f64 + frac)
}

fn origin_of(p: &Option<Provenance>) -> Option<Origin> {
    p.as_ref().map(|p| p.origin)
}

impl<'a> Ctx<'a> {
    fn common(
        &mut self,
        table: &'static str,
        id: &str,
        prov: &Option<Provenance>,
        lic: &Option<License>,
    ) {
        if id.trim().is_empty() {
            self.push(
                Code::EmptyId,
                table,
                id,
                "id",
                "every record needs a non-empty id".into(),
            );
        }
        match prov {
            None => self.push(
                Code::MissingProvenance,
                table,
                id,
                "provenance",
                "record has no provenance: refused".into(),
            ),
            Some(p) => {
                if p.source.trim().is_empty() {
                    self.push(
                        Code::EmptyField,
                        table,
                        id,
                        "provenance.source",
                        "provenance.source is empty".into(),
                    );
                }
                if p.produced_by.trim().is_empty() {
                    self.push(
                        Code::EmptyField,
                        table,
                        id,
                        "provenance.produced_by",
                        "provenance.produced_by is empty".into(),
                    );
                }
                if let Some(r) = &p.retrieved {
                    if parse_time(r).is_none() {
                        self.push(
                            Code::BadTimestamp,
                            table,
                            id,
                            "provenance.retrieved",
                            format!("'{r}' is not a date"),
                        );
                    }
                }
                let declared = self.ds.header.declared_origin;
                let bad = matches!(
                    (declared, p.origin),
                    (DeclaredOrigin::Measured, Origin::Simulated)
                        | (DeclaredOrigin::Simulated, Origin::Measured)
                );
                if bad {
                    self.push(
                        Code::OriginMix,
                        table,
                        id,
                        "provenance.origin",
                        format!(
                            "a {:?} record in a dataset declared {:?}",
                            p.origin, declared
                        ),
                    );
                }
            }
        }
        match lic {
            None => self.push(Code::MissingLicense, table, id, "license", "record has no licence: refused".into()),
            Some(l) if l.spdx.trim().is_empty() => {
                self.push(Code::MissingLicense, table, id, "license.spdx", "licence has an empty SPDX expression: refused".into())
            }
            Some(l) if !redistributable(&l.spdx) => self.push(
                Code::NotRedistributable,
                table,
                id,
                "license.spdx",
                format!("'{}' does not allow redistribution as far as this tool knows; export --redistributable-only drops it", l.spdx),
            ),
            _ => {}
        }
    }

    /// One parameter, one unit, across the dataset.
    fn declare(&mut self, key: String, unit: &str, table: &'static str, id: &str, path: &str) {
        let n = units::normalize(unit);
        match self.decl.get(&key) {
            None => {
                self.decl.insert(key, (n, format!("{table}/{id}")));
            }
            Some((first, at)) if *first != n => {
                let msg = format!(
                    "'{key}' is in {first} elsewhere in this dataset (first at {at}); this record uses {n}. One column, one unit: convert at import"
                );
                self.push(Code::UnitDeclMismatch, table, id, path, msg);
            }
            _ => {}
        }
    }

    fn unit(
        &mut self,
        unit: &Option<String>,
        kind: Option<QuantityKind>,
        table: &'static str,
        id: &str,
        path: &str,
    ) -> Option<String> {
        let Some(u) = unit else {
            self.push(
                Code::MissingUnit,
                table,
                id,
                path,
                "quantity has no unit".into(),
            );
            return None;
        };
        match units::parse(u) {
            None => {
                self.push(
                    Code::UnknownUnit,
                    table,
                    id,
                    path,
                    format!("unit '{u}' is not in the unit table"),
                );
                None
            }
            Some(p) => {
                if let Some(k) = kind {
                    if p.dim != k.dim() {
                        self.push(
                            Code::UnitDimension,
                            table,
                            id,
                            path,
                            format!("unit '{u}' does not have the dimension of {k:?}"),
                        );
                        return None;
                    }
                }
                Some(u.clone())
            }
        }
    }

    fn uncertainty(
        &mut self,
        u: &Option<Uncertainty>,
        role: Role,
        table: &'static str,
        id: &str,
        path: &str,
    ) {
        match (u, role) {
            (None, _) => self.push(
                Code::MissingUncertainty,
                table,
                id,
                path,
                "no uncertainty: give one, or state `unknown` with the reason".into(),
            ),
            (Some(Uncertainty::Setpoint { .. }), Role::Measured | Role::Derived) => self.push(
                Code::SetpointAsMeasurement,
                table,
                id,
                path,
                "a measured or derived value is marked as a setpoint".into(),
            ),
            (
                Some(
                    Uncertainty::Standard { .. }
                    | Uncertainty::Expanded { .. }
                    | Uncertainty::Unknown { .. },
                ),
                Role::Setpoint,
            ) => self.push(
                Code::NotASetpoint,
                table,
                id,
                path,
                "a recipe parameter or setting must be a setpoint".into(),
            ),
            (Some(Uncertainty::Standard { u, .. }), _) if !(u.is_finite() && *u >= 0.0) => self
                .push(
                    Code::BadUncertainty,
                    table,
                    id,
                    path,
                    format!("standard uncertainty {u} is not a finite non-negative number"),
                ),
            (Some(Uncertainty::Expanded { u, k, .. }), _)
                if !(u.is_finite() && *u >= 0.0 && k.is_finite() && *k > 0.0) =>
            {
                self.push(
                    Code::BadUncertainty,
                    table,
                    id,
                    path,
                    format!("expanded uncertainty U = {u}, k = {k} is invalid"),
                )
            }
            (Some(Uncertainty::Unknown { reason }), _) if reason.trim().is_empty() => self.push(
                Code::BadUncertainty,
                table,
                id,
                path,
                "an unknown uncertainty must say why".into(),
            ),
            (Some(Uncertainty::Setpoint { tolerance: Some(t) }), _)
                if !(t.is_finite() && *t >= 0.0) =>
            {
                self.push(
                    Code::BadUncertainty,
                    table,
                    id,
                    path,
                    format!("setpoint tolerance {t} is invalid"),
                )
            }
            _ => {}
        }
    }

    fn quantity(
        &mut self,
        q: &Quantity,
        kind: Option<QuantityKind>,
        role: Role,
        decl_key: Option<String>,
        table: &'static str,
        id: &str,
        path: &str,
    ) {
        if !q.value.is_finite() {
            self.push(
                Code::NonFinite,
                table,
                id,
                &format!("{path}.value"),
                format!("value {} is not finite", q.value),
            );
        }
        if let Some(u) = self.unit(&q.unit, kind, table, id, &format!("{path}.unit")) {
            if let Some(k) = decl_key {
                self.declare(k, &u, table, id, &format!("{path}.unit"));
            }
            self.range(q.value, &u, kind, path, table, id);
        }
        self.uncertainty(
            &q.uncertainty,
            role,
            table,
            id,
            &format!("{path}.uncertainty"),
        );
        match (&q.derivation, role) {
            (None, Role::Derived) => self.push(
                Code::MissingMethod,
                table,
                id,
                &format!("{path}.derivation"),
                "a derived value must name its method".into(),
            ),
            (Some(d), _) if d.method.trim().is_empty() => self.push(
                Code::MissingMethod,
                table,
                id,
                &format!("{path}.derivation.method"),
                "derivation method is empty".into(),
            ),
            _ => {}
        }
    }

    fn param(
        &mut self,
        p: &Parameter,
        role: Role,
        scope: &str,
        table: &'static str,
        id: &str,
        path: &str,
    ) {
        let key = format!("{scope}:{}", p.name);
        self.quantity(
            &p.quantity,
            p.kind,
            role,
            Some(key),
            table,
            id,
            &format!("{path}[{}]", p.name),
        );
    }

    /// Values physics forbids. Warnings: the data is kept, flagged.
    fn range(
        &mut self,
        v: f64,
        unit: &str,
        kind: Option<QuantityKind>,
        path: &str,
        table: &'static str,
        id: &str,
    ) {
        let Some(k) = kind else { return };
        let Some(si) = units::parse(unit).map(|u| u.to_si(v)) else {
            return;
        };
        let bad = match k {
            QuantityKind::Temperature => si < 0.0,
            QuantityKind::Pressure
            | QuantityKind::Resistance
            | QuantityKind::SheetResistance
            | QuantityKind::Resistivity
            | QuantityKind::GasFlow
            | QuantityKind::Power
            | QuantityKind::Time
            | QuantityKind::ArealDensity => si < 0.0,
            _ => false,
        };
        if bad {
            self.push(
                Code::PhysicalRange,
                table,
                id,
                path,
                format!("{v} {unit} is impossible for a {k:?}"),
            );
        }
    }

    fn time(
        &mut self,
        t: &Option<String>,
        table: &'static str,
        id: &str,
        path: &str,
    ) -> Option<f64> {
        let s = t.as_ref()?;
        let v = parse_time(s);
        if v.is_none() {
            self.push(
                Code::BadTimestamp,
                table,
                id,
                path,
                format!("'{s}' is not RFC 3339 (or YYYY-MM-DD)"),
            );
        }
        v
    }

    fn link(
        &mut self,
        from_table: &'static str,
        from_id: &str,
        from_origin: Option<Origin>,
        to_table: &'static str,
        to: &str,
        path: &str,
    ) {
        match self.origin.get(&(to_table, to)).copied() {
            None => self.push(
                Code::BrokenLink,
                from_table,
                from_id,
                path,
                format!("'{to}' is not a record in {to_table}"),
            ),
            Some(target) => {
                if target == Origin::Simulated
                    && matches!(from_origin, Some(Origin::Measured | Origin::Specified))
                {
                    self.push(
                        Code::SimulatedInMeasuredJoin,
                        from_table,
                        from_id,
                        path,
                        format!(
                            "a {:?} record links to SIMULATED {to_table}/{to}",
                            from_origin.unwrap()
                        ),
                    );
                }
            }
        }
    }
}

/// Validate a whole dataset.
pub fn validate(ds: &Dataset) -> Report {
    let mut c = Ctx {
        ds,
        out: Vec::new(),
        decl: HashMap::new(),
        origin: HashMap::new(),
    };

    // Header.
    if ds.header.schema != SCHEMA_NAME {
        c.push(
            Code::SchemaMismatch,
            "header",
            &ds.header.dataset_id,
            "schema",
            format!("schema '{}' is not {SCHEMA_NAME}", ds.header.schema),
        );
    }
    let major = |v: &str| v.split('.').next().map(str::to_string);
    if major(&ds.header.schema_version) != major(SCHEMA_VERSION) {
        c.push(
            Code::SchemaMismatch,
            "header",
            &ds.header.dataset_id,
            "schema_version",
            format!(
                "schema version {} is not compatible with {SCHEMA_VERSION}",
                ds.header.schema_version
            ),
        );
    }

    // Ids, duplicates, and the origin index every link check uses.
    macro_rules! index {
        ($field:ident, $name:literal) => {
            let mut seen = BTreeSet::new();
            for r in &ds.$field {
                if !seen.insert(r.id.as_str()) {
                    c.push(
                        Code::DuplicateId,
                        $name,
                        &r.id,
                        "id",
                        format!("id '{}' appears twice in {}", r.id, $name),
                    );
                }
                if let Some(o) = origin_of(&r.provenance) {
                    c.origin.insert(($name, r.id.as_str()), o);
                } else {
                    // Still a valid link target; its own missing provenance is reported separately.
                    c.origin
                        .entry(($name, r.id.as_str()))
                        .or_insert(Origin::Specified);
                }
            }
        };
    }
    index!(units, "units");
    index!(events, "events");
    index!(recipes, "recipes");
    index!(runs, "runs");
    index!(traces, "traces");
    index!(metrology, "metrology");
    index!(electrical, "electrical");
    index!(summaries, "summaries");
    index!(yields, "yields");
    index!(defects, "defects");
    index!(bin_maps, "bin_maps");
    index!(observations, "observations");
    index!(estimates, "estimates");

    // ── units ──
    let by_id: HashMap<&str, &MaterialUnit> = ds.units.iter().map(|u| (u.id.as_str(), u)).collect();
    for u in &ds.units {
        c.common("units", &u.id, &u.provenance, &u.license);
        let o = origin_of(&u.provenance);
        if let Some(p) = &u.parent {
            c.link("units", &u.id, o, "units", p, "parent");
            if let Some(pu) = by_id.get(p.as_str()) {
                if !u.kind.allows_parent(pu.kind) {
                    c.push(
                        Code::BadParent,
                        "units",
                        &u.id,
                        "parent",
                        format!("a {:?} cannot sit inside a {:?}", u.kind, pu.kind),
                    );
                }
            }
        }
        if let Some(s) = &u.substrate {
            for p in &s.properties {
                c.param(
                    p,
                    Role::Any,
                    "substrate",
                    "units",
                    &u.id,
                    "substrate.properties",
                );
            }
        }
        if let Some(pos) = &u.position {
            position(&mut c, pos, "units", &u.id, "position");
        }
    }
    // Cycles in the parent chain.
    for u in &ds.units {
        let mut seen = BTreeSet::new();
        let mut cur = Some(u.id.as_str());
        while let Some(id) = cur {
            if !seen.insert(id) {
                c.push(
                    Code::GenealogyCycle,
                    "units",
                    &u.id,
                    "parent",
                    format!("the parent chain of '{}' returns to '{id}'", u.id),
                );
                break;
            }
            cur = by_id.get(id).and_then(|x| x.parent.as_deref());
        }
    }

    // ── events ──
    for e in &ds.events {
        c.common("events", &e.id, &e.provenance, &e.license);
        let o = origin_of(&e.provenance);
        for (i, x) in e.inputs.iter().enumerate() {
            c.link("events", &e.id, o, "units", x, &format!("inputs[{i}]"));
        }
        for (i, x) in e.outputs.iter().enumerate() {
            c.link("events", &e.id, o, "units", x, &format!("outputs[{i}]"));
        }
        if let Some(r) = &e.run {
            c.link("events", &e.id, o, "runs", r, "run");
        }
        c.time(&e.at, "events", &e.id, "at");
        let shape_ok = match e.op {
            GenealogyOp::Create => e.inputs.is_empty() && !e.outputs.is_empty(),
            GenealogyOp::Split => e.inputs.len() == 1 && e.outputs.len() >= 2,
            GenealogyOp::Merge => e.inputs.len() >= 2 && e.outputs.len() == 1,
            GenealogyOp::Rework => !e.inputs.is_empty() && e.run.is_some(),
            GenealogyOp::Scrap => !e.inputs.is_empty() && e.outputs.is_empty(),
        };
        if !shape_ok {
            c.push(
                Code::BrokenLink,
                "events",
                &e.id,
                "op",
                format!(
                    "a {:?} event has the wrong inputs/outputs (or no run for a rework)",
                    e.op
                ),
            );
        }
    }

    // ── recipes ──
    for r in &ds.recipes {
        c.common("recipes", &r.id, &r.provenance, &r.license);
        let o = origin_of(&r.provenance);
        if let Some(s) = &r.supersedes {
            c.link("recipes", &r.id, o, "recipes", s, "supersedes");
        }
        for p in &r.parameters {
            c.param(
                p,
                Role::Setpoint,
                &format!("recipe:{}", r.name),
                "recipes",
                &r.id,
                "parameters",
            );
        }
        closure(&mut c, &r.parameters, "recipes", &r.id, "parameters");
    }

    // ── runs ──
    let runs_by_id: HashMap<&str, &ProcessRun> =
        ds.runs.iter().map(|r| (r.id.as_str(), r)).collect();
    for r in &ds.runs {
        c.common("runs", &r.id, &r.provenance, &r.license);
        let o = origin_of(&r.provenance);
        if let Some(rc) = &r.recipe {
            c.link("runs", &r.id, o, "recipes", rc, "recipe");
        }
        for (i, m) in r.material.iter().enumerate() {
            c.link("runs", &r.id, o, "units", m, &format!("material[{i}]"));
        }
        let t0 = c.time(&r.start, "runs", &r.id, "start");
        let t1 = c.time(&r.end, "runs", &r.id, "end");
        if let (Some(a), Some(b)) = (t0, t1) {
            if b < a {
                c.push(
                    Code::TimeOrder,
                    "runs",
                    &r.id,
                    "end",
                    "run ends before it starts".into(),
                );
            }
        }
        let scope = r.step_name.clone().unwrap_or_else(|| r.tool.clone());
        for p in &r.settings {
            c.param(
                p,
                Role::Setpoint,
                &format!("settings:{scope}"),
                "runs",
                &r.id,
                "settings",
            );
        }
        for p in &r.actuals {
            c.param(
                p,
                Role::Measured,
                &format!("actuals:{scope}"),
                "runs",
                &r.id,
                "actuals",
            );
        }
        closure(&mut c, &r.settings, "runs", &r.id, "settings");
    }

    // ── traces ──
    for t in &ds.traces {
        c.common("traces", &t.id, &t.provenance, &t.license);
        let o = origin_of(&t.provenance);
        c.link("traces", &t.id, o, "runs", &t.run, "run");
        if let Some(u) = c.unit(&t.unit, t.kind, "traces", &t.id, "unit") {
            c.declare(
                format!("trace:{}:{}", t.tool, t.sensor),
                &u,
                "traces",
                &t.id,
                "unit",
            );
        }
        c.uncertainty(&t.uncertainty, Role::Any, "traces", &t.id, "uncertainty");
        match (t.sample_rate_hz, t.times_s.is_empty()) {
            (None, true) => c.push(
                Code::MissingSampleRate,
                "traces",
                &t.id,
                "sample_rate_hz",
                "give sample_rate_hz or times_s".into(),
            ),
            (Some(_), false) => c.push(
                Code::MissingSampleRate,
                "traces",
                &t.id,
                "sample_rate_hz",
                "give sample_rate_hz or times_s, not both".into(),
            ),
            (Some(r), true) if !(r.is_finite() && r > 0.0) => c.push(
                Code::MissingSampleRate,
                "traces",
                &t.id,
                "sample_rate_hz",
                format!("sample rate {r} Hz is not positive"),
            ),
            _ => {}
        }
        if t.values.is_empty() {
            c.push(
                Code::TraceShape,
                "traces",
                &t.id,
                "values",
                "a trace with no samples".into(),
            );
        }
        if !t.times_s.is_empty() {
            if t.times_s.len() != t.values.len() {
                c.push(
                    Code::TraceShape,
                    "traces",
                    &t.id,
                    "times_s",
                    format!("{} times for {} values", t.times_s.len(), t.values.len()),
                );
            }
            if t.times_s.iter().any(|x| !x.is_finite()) || t.times_s.windows(2).any(|w| w[1] < w[0])
            {
                c.push(
                    Code::TraceShape,
                    "traces",
                    &t.id,
                    "times_s",
                    "sample times are not finite and non-decreasing".into(),
                );
            }
        }
        if t.values.iter().flatten().any(|v| !v.is_finite()) {
            c.push(
                Code::NonFinite,
                "traces",
                &t.id,
                "values",
                "a sample is not finite (write a missing sample as null)".into(),
            );
        }
        let n = t.values.len() as u64;
        if t.flags.iter().any(|f| f.index >= n) {
            c.push(
                Code::TraceShape,
                "traces",
                &t.id,
                "flags",
                "a flag points past the last sample".into(),
            );
        }
        match &t.calibration {
            None => c.push(
                Code::MissingCalibration,
                "traces",
                &t.id,
                "calibration",
                "the sensor's calibration is not stated (use status `unknown` if it is not known)"
                    .into(),
            ),
            Some(cal) => {
                if cal.status == CalStatus::Calibrated && cal.date.is_none() {
                    c.push(
                        Code::MissingCalibration,
                        "traces",
                        &t.id,
                        "calibration.date",
                        "calibrated, but no calibration date".into(),
                    );
                }
                c.time(&cal.date, "traces", &t.id, "calibration.date");
            }
        }
        c.time(&t.t0, "traces", &t.id, "t0");
        // VSWR cannot be below 1: (V_f + V_r)/(V_f − V_r) ≥ 1 for V_f > V_r ≥ 0.
        if t.sensor.to_lowercase().contains("vswr") {
            let flagged: BTreeSet<u64> = t.flags.iter().map(|f| f.index).collect();
            let bad = t
                .values
                .iter()
                .enumerate()
                .filter(|(i, v)| {
                    matches!(v, Some(x) if *x < 1.0) && !flagged.contains(&(*i as u64))
                })
                .count();
            if bad > 0 {
                c.push(
                    Code::PhysicalRange,
                    "traces",
                    &t.id,
                    "values",
                    format!("{bad} unflagged VSWR samples below 1, which physics forbids"),
                );
            }
        }
    }

    // ── metrology ──
    for m in &ds.metrology {
        c.common("metrology", &m.id, &m.provenance, &m.license);
        let o = origin_of(&m.provenance);
        c.link("metrology", &m.id, o, "units", &m.material, "material");
        if let Some(r) = &m.after_run {
            c.link("metrology", &m.id, o, "runs", r, "after_run");
        }
        let derived = m.points.iter().any(|p| p.value.derivation.is_some());
        let role = if derived {
            Role::Derived
        } else {
            Role::Measured
        };
        for (i, p) in m.points.iter().enumerate() {
            c.quantity(
                &p.value,
                m.kind,
                role,
                Some(format!("metrology:{}", m.parameter)),
                "metrology",
                &m.id,
                &format!("points[{i}].value"),
            );
            if let Some(pos) = &p.position {
                position(
                    &mut c,
                    pos,
                    "metrology",
                    &m.id,
                    &format!("points[{i}].position"),
                );
            }
        }
        if m.points.is_empty() {
            c.push(
                Code::TraceShape,
                "metrology",
                &m.id,
                "points",
                "a metrology record with no values".into(),
            );
        }
        for p in &m.conditions {
            c.param(
                p,
                Role::Any,
                &format!("conditions:{}", m.method),
                "metrology",
                &m.id,
                "conditions",
            );
        }
        c.time(&m.at, "metrology", &m.id, "at");
        // A measured value 10× or 1000× its recipe target is a unit label error until shown otherwise.
        if let (Some(run), Some(pt), Some(kind)) = (
            m.after_run
                .as_ref()
                .and_then(|r| runs_by_id.get(r.as_str())),
            m.points.first(),
            m.kind,
        ) {
            let recipe = run
                .recipe
                .as_ref()
                .and_then(|rid| ds.recipes.iter().find(|x| &x.id == rid));
            let target = run
                .settings
                .iter()
                .chain(recipe.map(|r| r.parameters.as_slice()).unwrap_or(&[]))
                .find(|p| p.kind == Some(kind) && same_param(&p.name, &m.parameter));
            let si = |q: &Quantity| {
                q.unit
                    .as_deref()
                    .and_then(units::parse)
                    .map(|u| u.to_si(q.value))
            };
            if let Some((a, b)) = target.and_then(|t| Some((si(&pt.value)?, si(&t.quantity)?))) {
                scale_check(
                    &mut c,
                    a,
                    b,
                    "metrology",
                    &m.id,
                    "points[0].value",
                    "measured value and its recipe target",
                );
            }
        }
    }

    // ── electrical ──
    for e in &ds.electrical {
        c.common("electrical", &e.id, &e.provenance, &e.license);
        let o = origin_of(&e.provenance);
        c.link("electrical", &e.id, o, "units", &e.material, "material");
        for p in &e.structure_params {
            c.param(
                p,
                Role::Any,
                &format!("structure:{}", e.structure),
                "electrical",
                &e.id,
                "structure_params",
            );
        }
        for (i, b) in e.bias.iter().enumerate() {
            let path = format!("bias[{i}]");
            match &b.mode {
                BiasMode::Fixed { value } => {
                    c.quantity(value, None, Role::Any, None, "electrical", &e.id, &path)
                }
                BiasMode::Sweep { start, stop, step } => {
                    c.quantity(
                        start,
                        None,
                        Role::Any,
                        None,
                        "electrical",
                        &e.id,
                        &format!("{path}.start"),
                    );
                    c.quantity(
                        stop,
                        None,
                        Role::Any,
                        None,
                        "electrical",
                        &e.id,
                        &format!("{path}.stop"),
                    );
                    if let Some(s) = step {
                        c.quantity(
                            s,
                            None,
                            Role::Any,
                            None,
                            "electrical",
                            &e.id,
                            &format!("{path}.step"),
                        );
                    }
                }
            }
        }
        for (i, cv) in e.curves.iter().enumerate() {
            for (ax, name) in [(&cv.x, "x"), (&cv.y, "y")] {
                let path = format!("curves[{i}].{name}");
                if let Some(u) = c.unit(
                    &ax.unit,
                    ax.kind,
                    "electrical",
                    &e.id,
                    &format!("{path}.unit"),
                ) {
                    c.declare(
                        format!("curve:{}:{}", e.structure, ax.name),
                        &u,
                        "electrical",
                        &e.id,
                        &format!("{path}.unit"),
                    );
                }
                if let Some(d) = &ax.derivation {
                    if d.method.trim().is_empty() {
                        c.push(
                            Code::MissingMethod,
                            "electrical",
                            &e.id,
                            &format!("{path}.derivation"),
                            "derivation method is empty".into(),
                        );
                    }
                }
                if ax.values.iter().flatten().any(|v| !v.is_finite()) {
                    c.push(
                        Code::NonFinite,
                        "electrical",
                        &e.id,
                        &format!("{path}.values"),
                        "a curve value is not finite".into(),
                    );
                }
            }
            if cv.x.values.len() != cv.y.values.len() {
                c.push(
                    Code::TraceShape,
                    "electrical",
                    &e.id,
                    &format!("curves[{i}]"),
                    "x and y have different lengths".into(),
                );
            }
            c.uncertainty(
                &cv.uncertainty,
                Role::Measured,
                "electrical",
                &e.id,
                &format!("curves[{i}].uncertainty"),
            );
            for p in &cv.fixed {
                c.param(
                    p,
                    Role::Any,
                    &format!("curve-fixed:{}", e.structure),
                    "electrical",
                    &e.id,
                    &format!("curves[{i}].fixed"),
                );
            }
        }
        for x in &e.extracted {
            c.quantity(
                &x.quantity,
                x.kind,
                Role::Derived,
                Some(format!("extracted:{}", x.name)),
                "electrical",
                &e.id,
                &format!("extracted[{}]", x.name),
            );
        }
        c.time(&e.at, "electrical", &e.id, "at");
    }

    // ── summaries ──
    for s in &ds.summaries {
        c.common("summaries", &s.id, &s.provenance, &s.license);
        let o = origin_of(&s.provenance);
        c.link("summaries", &s.id, o, "units", &s.scope, "scope");
        for (i, m) in s.members.iter().enumerate() {
            c.link(
                "summaries",
                &s.id,
                o,
                "electrical",
                m,
                &format!("members[{i}]"),
            );
        }
        if let Some(u) = c.unit(&s.unit, s.kind, "summaries", &s.id, "unit") {
            c.declare(
                format!("summary:{}", s.parameter),
                &u,
                "summaries",
                &s.id,
                "unit",
            );
        }
        if s.method.as_deref().map(str::trim).unwrap_or("").is_empty() {
            c.push(
                Code::MissingMethod,
                "summaries",
                &s.id,
                "method",
                "a summary must name how its member values were obtained".into(),
            );
        }
        if s.n == 0 || (!s.members.is_empty() && s.members.len() as u64 != s.n) {
            c.push(
                Code::SummaryCount,
                "summaries",
                &s.id,
                "n",
                format!("n = {} with {} members listed", s.n, s.members.len()),
            );
        }
        for (v, f) in [
            (Some(s.mean), "mean"),
            (s.sd, "sd"),
            (s.min, "min"),
            (s.median, "median"),
            (s.max, "max"),
        ] {
            if let Some(v) = v {
                if !v.is_finite() {
                    c.push(
                        Code::NonFinite,
                        "summaries",
                        &s.id,
                        f,
                        format!("{f} = {v} is not finite"),
                    );
                }
            }
        }
        if s.sd.is_some_and(|x| x < 0.0) {
            c.push(
                Code::BadUncertainty,
                "summaries",
                &s.id,
                "sd",
                "a standard deviation cannot be negative".into(),
            );
        }
    }

    // ── yields ──
    for y in &ds.yields {
        c.common("yields", &y.id, &y.provenance, &y.license);
        c.link(
            "yields",
            &y.id,
            origin_of(&y.provenance),
            "units",
            &y.scope,
            "scope",
        );
        if y.passed > y.tested || y.tested == 0 {
            c.push(
                Code::YieldCount,
                "yields",
                &y.id,
                "passed",
                format!("{} passed of {} tested", y.passed, y.tested),
            );
        }
    }

    // ── defects, bin maps, observations, estimates ──
    for d in &ds.defects {
        c.common("defects", &d.id, &d.provenance, &d.license);
        let o = origin_of(&d.provenance);
        c.link("defects", &d.id, o, "units", &d.material, "material");
        if let Some(r) = &d.after_run {
            c.link("defects", &d.id, o, "runs", r, "after_run");
        }
        if let Some(s) = &d.size {
            c.quantity(
                s,
                Some(QuantityKind::Length),
                Role::Measured,
                Some("defect:size".into()),
                "defects",
                &d.id,
                "size",
            );
        }
        if let Some(pos) = &d.position {
            position(&mut c, pos, "defects", &d.id, "position");
        }
    }
    for b in &ds.bin_maps {
        c.common("bin_maps", &b.id, &b.provenance, &b.license);
        let o = origin_of(&b.provenance);
        c.link("bin_maps", &b.id, o, "units", &b.material, "material");
        if let Some(r) = &b.after_run {
            c.link("bin_maps", &b.id, o, "runs", r, "after_run");
        }
        let codes: BTreeSet<u32> = b.bin_defs.iter().map(|d| d.code).collect();
        for (i, d) in b.dies.iter().enumerate() {
            if !codes.contains(&d.bin) {
                c.push(
                    Code::BrokenLink,
                    "bin_maps",
                    &b.id,
                    &format!("dies[{i}].bin"),
                    format!("bin {} is not defined", d.bin),
                );
            }
            if let Some(u) = &d.die {
                c.link("bin_maps", &b.id, o, "units", u, &format!("dies[{i}].die"));
            }
        }
    }
    for ob in &ds.observations {
        c.common("observations", &ob.id, &ob.provenance, &ob.license);
        let o = origin_of(&ob.provenance);
        c.link("observations", &ob.id, o, "units", &ob.material, "material");
        if let Some(r) = &ob.after_run {
            c.link("observations", &ob.id, o, "runs", r, "after_run");
        }
    }
    for e in &ds.estimates {
        c.common("estimates", &e.id, &e.provenance, &e.license);
        let o = origin_of(&e.provenance);
        for (i, s) in e.scope.iter().enumerate() {
            c.link("estimates", &e.id, o, "units", s, &format!("scope[{i}]"));
        }
        c.quantity(
            &e.quantity,
            e.kind,
            Role::Derived,
            Some(format!("estimate:{}", e.parameter)),
            "estimates",
            &e.id,
            "quantity",
        );
    }

    Report { issues: c.out }
}

fn position(c: &mut Ctx, p: &MapPosition, table: &'static str, id: &str, path: &str) {
    for (q, n) in [(&p.x, "x"), (&p.y, "y")] {
        if let Some(q) = q {
            c.quantity(
                q,
                Some(QuantityKind::Length),
                Role::Any,
                None,
                table,
                id,
                &format!("{path}.{n}"),
            );
        }
    }
}

fn same_param(a: &str, b: &str) -> bool {
    let k = |s: &str| s.to_lowercase().replace(['_', '-', ' '], "");
    let (a, b) = (k(a), k(b));
    a == b || (a.contains("thickness") && b.contains("thickness"))
}

/// Unit factors a mislabelled value tends to be off by.
const UNIT_FACTORS: &[(f64, &str)] = &[
    (6.0, "Å/s ↔ nm/min"),
    (10.0, "Å ↔ nm"),
    (60.0, "s ↔ min (or nm/s ↔ nm/min)"),
    (100.0, "% ↔ fraction, or hPa ↔ Pa"),
    (1000.0, "nm ↔ µm (or µm ↔ mm)"),
    (3600.0, "s ↔ h"),
    (133.322, "Torr ↔ Pa"),
    (0.750_062, "hPa ↔ Torr"),
];

fn scale_check(c: &mut Ctx, a: f64, b: f64, table: &'static str, id: &str, path: &str, what: &str) {
    if !(a.is_finite() && b.is_finite()) || a == 0.0 || b == 0.0 {
        return;
    }
    let r = (a / b).abs();
    if (0.75..=1.333).contains(&r) {
        return;
    }
    let ratio = if r >= 1.0 { r } else { 1.0 / r };
    let hint = UNIT_FACTORS
        .iter()
        .find(|(f, _)| (ratio / f - 1.0).abs() < 0.03 || (ratio * f - 1.0).abs() < 0.03)
        .map(|(f, n)| {
            format!("; the ratio matches the {n} factor ({f}), so a unit label is probably wrong")
        })
        .unwrap_or_default();
    c.push(
        Code::ScaleSuspect,
        table,
        id,
        path,
        format!("{what} differ by {ratio:.3}×{hint}"),
    );
}

/// Deposited thickness against rate × time, when a parameter list holds all three.
fn closure(c: &mut Ctx, ps: &[Parameter], table: &'static str, id: &str, path: &str) {
    let find = |k: QuantityKind, word: &str| {
        let hits: Vec<&Parameter> = ps
            .iter()
            .filter(|p| p.kind == Some(k) && p.name.to_lowercase().contains(word))
            .collect();
        let best = hits
            .iter()
            .find(|p| p.name.to_lowercase().contains("depos"))
            .or(hits.first())
            .copied();
        best.and_then(|p| {
            let u = p.quantity.unit.as_deref()?;
            let si = units::parse(u)?.to_si(p.quantity.value);
            Some(si)
        })
    };
    if let (Some(t), Some(r), Some(d)) = (
        find(QuantityKind::Length, "thickness"),
        find(QuantityKind::Rate, "rate"),
        find(QuantityKind::Time, "time"),
    ) {
        scale_check(
            c,
            r * d,
            t,
            table,
            id,
            path,
            "deposition rate × time and stated thickness",
        );
    }
}

/// Drop every record with a record-level error. Returns the kept dataset and the refused ids.
/// Link and origin errors are not dropped here: they mean the dataset as a whole is wrong.
pub fn refuse(ds: &Dataset, report: &Report) -> (Dataset, Vec<(String, String)>) {
    let bad: BTreeSet<(&str, &str)> = report
        .errors()
        .filter(|i| i.code.record_level())
        .map(|i| (i.table, i.id.as_str()))
        .collect();
    let mut out = ds.clone();
    let mut refused = Vec::new();
    macro_rules! drop_bad {
        ($field:ident, $name:literal) => {
            out.$field.retain(|r| {
                let keep = !bad.contains(&($name, r.id.as_str()));
                if !keep {
                    refused.push(($name.to_string(), r.id.clone()));
                }
                keep
            });
        };
    }
    drop_bad!(units, "units");
    drop_bad!(events, "events");
    drop_bad!(recipes, "recipes");
    drop_bad!(runs, "runs");
    drop_bad!(traces, "traces");
    drop_bad!(metrology, "metrology");
    drop_bad!(electrical, "electrical");
    drop_bad!(summaries, "summaries");
    drop_bad!(yields, "yields");
    drop_bad!(defects, "defects");
    drop_bad!(bin_maps, "bin_maps");
    drop_bad!(observations, "observations");
    drop_bad!(estimates, "estimates");
    (out, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_and_dates() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_time("1970-01-02"), Some(86_400.0));
        assert_eq!(
            parse_time("2026-04-03T12:51:51.109-04:00").map(|t| (t * 1000.0).round() / 1000.0),
            Some(1_775_235_111.109)
        );
        assert!(parse_time("2026-13-01").is_none());
        assert!(parse_time("2026-04-03 12:51").is_none());
        assert!(parse_time("yesterday").is_none());
    }

    #[test]
    fn spdx_redistribution() {
        assert!(redistributable("CC-BY-4.0"));
        assert!(redistributable("CC-BY-SA-4.0 AND CERN-OHL-S-2.0"));
        assert!(redistributable("NOASSERTION OR MIT"));
        assert!(!redistributable("NOASSERTION"));
        assert!(!redistributable("CC-BY-4.0 AND NOASSERTION"));
        assert!(!redistributable(""));
    }
}
