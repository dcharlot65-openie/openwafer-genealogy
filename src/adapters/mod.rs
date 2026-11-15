//! Adapters: turn the files a fab already produces into schema records.
//!
//! Each adapter reads one format, writes every record with a [`Provenance`] that points at the
//! exact source (URL, commit or file hash, row) and the [`License`] the caller states, and
//! never invents a value. Where the source has no unit, the adapter either knows the unit from
//! the tool that wrote the file (and says how it knows) or leaves the quantity without one, so
//! the validator refuses it.

pub mod hackerfab;
pub mod sheet;
pub mod xlsx;

use super::record::*;
use super::units::{self, QuantityKind};

/// Where an input came from, and under what terms.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// URL of the file, ideally pinned to a commit.
    pub url: String,
    /// Commit, release or SHA-256 of the file.
    pub version: Option<String>,
    /// `YYYY-MM-DD`.
    pub retrieved: Option<String>,
    pub license: License,
    /// MEASURED for instrument output, SPECIFIED for a traveler or recipe file, SIMULATED for a
    /// synthetic test input.
    pub origin: Origin,
    /// The fab site the data belongs to.
    pub site: Option<String>,
    pub note: Option<String>,
}

impl Source {
    pub fn new(url: &str, spdx: &str, origin: Origin) -> Source {
        Source {
            url: url.into(),
            version: None,
            retrieved: None,
            license: License {
                spdx: spdx.into(),
                attribution: None,
                url: None,
            },
            origin,
            site: None,
            note: None,
        }
    }
    pub fn prov(&self, adapter: &str, locator: Option<String>) -> Option<Provenance> {
        Some(Provenance {
            origin: self.origin,
            source: self.url.clone(),
            source_version: self.version.clone(),
            locator,
            produced_by: format!("owg {} {adapter}", env!("CARGO_PKG_VERSION")),
            retrieved: self.retrieved.clone(),
            note: self.note.clone(),
        })
    }
    pub fn prov_note(
        &self,
        adapter: &str,
        locator: Option<String>,
        note: &str,
    ) -> Option<Provenance> {
        let mut p = self.prov(adapter, locator)?;
        p.note = Some(match &self.note {
            Some(n) => format!("{n} {note}"),
            None => note.to_string(),
        });
        Some(p)
    }
    pub fn lic(&self) -> Option<License> {
        Some(self.license.clone())
    }
    pub fn dataset(&self, id: &str, title: &str) -> Dataset {
        let declared = match self.origin {
            Origin::Simulated => DeclaredOrigin::Simulated,
            _ => DeclaredOrigin::Measured,
        };
        Dataset::new(Header::new(id, title, declared))
    }
}

/// Lowercase alphanumerics and dashes.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// snake_case name of a free-text parameter key.
pub fn snake(s: &str) -> String {
    slug(s).replace('-', "_")
}

/// RFC 3339 UTC time for seconds since the Unix epoch, to the millisecond.
pub fn rfc3339(t: f64) -> String {
    let ms_total = (t * 1000.0).round() as i64;
    let (secs, ms) = (ms_total.div_euclid(1000), ms_total.rem_euclid(1000));
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        sod / 3600,
        (sod / 60) % 60,
        sod % 60
    )
}

/// Unit spellings seen in fab documents, mapped to the unit table's.
pub fn unit_alias(u: &str) -> String {
    let t = u.trim();
    match t {
        "°C" | "ºC" | "C°" | "degC" | "deg C" => "degC".into(),
        "sec" | "secs" | "s" | "seconds" | "second" => "s".into(),
        "min" | "mins" | "minute" | "minutes" | "Minutes" | "Minute" => "min".into(),
        "hr" | "hrs" | "hour" | "hours" => "h".into(),
        "SCCM" | "sccm" => "sccm".into(),
        "Watts" | "watts" | "W" | "Watt" => "W".into(),
        "\"" | "in" | "inch" | "inches" => "in".into(),
        "RPM" | "rpm" => "rpm".into(),
        "\u{c5}" | "\u{212b}" => "angstrom".into(),
        "um" | "µm" | "μm" | "micron" | "microns" => "um".into(),
        "ohm" | "Ω" | "ohms" | "Ohm" => "ohm".into(),
        "ohm/square" | "ohm/sq" | "Ω/sq" | "Ω/□" => "ohm/sq".into(),
        other => other.replace(['\u{c5}', '\u{212b}'], "angstrom"),
    }
}

/// Split "4.7E-3 Torr", ".25 Å/s", "15 Minutes", `3"` into a number and a unit.
pub fn number_unit(s: &str) -> Option<(f64, String)> {
    let t = s.trim();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
            j += 1;
        }
        let k = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > k {
            i = j;
        }
    }
    let v: f64 = t[..i].parse().ok()?;
    let unit = t[i..].trim();
    Some((v, unit_alias(unit)))
}

/// The kind a parameter most plausibly has, from its unit's dimension and its name.
pub fn kind_for(name: &str, unit: &str) -> Option<QuantityKind> {
    use QuantityKind::*;
    let u = units::parse(unit)?;
    let n = name.to_lowercase();
    let mut order: Vec<QuantityKind> = Vec::new();
    if n.contains("speed") || n.contains("rpm") || unit == "rpm" {
        order.push(RotationRate);
    }
    if n.contains("sheet") {
        order.push(SheetResistance);
    }
    if unit == "%" {
        order.push(Fraction);
    }
    if unit == "rad" || unit == "deg" || n.contains("angle") || n.contains("_pos") {
        order.push(Angle);
    }
    if unit.contains("count") {
        order.push(Counts);
    }
    order.extend([
        Length,
        Time,
        Temperature,
        Pressure,
        Power,
        Voltage,
        Current,
        Resistance,
        Resistivity,
        Frequency,
        GasFlow,
        Rate,
        Energy,
        ArealDensity,
        Area,
        Capacitance,
        Transconductance,
        Mass,
        Volume,
        Concentration,
        Dimensionless,
    ]);
    order.into_iter().find(|k| k.dim() == u.dim)
}

/// A setpoint parameter from a number and a unit, with its kind inferred.
pub fn setpoint(name: &str, v: f64, unit: &str) -> Parameter {
    Parameter {
        name: name.into(),
        kind: kind_for(name, unit),
        quantity: Quantity::setpoint(v, unit),
    }
}
