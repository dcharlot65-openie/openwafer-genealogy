//! The records of the openWafer genealogy schema (`owg`), version [`SCHEMA_VERSION`].
//!
//! One [`Dataset`] holds a header and thirteen tables. Every record in every table carries a
//! [`Provenance`] and a [`License`]; a record without either is refused by
//! [`super::validate`]. Every number is a [`Quantity`] with a unit, and every measured number
//! has an uncertainty or an explicit [`Uncertainty::Unknown`] with the reason.
//!
//! The joins the schema exists for:
//!
//! ```text
//!   Recipe ◀─recipe── ProcessRun ──material──▶ MaterialUnit ◀─parent── MaterialUnit (lot ▸ wafer ▸ chip ▸ die ▸ site)
//!                        ▲   ▲                       ▲
//!                    run │   │ after_run             │ material
//!                      Trace  Metrology, Observation, Defect, ElectricalTest, BinMap, Yield, Summary
//! ```
//!
//! Fields that the validator must be able to report as *missing* (a unit, an uncertainty, a
//! provenance, a licence) are `Option` in the Rust types so a broken record still parses and
//! gets a precise diagnostic, instead of failing deserialization at the first gap.

use super::units::QuantityKind;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Schema name written into every dataset header.
pub const SCHEMA_NAME: &str = "openwafer-genealogy";
/// Semantic version of the schema. Bump the minor version for additive changes, the major
/// version for anything that changes the meaning of an existing field.
pub const SCHEMA_VERSION: &str = "0.1.0";

/// Where a record's content came from. The schema's central physics rule: SIMULATED and
/// MEASURED records never mix silently (see [`super::validate`]).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Origin {
    /// Read from an instrument on real material.
    Measured,
    /// Produced by a model or simulator. No physical material is involved.
    Simulated,
    /// Written down by a person as an intention: a recipe, a traveler, a setpoint table.
    Specified,
}

/// What a whole dataset claims to contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeclaredOrigin {
    /// Real fab records: MEASURED and SPECIFIED only. A SIMULATED record is an error.
    Measured,
    /// Simulation output: SIMULATED and SPECIFIED only. A MEASURED record is an error.
    Simulated,
    /// Both, on purpose (for example virtual-metrology predictions about real wafers). Every
    /// query result is labelled with the origins it touched.
    Mixed,
}

/// Where one record came from. Required on every record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Provenance {
    pub origin: Origin,
    /// A URL, a path relative to the dataset, or a tool identifier.
    pub source: String,
    /// The exact version of the source: a git commit, a file SHA-256, a software version.
    #[serde(default)]
    pub source_version: Option<String>,
    /// Where inside the source: a row, a table, a line range, a cell.
    #[serde(default)]
    pub locator: Option<String>,
    /// The adapter or program that produced this record, with its version.
    pub produced_by: String,
    /// Date the source was retrieved, `YYYY-MM-DD`.
    #[serde(default)]
    pub retrieved: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// The licence a record may be redistributed under. Required on every record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct License {
    /// An SPDX licence expression (`CC-BY-4.0`, `CC0-1.0`, `CC-BY-SA-4.0 AND CERN-OHL-S-2.0`).
    /// `NOASSERTION` records that the licence is unknown: such records validate but are not
    /// redistributable and are dropped by `export --redistributable-only`.
    pub spdx: String,
    /// The attribution the licence requires.
    #[serde(default)]
    pub attribution: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Uncertainty of a quantity, in the quantity's own unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Uncertainty {
    /// Standard uncertainty `u` (coverage factor k = 1).
    Standard {
        u: f64,
        /// How `u` was obtained: "type A, n = 25 repeats", "instrument spec", "model posterior s.d.".
        #[serde(default)]
        basis: Option<String>,
    },
    /// Expanded uncertainty `U = k·u`.
    Expanded {
        u: f64,
        k: f64,
        #[serde(default)]
        basis: Option<String>,
    },
    /// The uncertainty is not known. The reason is required.
    Unknown { reason: String },
    /// The value is a setpoint or a commanded value, not a measurement.
    Setpoint {
        #[serde(default)]
        tolerance: Option<f64>,
    },
}

/// How a derived value was computed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Derivation {
    /// The method, specific enough to reproduce: "Rs = (π/ln 2)·mean(V/I), collinear 4PP".
    pub method: String,
    /// Ids of the records (or names of the quantities) it was computed from.
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// A number with its unit, its uncertainty and, if derived, its method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Quantity {
    pub value: f64,
    /// Unit symbol (see `units::parse`). Required.
    #[serde(default)]
    pub unit: Option<String>,
    /// Required on every measured or derived value; [`Uncertainty::Setpoint`] on setpoints.
    #[serde(default)]
    pub uncertainty: Option<Uncertainty>,
    /// Present exactly when the value was computed from other values.
    #[serde(default)]
    pub derivation: Option<Derivation>,
}

impl Quantity {
    pub fn setpoint(value: f64, unit: &str) -> Quantity {
        Quantity {
            value,
            unit: Some(unit.into()),
            uncertainty: Some(Uncertainty::Setpoint { tolerance: None }),
            derivation: None,
        }
    }
    pub fn measured(value: f64, unit: &str, u: Uncertainty) -> Quantity {
        Quantity {
            value,
            unit: Some(unit.into()),
            uncertainty: Some(u),
            derivation: None,
        }
    }
    pub fn unknown_u(value: f64, unit: &str, reason: &str) -> Quantity {
        Quantity::measured(
            value,
            unit,
            Uncertainty::Unknown {
                reason: reason.into(),
            },
        )
    }
    pub fn derived(
        value: f64,
        unit: &str,
        u: Uncertainty,
        method: &str,
        inputs: Vec<String>,
    ) -> Quantity {
        Quantity {
            value,
            unit: Some(unit.into()),
            uncertainty: Some(u),
            derivation: Some(Derivation {
                method: method.into(),
                inputs,
            }),
        }
    }
}

/// A named quantity with a declared kind. The kind fixes the dimension of the unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Parameter {
    pub name: String,
    #[serde(default)]
    pub kind: Option<QuantityKind>,
    pub quantity: Quantity,
}

/// A categorical value: a chemical, a resist name, a yes/no.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

// ───────────────────────────── genealogy ─────────────────────────────

/// Levels of the material hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    Lot,
    Wafer,
    /// A piece that is not a wafer: a cleaved chip, a glass slide, a coupon.
    Substrate,
    Chip,
    Die,
    /// A test structure or measurement location on a die or chip.
    TestSite,
}

impl UnitKind {
    /// Allowed parent kinds. A lot has no parent; a test site sits on anything below a lot.
    pub fn allows_parent(self, parent: UnitKind) -> bool {
        use UnitKind::*;
        match self {
            Lot => false,
            Wafer | Substrate => matches!(parent, Lot | Wafer | Substrate),
            Chip | Die => matches!(parent, Lot | Wafer | Substrate | Chip),
            TestSite => matches!(parent, Wafer | Substrate | Chip | Die),
        }
    }
}

/// Position in a parent's map, after SEMI E142's substrate map (die index and coordinates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MapPosition {
    #[serde(default)]
    pub x_index: Option<i64>,
    #[serde(default)]
    pub y_index: Option<i64>,
    #[serde(default)]
    pub x: Option<Quantity>,
    #[serde(default)]
    pub y: Option<Quantity>,
    /// Where (0, 0) is: "center", "lower_left", "notch_down_lower_left" (E142 OriginLocation).
    #[serde(default)]
    pub origin: Option<String>,
}

/// The starting material of a wafer or substrate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Substrate {
    /// "Si", "glass", "SOI".
    pub material: String,
    #[serde(default)]
    pub dopant_type: Option<String>,
    #[serde(default)]
    pub orientation: Option<String>,
    #[serde(default)]
    pub properties: Vec<Parameter>,
}

/// One piece of material at one level of the hierarchy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MaterialUnit {
    pub id: String,
    pub kind: UnitKind,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// The fab site that holds it.
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub substrate: Option<Substrate>,
    #[serde(default)]
    pub position: Option<MapPosition>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GenealogyOp {
    /// New material enters the fab (`outputs` only).
    Create,
    /// One unit becomes several: lot split, wafer cleaved into chips.
    Split,
    /// Several units become one: lot merge.
    Merge,
    /// A step is redone on the same material; `run` names the run being redone.
    Rework,
    /// Material leaves the fab (`inputs` only).
    Scrap,
}

/// A change to the hierarchy itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GenealogyEvent {
    pub id: String,
    pub op: GenealogyOp,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    /// RFC 3339 time.
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

// ───────────────────────────── recipes and runs ─────────────────────────────

/// A versioned recipe: parameters are setpoints with units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The recipe id this version replaces.
    #[serde(default)]
    pub supersedes: Option<String>,
    /// "rf_sputter", "plasma_etch", "spin_coat", "bake", "diffusion", "wet_etch", "litho_expose".
    #[serde(default)]
    pub step_kind: Option<String>,
    #[serde(default)]
    pub tool_class: Option<String>,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Aborted,
    Faulted,
    Unknown,
}

/// One execution of a step on a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessRun {
    pub id: String,
    #[serde(default)]
    pub recipe: Option<String>,
    pub tool: String,
    #[serde(default)]
    pub chamber: Option<String>,
    /// The flow or traveler the step belongs to.
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub step_no: Option<u32>,
    #[serde(default)]
    pub step_name: Option<String>,
    /// Material units processed together in this run.
    #[serde(default)]
    pub material: Vec<String>,
    /// RFC 3339 start and end.
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    /// The operator's role ("student", "technician", "automation"), never a name.
    #[serde(default)]
    pub operator_role: Option<String>,
    pub status: RunStatus,
    /// As-run setpoints that complete or override the recipe.
    #[serde(default)]
    pub settings: Vec<Parameter>,
    /// Values the tool reported for this run.
    #[serde(default)]
    pub actuals: Vec<Parameter>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

// ───────────────────────────── tool trace ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CalStatus {
    /// Calibrated on `date` against `reference`.
    Calibrated,
    /// Never calibrated, or no record of it. Stated, not omitted.
    Unknown,
    /// A simulated sensor or a counter that has nothing to calibrate.
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Calibration {
    pub status: CalStatus,
    /// `YYYY-MM-DD`; required when `status` is `calibrated`.
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub reference: Option<String>,
}

/// A flag on one sample of a trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SampleFlag {
    pub index: u64,
    /// "physically_impossible", "missing", "saturated".
    pub code: String,
}

/// One sensor's time series during one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Trace {
    pub id: String,
    pub run: String,
    pub tool: String,
    pub sensor: String,
    #[serde(default)]
    pub kind: Option<QuantityKind>,
    #[serde(default)]
    pub unit: Option<String>,
    /// Uniform sampling rate. Exactly one of `sample_rate_hz` and `times_s` is given.
    #[serde(default)]
    pub sample_rate_hz: Option<f64>,
    /// Sample times in seconds from `t0`, for irregular sampling.
    #[serde(default)]
    pub times_s: Vec<f64>,
    /// RFC 3339 time of the first sample.
    #[serde(default)]
    pub t0: Option<String>,
    /// Sample values in `unit`. `null` is a missing sample.
    #[serde(default)]
    pub values: Vec<Option<f64>>,
    #[serde(default)]
    pub uncertainty: Option<Uncertainty>,
    #[serde(default)]
    pub calibration: Option<Calibration>,
    #[serde(default)]
    pub flags: Vec<SampleFlag>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

// ───────────────────────────── metrology ─────────────────────────────

/// One measured value at one place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetroPoint {
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub position: Option<MapPosition>,
    pub value: Quantity,
}

/// A measurement of one parameter on one material unit, at one or more sites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Metrology {
    pub id: String,
    pub material: String,
    /// The process run whose result this measures.
    #[serde(default)]
    pub after_run: Option<String>,
    /// "XRR", "reflectometry", "profilometer", "QCM", "CD-SEM", "AFM", "4PP", "virtual metrology".
    pub method: String,
    #[serde(default)]
    pub tool: Option<String>,
    pub parameter: String,
    #[serde(default)]
    pub kind: Option<QuantityKind>,
    #[serde(default)]
    pub points: Vec<MetroPoint>,
    /// Measurement conditions: model assumptions (n = 1.6), temperature, probe spacing.
    #[serde(default)]
    pub conditions: Vec<Parameter>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

// ───────────────────────────── electrical test ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BiasMode {
    Fixed {
        value: Quantity,
    },
    Sweep {
        start: Quantity,
        stop: Quantity,
        #[serde(default)]
        step: Option<Quantity>,
    },
}

/// The bias on one terminal during a test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Bias {
    /// "gate", "drain", "source", "body", "force+", "sense-".
    pub terminal: String,
    pub mode: BiasMode,
}

/// One axis of a measured curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Axis {
    pub name: String,
    #[serde(default)]
    pub kind: Option<QuantityKind>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub values: Vec<Option<f64>>,
    /// Present when the axis was computed, not read: drain current from a series-resistor voltage.
    #[serde(default)]
    pub derivation: Option<Derivation>,
}

/// A raw curve: y against x at fixed other biases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Curve {
    pub name: String,
    pub x: Axis,
    pub y: Axis,
    #[serde(default)]
    pub fixed: Vec<Parameter>,
    #[serde(default)]
    pub uncertainty: Option<Uncertainty>,
}

/// An extracted parameter. Its quantity must carry a derivation: extraction is a method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Extracted {
    pub name: String,
    #[serde(default)]
    pub kind: Option<QuantityKind>,
    pub quantity: Quantity,
}

/// One electrical test of one structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ElectricalTest {
    pub id: String,
    pub material: String,
    /// "NMOS W/L", "collinear 4-point probe", "MOS capacitor".
    pub structure: String,
    #[serde(default)]
    pub structure_params: Vec<Parameter>,
    #[serde(default)]
    pub instrument: Option<String>,
    #[serde(default)]
    pub bias: Vec<Bias>,
    #[serde(default)]
    pub curves: Vec<Curve>,
    #[serde(default)]
    pub extracted: Vec<Extracted>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

/// A population statistic published without its members: "Vth = 1.40 ± 0.64 V over 5 devices".
/// `sd` is the spread of the population, not the uncertainty of the mean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Summary {
    pub id: String,
    /// The material unit the population lives on.
    pub scope: String,
    /// Which members: "NMOS 1 (L = 5 µm), functional devices".
    pub population: String,
    pub parameter: String,
    #[serde(default)]
    pub kind: Option<QuantityKind>,
    #[serde(default)]
    pub unit: Option<String>,
    pub n: u64,
    pub mean: f64,
    #[serde(default)]
    pub sd: Option<f64>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub median: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// How each member value was obtained (the extraction method). Required.
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

// ───────────────────────────── yield and defects ─────────────────────────────

/// A pass count at one stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct YieldRecord {
    pub id: String,
    pub scope: String,
    /// "visual", "electrical", "sort", "parametric".
    pub stage: String,
    /// What counted as a pass.
    pub criterion: String,
    pub tested: u64,
    pub passed: u64,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

/// One defect, or a count of a class of defects, found on a material unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Defect {
    pub id: String,
    pub material: String,
    #[serde(default)]
    pub after_run: Option<String>,
    /// "blister", "particle", "crack", "killer".
    pub class: String,
    pub method: String,
    #[serde(default)]
    pub position: Option<MapPosition>,
    #[serde(default)]
    pub size: Option<Quantity>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

/// One bin code of a bin map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BinDef {
    pub code: u32,
    pub name: String,
    pub pass: bool,
}

/// One die's bin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DieBin {
    #[serde(default)]
    pub die: Option<String>,
    #[serde(default)]
    pub index: Option<u64>,
    #[serde(default)]
    pub position: Option<MapPosition>,
    pub bin: u32,
}

/// A wafer's sort result, after SEMI E142's bin map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BinMap {
    pub id: String,
    pub material: String,
    #[serde(default)]
    pub after_run: Option<String>,
    pub bin_defs: Vec<BinDef>,
    #[serde(default)]
    pub dies: Vec<DieBin>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

/// A qualitative result: an inspection grade, a yes/no. Not a quantity, and never coerced to one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Observation {
    pub id: String,
    pub material: String,
    #[serde(default)]
    pub after_run: Option<String>,
    pub method: String,
    pub name: String,
    pub value: String,
    /// The value's scale: "1 (worst) to 5 (best)", "Y/N".
    #[serde(default)]
    pub scale: Option<String>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

/// A model estimate about some material: a defect-density posterior, a fitted parameter.
/// Its quantity must carry a derivation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Estimate {
    pub id: String,
    pub scope: Vec<String>,
    pub parameter: String,
    #[serde(default)]
    pub kind: Option<QuantityKind>,
    pub quantity: Quantity,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub license: Option<License>,
}

// ───────────────────────────── the dataset ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Header {
    pub schema: String,
    pub schema_version: String,
    pub dataset_id: String,
    pub title: String,
    pub declared_origin: DeclaredOrigin,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub created: Option<String>,
}

impl Header {
    pub fn new(dataset_id: &str, title: &str, declared_origin: DeclaredOrigin) -> Header {
        Header {
            schema: SCHEMA_NAME.into(),
            schema_version: SCHEMA_VERSION.into(),
            dataset_id: dataset_id.into(),
            title: title.into(),
            declared_origin,
            description: None,
            created: None,
        }
    }
}

/// A complete dataset: a header and thirteen tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Dataset {
    pub header: Header,
    #[serde(default)]
    pub units: Vec<MaterialUnit>,
    #[serde(default)]
    pub events: Vec<GenealogyEvent>,
    #[serde(default)]
    pub recipes: Vec<Recipe>,
    #[serde(default)]
    pub runs: Vec<ProcessRun>,
    #[serde(default)]
    pub traces: Vec<Trace>,
    #[serde(default)]
    pub metrology: Vec<Metrology>,
    #[serde(default)]
    pub electrical: Vec<ElectricalTest>,
    #[serde(default)]
    pub summaries: Vec<Summary>,
    #[serde(default)]
    pub yields: Vec<YieldRecord>,
    #[serde(default)]
    pub defects: Vec<Defect>,
    #[serde(default)]
    pub bin_maps: Vec<BinMap>,
    #[serde(default)]
    pub observations: Vec<Observation>,
    #[serde(default)]
    pub estimates: Vec<Estimate>,
}

/// The table names, in the order they are written.
pub const TABLES: [&str; 13] = [
    "units",
    "events",
    "recipes",
    "runs",
    "traces",
    "metrology",
    "electrical",
    "summaries",
    "yields",
    "defects",
    "bin_maps",
    "observations",
    "estimates",
];

impl Dataset {
    pub fn new(header: Header) -> Dataset {
        Dataset {
            header,
            units: vec![],
            events: vec![],
            recipes: vec![],
            runs: vec![],
            traces: vec![],
            metrology: vec![],
            electrical: vec![],
            summaries: vec![],
            yields: vec![],
            defects: vec![],
            bin_maps: vec![],
            observations: vec![],
            estimates: vec![],
        }
    }

    /// Number of records over all tables.
    pub fn len(&self) -> usize {
        self.counts().iter().map(|c| c.1).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn counts(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("units", self.units.len()),
            ("events", self.events.len()),
            ("recipes", self.recipes.len()),
            ("runs", self.runs.len()),
            ("traces", self.traces.len()),
            ("metrology", self.metrology.len()),
            ("electrical", self.electrical.len()),
            ("summaries", self.summaries.len()),
            ("yields", self.yields.len()),
            ("defects", self.defects.len()),
            ("bin_maps", self.bin_maps.len()),
            ("observations", self.observations.len()),
            ("estimates", self.estimates.len()),
        ]
    }

    /// Append every record of `other` (headers are kept from `self`).
    pub fn extend(&mut self, other: Dataset) {
        self.units.extend(other.units);
        self.events.extend(other.events);
        self.recipes.extend(other.recipes);
        self.runs.extend(other.runs);
        self.traces.extend(other.traces);
        self.metrology.extend(other.metrology);
        self.electrical.extend(other.electrical);
        self.summaries.extend(other.summaries);
        self.yields.extend(other.yields);
        self.defects.extend(other.defects);
        self.bin_maps.extend(other.bin_maps);
        self.observations.extend(other.observations);
        self.estimates.extend(other.estimates);
    }
}
