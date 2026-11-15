//! Units for the genealogy schema: parse a unit symbol into a dimension and a scale.
//!
//! Every quantity in the schema carries a unit string. This module decides whether that
//! string means anything, which physical dimension it has, and how it converts to SI. It is
//! deliberately small and closed: a unit it does not know is an error, not a guess.
//!
//! Three things it refuses to blur:
//!
//! * **`C` is not Celsius.** In SI, `C` is the coulomb. Celsius is `degC` or `°C`.
//! * **`sccm` is not converted to mol/s.** A standard cubic centimetre depends on which
//!   "standard" conditions the flow controller uses (0 °C or 20 °C, 1 atm or 1 bar), so it
//!   gets its own pseudo-dimension and only converts to `slm`.
//! * **Same dimension is not same unit.** `nm` and `µm` are both lengths, and a column that
//!   mixes them is corrupt for training. [`normalize`] gives the canonical spelling the
//!   validator compares; [`Unit::dim`] is only for the dimension check.

use serde::{Deserialize, Serialize};

/// Exponents of the SI base dimensions, plus two pseudo-dimensions the schema keeps apart:
/// `[m, kg, s, A, K, mol, cd, gasflow]`.
pub type Dim = [i8; 8];

const DIMLESS: Dim = [0; 8];
const M: Dim = [1, 0, 0, 0, 0, 0, 0, 0];
const KG: Dim = [0, 1, 0, 0, 0, 0, 0, 0];
const S: Dim = [0, 0, 1, 0, 0, 0, 0, 0];
const A: Dim = [0, 0, 0, 1, 0, 0, 0, 0];
const K: Dim = [0, 0, 0, 0, 1, 0, 0, 0];
const MOL: Dim = [0, 0, 0, 0, 0, 1, 0, 0];
const CD: Dim = [0, 0, 0, 0, 0, 0, 1, 0];
const GASFLOW: Dim = [0, 0, 0, 0, 0, 0, 0, 1];

fn dmul(a: Dim, b: Dim, sign: i8) -> Dim {
    let mut o = a;
    for i in 0..8 {
        o[i] += sign * b[i];
    }
    o
}

fn dpow(a: Dim, p: i8) -> Dim {
    let mut o = a;
    for x in &mut o {
        *x *= p;
    }
    o
}

/// A parsed unit: `value_SI = value × scale + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub dim: Dim,
    pub scale: f64,
    /// Non-zero only for affine temperature scales (°C).
    pub offset: f64,
}

impl Unit {
    pub fn to_si(&self, v: f64) -> f64 {
        v * self.scale + self.offset
    }
    pub fn from_si(&self, v: f64) -> f64 {
        (v - self.offset) / self.scale
    }
}

/// The physical kind of a quantity. A declared kind fixes the dimension its unit must have.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum QuantityKind {
    Length,
    Area,
    Time,
    Temperature,
    Pressure,
    Power,
    Voltage,
    Current,
    Resistance,
    /// Ω per square. Dimensionally a resistance; kept apart so it cannot be joined to one.
    SheetResistance,
    Resistivity,
    Capacitance,
    Frequency,
    /// rpm or rad/s.
    RotationRate,
    /// sccm, slm.
    GasFlow,
    /// Length per time: deposition and etch rates.
    Rate,
    Energy,
    Angle,
    /// Per area: defect density.
    ArealDensity,
    /// Volume or mass fraction, %, ratio.
    Fraction,
    /// Detector counts (OES, XPS).
    Counts,
    /// Pure number: VSWR, ratios, indices.
    Dimensionless,
    Transconductance,
    Mass,
    Volume,
    Concentration,
}

impl QuantityKind {
    /// The dimension a unit for this kind must have.
    pub fn dim(self) -> Dim {
        use QuantityKind::*;
        match self {
            Length => M,
            Area => dpow(M, 2),
            Time => S,
            Temperature => K,
            Pressure => dmul(dmul(KG, M, -1), dpow(S, 2), -1),
            Power => dmul(dmul(KG, dpow(M, 2), 1), dpow(S, 3), -1),
            Voltage => volt(),
            Current => A,
            Resistance | SheetResistance => ohm(),
            Resistivity => dmul(ohm(), M, 1),
            Capacitance => dmul(
                dpow(S, 4),
                dmul(dmul(KG, dpow(M, 2), 1), dpow(A, -2), 1),
                -1,
            ),
            Frequency | RotationRate => dpow(S, -1),
            GasFlow => GASFLOW,
            Rate => dmul(M, S, -1),
            Energy => dmul(dmul(KG, dpow(M, 2), 1), dpow(S, 2), -1),
            Angle | Fraction | Counts | Dimensionless => DIMLESS,
            ArealDensity => dpow(M, -2),
            Transconductance => dmul(A, volt(), -1),
            Mass => KG,
            Volume => dpow(M, 3),
            Concentration => dmul(MOL, dpow(M, 3), -1),
        }
    }
}

fn volt() -> Dim {
    // kg m² s⁻³ A⁻¹
    dmul(dmul(dmul(KG, dpow(M, 2), 1), dpow(S, 3), -1), A, -1)
}

fn ohm() -> Dim {
    dmul(volt(), A, -1)
}

/// Canonical spelling: micro signs to `u`, `Ω` to `ohm`, `°C` to `degC`, `Å` to `angstrom`,
/// whitespace removed, `·` and `*` to `.`. Two units are "the same unit" only if their
/// canonical spellings are equal.
pub fn normalize(s: &str) -> String {
    let mut t: String = s.trim().chars().filter(|c| !c.is_whitespace()).collect();
    for (from, to) in [
        ("µ", "u"),
        ("μ", "u"),
        ("\u{3a9}", "ohm"),
        ("\u{2126}", "ohm"),
        ("°C", "degC"),
        ("℃", "degC"),
        ("\u{c5}", "angstrom"),
        ("\u{212b}", "angstrom"),
        ("·", "."),
        ("*", "."),
        ("^", ""),
        ("²", "2"),
        ("³", "3"),
        ("⁻", "-"),
        ("Ohm", "ohm"),
        ("ohms", "ohm"),
    ] {
        t = t.replace(from, to);
    }
    // Whole-token aliases for time, which people spell many ways.
    match t.as_str() {
        "sec" | "secs" | "second" | "seconds" => "s".into(),
        "mins" | "minute" | "minutes" => "min".into(),
        "hr" | "hrs" | "hour" | "hours" => "h".into(),
        "Watts" | "watts" | "Watt" | "watt" => "W".into(),
        "Volts" | "volts" => "V".into(),
        "ohm/square" | "ohm/sqr" | "ohm/sq." | "ohms/sq" => "ohm/sq".into(),
        _ => t,
    }
}

fn prefix(c: &str) -> Option<f64> {
    Some(match c {
        "p" => 1e-12,
        "n" => 1e-9,
        "u" => 1e-6,
        "m" => 1e-3,
        "c" => 1e-2,
        "h" => 1e2,
        "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        _ => return None,
    })
}

/// A unit atom without prefix: (dimension, scale, offset, accepts SI prefixes).
fn atom(s: &str) -> Option<(Dim, f64, f64, bool)> {
    let pa = dmul(dmul(KG, M, -1), dpow(S, 2), -1);
    let w = dmul(dmul(KG, dpow(M, 2), 1), dpow(S, 3), -1);
    let j = dmul(dmul(KG, dpow(M, 2), 1), dpow(S, 2), -1);
    Some(match s {
        "m" => (M, 1.0, 0.0, true),
        "angstrom" => (M, 1e-10, 0.0, false),
        "in" => (M, 0.0254, 0.0, false),
        "g" => (KG, 1e-3, 0.0, true),
        "s" => (S, 1.0, 0.0, true),
        "sec" | "secs" => (S, 1.0, 0.0, false),
        "min" => (S, 60.0, 0.0, false),
        "h" => (S, 3600.0, 0.0, false),
        "d" => (S, 86_400.0, 0.0, false),
        "A" => (A, 1.0, 0.0, true),
        "K" => (K, 1.0, 0.0, false),
        "degC" => (K, 1.0, 273.15, false),
        "mol" => (MOL, 1.0, 0.0, true),
        "cd" => (CD, 1.0, 0.0, false),
        "Hz" => (dpow(S, -1), 1.0, 0.0, true),
        "rpm" => (dpow(S, -1), 1.0 / 60.0, 0.0, false),
        "Pa" => (pa, 1.0, 0.0, true),
        "bar" => (pa, 1e5, 0.0, true),
        "Torr" => (pa, 101_325.0 / 760.0, 0.0, true),
        "atm" => (pa, 101_325.0, 0.0, false),
        "W" => (w, 1.0, 0.0, true),
        "J" => (j, 1.0, 0.0, true),
        "eV" => (j, 1.602_176_634e-19, 0.0, true),
        "V" => (volt(), 1.0, 0.0, true),
        "ohm" => (ohm(), 1.0, 0.0, true),
        "S" => (dpow(ohm(), -1), 1.0, 0.0, true),
        "F" => (QuantityKind::Capacitance.dim(), 1.0, 0.0, true),
        "L" => (dpow(M, 3), 1e-3, 0.0, true),
        "sccm" => (GASFLOW, 1.0, 0.0, false),
        "slm" => (GASFLOW, 1000.0, 0.0, false),
        "%" => (DIMLESS, 0.01, 0.0, false),
        "1" => (DIMLESS, 1.0, 0.0, false),
        "rad" => (DIMLESS, 1.0, 0.0, false),
        "deg" => (DIMLESS, std::f64::consts::PI / 180.0, 0.0, false),
        "counts" | "count" => (DIMLESS, 1.0, 0.0, false),
        "sq" => (DIMLESS, 1.0, 0.0, false),
        _ => return None,
    })
}

/// One factor such as `nm`, `cm2`, `cm-2`, `s-1`, `kohm`.
fn factor(tok: &str) -> Option<(Dim, f64, f64)> {
    if tok.is_empty() {
        return None;
    }
    // Split a trailing signed integer exponent: cm2, cm-2, s-1.
    let bytes = tok.as_bytes();
    let mut cut = bytes.len();
    while cut > 0 && bytes[cut - 1].is_ascii_digit() {
        cut -= 1;
    }
    if cut > 0 && cut < bytes.len() && bytes[cut - 1] == b'-' {
        cut -= 1;
    }
    let (base, exp) = if cut == 0 || cut == bytes.len() {
        (tok, 1i8)
    } else {
        (&tok[..cut], tok[cut..].parse::<i8>().ok()?)
    };
    if base == "1" && exp == 1 {
        return Some((DIMLESS, 1.0, 0.0));
    }
    let (d, sc, off) = if let Some((d, sc, off, _)) = atom(base) {
        (d, sc, off)
    } else {
        // Try one SI prefix.
        let mut found = None;
        for (i, _) in base.char_indices().skip(1).take(1) {
            let (p, rest) = base.split_at(i);
            if let (Some(f), Some((d, sc, off, pre))) = (prefix(p), atom(rest)) {
                if pre {
                    found = Some((d, sc * f, off));
                }
            }
        }
        found?
    };
    if off != 0.0 && exp != 1 {
        return None; // °C² is meaningless
    }
    Some((dpow(d, exp), sc.powi(exp as i32), off))
}

/// Parse a unit expression: factors joined by `.`, divided by `/` (left to right).
pub fn parse(s: &str) -> Option<Unit> {
    let n = normalize(s);
    if n.is_empty() {
        return None;
    }
    if n == "ohm/sq" {
        return Some(Unit {
            dim: ohm(),
            scale: 1.0,
            offset: 0.0,
        });
    }
    let mut dim = DIMLESS;
    let mut scale = 1.0;
    let mut offset = 0.0;
    let mut nfactors = 0;
    for (i, part) in n.split('/').enumerate() {
        let sign = if i == 0 { 1 } else { -1 };
        for tok in part.split('.') {
            let (d, sc, off) = factor(tok)?;
            nfactors += 1;
            dim = dmul(dim, d, sign);
            scale = if sign > 0 { scale * sc } else { scale / sc };
            if off != 0.0 {
                offset = off;
            }
        }
    }
    if offset != 0.0 && nfactors > 1 {
        return None; // degC/min is a rate of change: write K/min
    }
    Some(Unit { dim, scale, offset })
}

/// Does `unit` fit the declared `kind`?
pub fn fits(kind: QuantityKind, unit: &str) -> Option<bool> {
    parse(unit).map(|u| u.dim == kind.dim())
}

/// Convert `v` from one unit to another of the same dimension.
pub fn convert(v: f64, from: &str, to: &str) -> Option<f64> {
    let (a, b) = (parse(from)?, parse(to)?);
    if a.dim != b.dim {
        return None;
    }
    Some(b.from_si(a.to_si(v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_convert_and_keep_distinct_spellings() {
        assert_eq!(normalize("µm"), normalize("um"));
        assert_eq!(normalize("μm"), "um");
        assert_ne!(
            normalize("nm"),
            normalize("um"),
            "same dimension is not the same unit"
        );
        assert!((convert(150.0, "nm", "um").unwrap() - 0.150).abs() < 1e-12);
        assert!((convert(1.0, "angstrom/s", "nm/min").unwrap() - 6.0).abs() < 1e-12);
        assert!((convert(1.0, "Å/s", "nm/min").unwrap() - 6.0).abs() < 1e-12);
    }

    #[test]
    fn pressure_temperature_and_flow() {
        assert!((convert(4e-3, "hPa", "Torr").unwrap() - 3.0002e-3).abs() < 1e-6);
        assert!((convert(25.0, "degC", "K").unwrap() - 298.15).abs() < 1e-9);
        assert!((convert(1.0, "slm", "sccm").unwrap() - 1000.0).abs() < 1e-9);
        assert!(
            convert(1.0, "sccm", "mol/s").is_none(),
            "sccm must not silently become mol/s"
        );
        assert!(
            parse("C").map(|u| u.dim) != Some(K),
            "C is the coulomb, not Celsius"
        );
    }

    #[test]
    fn electrical_and_compound_units() {
        assert_eq!(fits(QuantityKind::SheetResistance, "ohm/sq"), Some(true));
        assert_eq!(fits(QuantityKind::Resistivity, "ohm.m"), Some(true));
        assert_eq!(fits(QuantityKind::Resistivity, "ohm*cm"), Some(true));
        assert_eq!(fits(QuantityKind::Current, "mA"), Some(true));
        assert_eq!(fits(QuantityKind::Current, "µA"), Some(true));
        assert_eq!(fits(QuantityKind::ArealDensity, "cm-2"), Some(true));
        assert_eq!(fits(QuantityKind::ArealDensity, "1/cm2"), Some(true));
        assert_eq!(fits(QuantityKind::Transconductance, "A/V"), Some(true));
        assert_eq!(fits(QuantityKind::Rate, "nm/min"), Some(true));
        assert_eq!(fits(QuantityKind::RotationRate, "rpm"), Some(true));
        assert_eq!(fits(QuantityKind::Length, "s"), Some(false));
        assert_eq!(
            parse("furlong"),
            None,
            "an unknown unit is an error, not a guess"
        );
        assert_eq!(parse(""), None);
    }
}
