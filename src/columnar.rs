//! Columnar export for training: one flat table per record type, one more per nested list.
//!
//! The flattening is mechanical and lossless, so the schema stays the single source of truth:
//!
//! * nested objects become dotted columns (`provenance.origin`, `quantity.uncertainty.u`);
//! * a list becomes a child table named `<table>.<path>` with `_parent` (the parent's `_row`)
//!   and `_idx`, and the parent keeps the list length in `<path>#len` so an empty list and an
//!   absent one stay different;
//! * a list of numbers (trace samples) becomes a child table with one `value` column.
//!
//! So `metrology.points` holds one row per measured site with `value.value`, `value.unit` and
//! `value.uncertainty.u` side by side, and `traces.values` holds one row per sample. The
//! validator guarantees one unit per parameter, and the manifest lists the units found in
//! each value column, so a trainer can check it without parsing anything.
//!
//! Two encodings share this layout: CSV with a JSON manifest (always available) and Parquet
//! (feature `parquet`). Both read back into the same [`Dataset`].

use super::record::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DType {
    Null,
    Bool,
    Int,
    Float,
    Str,
}

impl DType {
    fn join(self, c: &Cell) -> DType {
        use DType::*;
        let other = match c {
            Cell::Null => return self,
            Cell::Bool(_) => Bool,
            Cell::Int(_) => Int,
            Cell::Float(_) => Float,
            Cell::Str(_) => Str,
        };
        match (self, other) {
            (Null, o) => o,
            (a, b) if a == b => a,
            (Int, Float) | (Float, Int) => Float,
            _ => Str,
        }
    }
}

/// One flat table.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn dtypes(&self) -> Vec<DType> {
        (0..self.columns.len())
            .map(|j| self.rows.iter().fold(DType::Null, |d, r| d.join(&r[j])))
            .collect()
    }
    fn col(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name)
    }
}

struct Builder {
    columns: Vec<String>,
    index: BTreeMap<String, usize>,
    rows: Vec<BTreeMap<usize, Cell>>,
}

impl Builder {
    fn new() -> Builder {
        Builder {
            columns: vec![],
            index: BTreeMap::new(),
            rows: vec![],
        }
    }
    fn col(&mut self, name: &str) -> usize {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        self.columns.push(name.to_string());
        self.index.insert(name.to_string(), self.columns.len() - 1);
        self.columns.len() - 1
    }
}

fn cell(v: &Value) -> Cell {
    match v {
        Value::Null => Cell::Null,
        Value::Bool(b) => Cell::Bool(*b),
        Value::Number(n) => {
            if n.is_f64() {
                Cell::Float(n.as_f64().unwrap_or(f64::NAN))
            } else if let Some(i) = n.as_i64() {
                Cell::Int(i)
            } else {
                Cell::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => Cell::Str(s.clone()),
        _ => unreachable!("only scalars become cells"),
    }
}

struct Flattener {
    tables: BTreeMap<String, Builder>,
    order: Vec<String>,
}

impl Flattener {
    fn table(&mut self, name: &str) -> &mut Builder {
        if !self.tables.contains_key(name) {
            self.tables.insert(name.to_string(), Builder::new());
            self.order.push(name.to_string());
        }
        self.tables.get_mut(name).unwrap()
    }

    /// Start a row in `table` (with `_parent`/`_idx` for child rows) and return its `_row`.
    fn row(&mut self, table: &str, parent: Option<(i64, i64)>) -> (usize, i64) {
        let b = self.table(table);
        let r = b.rows.len();
        b.rows.push(BTreeMap::new());
        let c = b.col("_row");
        b.rows[r].insert(c, Cell::Int(r as i64));
        if let Some((p, i)) = parent {
            let cp = b.col("_parent");
            b.rows[r].insert(cp, Cell::Int(p));
            let ci = b.col("_idx");
            b.rows[r].insert(ci, Cell::Int(i));
        }
        (r, r as i64)
    }

    fn set(&mut self, table: &str, r: usize, col: &str, c: Cell) {
        let b = self.table(table);
        let j = b.col(col);
        b.rows[r].insert(j, c);
    }

    /// Flatten `obj` into row `r` of `table`, with column prefix `prefix`. Returns whether any
    /// non-null leaf or list was written (so an all-null object can be marked present).
    fn object(
        &mut self,
        table: &str,
        r: usize,
        row_id: i64,
        prefix: &str,
        obj: &Map<String, Value>,
    ) -> bool {
        let mut wrote = false;
        for (k, v) in obj {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                Value::Object(o) => {
                    if !self.object(table, r, row_id, &path, o) {
                        self.set(table, r, &format!("{path}#obj"), Cell::Bool(true));
                    }
                    wrote = true;
                }
                Value::Array(a) => {
                    self.set(table, r, &format!("{path}#len"), Cell::Int(a.len() as i64));
                    let child = format!("{table}.{path}");
                    self.table(&child);
                    for (i, e) in a.iter().enumerate() {
                        let (cr, cid) = self.row(&child, Some((row_id, i as i64)));
                        match e {
                            Value::Object(o) => {
                                self.object(&child, cr, cid, "", o);
                            }
                            Value::Array(_) => {
                                let mut m = Map::new();
                                m.insert("value".into(), e.clone());
                                self.object(&child, cr, cid, "", &m);
                            }
                            s => self.set(&child, cr, "value", cell(s)),
                        }
                    }
                    wrote = true;
                }
                Value::Null => {}
                s => {
                    self.set(table, r, &path, cell(s));
                    wrote = true;
                }
            }
        }
        wrote
    }
}

fn table_values(ds: &Dataset) -> Vec<(&'static str, Vec<Value>)> {
    fn vals<T: Serialize>(v: &[T]) -> Vec<Value> {
        v.iter()
            .map(|r| serde_json::to_value(r).expect("records serialize"))
            .collect()
    }
    vec![
        (
            "header",
            vec![serde_json::to_value(&ds.header).expect("header serializes")],
        ),
        ("units", vals(&ds.units)),
        ("events", vals(&ds.events)),
        ("recipes", vals(&ds.recipes)),
        ("runs", vals(&ds.runs)),
        ("traces", vals(&ds.traces)),
        ("metrology", vals(&ds.metrology)),
        ("electrical", vals(&ds.electrical)),
        ("summaries", vals(&ds.summaries)),
        ("yields", vals(&ds.yields)),
        ("defects", vals(&ds.defects)),
        ("bin_maps", vals(&ds.bin_maps)),
        ("observations", vals(&ds.observations)),
        ("estimates", vals(&ds.estimates)),
    ]
}

/// Flatten a dataset into tables. Tables with no rows are still listed (with no columns).
pub fn flatten(ds: &Dataset) -> Vec<Table> {
    let mut f = Flattener {
        tables: BTreeMap::new(),
        order: vec![],
    };
    for (name, rows) in table_values(ds) {
        f.table(name);
        for v in &rows {
            let (r, id) = f.row(name, None);
            if let Value::Object(o) = v {
                f.object(name, r, id, "", o);
            }
        }
    }
    let Flattener { mut tables, order } = f;
    order
        .into_iter()
        .map(|name| {
            let b = tables.remove(&name).unwrap();
            let n = b.columns.len();
            let rows = b
                .rows
                .into_iter()
                .map(|mut m| (0..n).map(|j| m.remove(&j).unwrap_or(Cell::Null)).collect())
                .collect();
            Table {
                name,
                columns: b.columns,
                rows,
            }
        })
        .collect()
}

fn to_value(c: &Cell) -> Value {
    match c {
        Cell::Null => Value::Null,
        Cell::Bool(b) => Value::Bool(*b),
        Cell::Int(i) => Value::from(*i),
        Cell::Float(x) => serde_json::Number::from_f64(*x)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Cell::Str(s) => Value::String(s.clone()),
    }
}

fn insert_path(obj: &mut Map<String, Value>, path: &str, v: Value) {
    let mut parts = path.split('.').peekable();
    let mut cur = obj;
    while let Some(p) = parts.next() {
        if parts.peek().is_none() {
            cur.insert(p.to_string(), v);
            return;
        }
        let e = cur
            .entry(p.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        cur = match e {
            Value::Object(m) => m,
            _ => return,
        };
    }
}

struct Index<'a> {
    tables: BTreeMap<&'a str, &'a Table>,
    /// child table → parent `_row` → row indices sorted by `_idx`.
    children: BTreeMap<&'a str, BTreeMap<i64, Vec<usize>>>,
}

fn as_i64(c: &Cell) -> Option<i64> {
    match c {
        Cell::Int(i) => Some(*i),
        Cell::Float(x) if x.fract() == 0.0 => Some(*x as i64),
        _ => None,
    }
}

impl<'a> Index<'a> {
    fn new(tables: &'a [Table]) -> Index<'a> {
        let mut children = BTreeMap::new();
        for t in tables {
            if let (Some(p), Some(i)) = (t.col("_parent"), t.col("_idx")) {
                let mut m: BTreeMap<i64, Vec<(i64, usize)>> = BTreeMap::new();
                for (k, r) in t.rows.iter().enumerate() {
                    if let (Some(pp), Some(ii)) = (as_i64(&r[p]), as_i64(&r[i])) {
                        m.entry(pp).or_default().push((ii, k));
                    }
                }
                let m = m
                    .into_iter()
                    .map(|(k, mut v)| {
                        v.sort();
                        (k, v.into_iter().map(|x| x.1).collect())
                    })
                    .collect();
                children.insert(t.name.as_str(), m);
            }
        }
        Index {
            tables: tables.iter().map(|t| (t.name.as_str(), t)).collect(),
            children,
        }
    }

    fn row(&self, table: &str, k: usize) -> Result<Value, String> {
        let t = self
            .tables
            .get(table)
            .ok_or_else(|| format!("missing table {table}"))?;
        let row = &t.rows[k];
        let row_id = t
            .col("_row")
            .and_then(|c| as_i64(&row[c]))
            .unwrap_or(k as i64);
        // A scalar list's element, or a nested list's.
        if t.col("_parent").is_some() {
            let own: Vec<&String> = t.columns.iter().filter(|c| !c.starts_with('_')).collect();
            if own.iter().all(|c| *c == "value") {
                return Ok(t
                    .col("value")
                    .map(|j| to_value(&row[j]))
                    .unwrap_or(Value::Null));
            }
            if own.iter().all(|c| *c == "value#len") {
                let inner = self.row_object(table, t, row, row_id)?;
                return Ok(inner.get("value").cloned().unwrap_or(Value::Array(vec![])));
            }
        }
        Ok(Value::Object(self.row_object(table, t, row, row_id)?))
    }

    fn row_object(
        &self,
        table: &str,
        t: &Table,
        row: &[Cell],
        row_id: i64,
    ) -> Result<Map<String, Value>, String> {
        let mut obj = Map::new();
        for (j, c) in t.columns.iter().enumerate() {
            if c.starts_with('_') {
                continue;
            }
            if let Some(path) = c.strip_suffix("#len") {
                let Some(n) = as_i64(&row[j]) else { continue };
                let child = format!("{table}.{path}");
                let mut items = Vec::new();
                if n > 0 {
                    let rows = self
                        .children
                        .get(child.as_str())
                        .and_then(|m| m.get(&row_id))
                        .cloned()
                        .unwrap_or_default();
                    if rows.len() as i64 != n {
                        return Err(format!("{table} row {row_id}: {path} should have {n} items, child table has {}", rows.len()));
                    }
                    for ck in rows {
                        items.push(self.row(&child, ck)?);
                    }
                }
                insert_path(&mut obj, path, Value::Array(items));
            } else if let Some(path) = c.strip_suffix("#obj") {
                if row[j] == Cell::Bool(true) {
                    insert_path(&mut obj, path, Value::Object(Map::new()));
                }
            } else if row[j] != Cell::Null {
                insert_path(&mut obj, c, to_value(&row[j]));
            }
        }
        Ok(obj)
    }

    fn top(&self, table: &str) -> Result<Vec<Value>, String> {
        let Some(t) = self.tables.get(table) else {
            return Ok(vec![]);
        };
        (0..t.rows.len()).map(|k| self.row(table, k)).collect()
    }
}

/// Rebuild a dataset from its tables.
pub fn unflatten(tables: &[Table]) -> Result<Dataset, String> {
    let ix = Index::new(tables);
    let header = ix
        .top("header")?
        .into_iter()
        .next()
        .ok_or("no header table")?;
    let mut ds = Dataset::new(serde_json::from_value(header).map_err(|e| format!("header: {e}"))?);
    macro_rules! load {
        ($f:ident) => {
            for v in ix.top(stringify!($f))? {
                ds.$f.push(
                    serde_json::from_value(v).map_err(|e| format!("{}: {e}", stringify!($f)))?,
                );
            }
        };
    }
    load!(units);
    load!(events);
    load!(recipes);
    load!(runs);
    load!(traces);
    load!(metrology);
    load!(electrical);
    load!(summaries);
    load!(yields);
    load!(defects);
    load!(bin_maps);
    load!(observations);
    load!(estimates);
    Ok(ds)
}

// ───────────────────────────── CSV with a manifest ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub dtype: DType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub file: String,
    pub rows: usize,
    pub sha256: String,
    pub columns: Vec<ColumnInfo>,
    /// For each `<x>.value` column with a sibling `<x>.unit`, the distinct units found.
    #[serde(default)]
    pub units: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub schema_version: String,
    pub dataset_id: String,
    pub format: String,
    /// How a missing value is written.
    pub null: String,
    pub tables: Vec<TableInfo>,
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn csv_field(c: &Cell) -> String {
    match c {
        Cell::Null => String::new(),
        Cell::Bool(b) => b.to_string(),
        Cell::Int(i) => i.to_string(),
        Cell::Float(x) => format!("{x:?}"),
        Cell::Str(s) => format!("\"{}\"", s.replace('"', "\"\"")),
    }
}

/// The CSV text of one table: header row of column names, every string quoted, a missing
/// value as an empty unquoted field.
pub fn table_csv(t: &Table) -> String {
    let mut s = String::new();
    s.push_str(
        &t.columns
            .iter()
            .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(","),
    );
    s.push('\n');
    for r in &t.rows {
        s.push_str(&r.iter().map(csv_field).collect::<Vec<_>>().join(","));
        s.push('\n');
    }
    s
}

/// Split CSV text into records of (field, was-quoted).
pub fn parse_csv(text: &str) -> Result<Vec<Vec<(String, bool)>>, String> {
    let mut out = Vec::new();
    let mut rec: Vec<(String, bool)> = Vec::new();
    let mut f = String::new();
    let mut quoted = false;
    let mut in_q = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(ch) = chars.next() {
        any = true;
        if in_q {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    f.push('"');
                    chars.next();
                } else {
                    in_q = false;
                }
            } else {
                f.push(ch);
            }
            continue;
        }
        match ch {
            '"' if f.is_empty() && !quoted => {
                in_q = true;
                quoted = true;
            }
            ',' => {
                rec.push((std::mem::take(&mut f), quoted));
                quoted = false;
            }
            '\r' => {}
            '\n' => {
                rec.push((std::mem::take(&mut f), quoted));
                quoted = false;
                out.push(std::mem::take(&mut rec));
                any = false;
            }
            c => f.push(c),
        }
    }
    if in_q {
        return Err("unterminated quoted field".into());
    }
    if any {
        rec.push((f, quoted));
        out.push(rec);
    }
    Ok(out)
}

fn parse_cell(field: &(String, bool), d: DType) -> Result<Cell, String> {
    let (s, q) = field;
    if !q && s.is_empty() {
        return Ok(Cell::Null);
    }
    Ok(match d {
        DType::Null => Cell::Null,
        DType::Str => Cell::Str(s.clone()),
        DType::Bool => Cell::Bool(s.parse().map_err(|_| format!("'{s}' is not a bool"))?),
        DType::Int => Cell::Int(s.parse().map_err(|_| format!("'{s}' is not an integer"))?),
        DType::Float => Cell::Float(s.parse().map_err(|_| format!("'{s}' is not a number"))?),
    })
}

fn units_of(t: &Table) -> BTreeMap<String, Vec<String>> {
    let distinct = |j: usize| -> Vec<String> {
        let set: BTreeSet<String> = t
            .rows
            .iter()
            .filter_map(|r| {
                if let Cell::Str(s) = &r[j] {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect();
        set.into_iter().collect()
    };
    let mut m = BTreeMap::new();
    for c in &t.columns {
        if let Some(base) = c.strip_suffix(".value") {
            if let Some(j) = t.col(&format!("{base}.unit")) {
                m.insert(c.clone(), distinct(j));
            }
        }
    }
    if let Some(j) = t.col("unit") {
        m.insert("unit".into(), distinct(j));
    }
    m
}

fn manifest_for(
    ds: &Dataset,
    tables: &[Table],
    format: &str,
    ext: &str,
    bytes: &[Vec<u8>],
) -> Manifest {
    Manifest {
        schema: ds.header.schema.clone(),
        schema_version: ds.header.schema_version.clone(),
        dataset_id: ds.header.dataset_id.clone(),
        format: format.into(),
        null: if format == "csv" {
            "an empty unquoted field; every string is quoted".into()
        } else {
            "a null".into()
        },
        tables: tables
            .iter()
            .zip(bytes)
            .map(|(t, b)| TableInfo {
                name: t.name.clone(),
                file: format!("{}.{ext}", t.name),
                rows: t.rows.len(),
                sha256: sha256_hex(b),
                columns: t
                    .columns
                    .iter()
                    .zip(t.dtypes())
                    .map(|(n, d)| ColumnInfo {
                        name: n.clone(),
                        dtype: d,
                    })
                    .collect(),
                units: units_of(t),
            })
            .collect(),
    }
}

/// Write `<dir>/<table>.csv` for every table and `<dir>/manifest.json`.
pub fn write_csv(ds: &Dataset, dir: &Path) -> Result<Manifest, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tables = flatten(ds);
    let bytes: Vec<Vec<u8>> = tables.iter().map(|t| table_csv(t).into_bytes()).collect();
    for (t, b) in tables.iter().zip(&bytes) {
        std::fs::write(dir.join(format!("{}.csv", t.name)), b)
            .map_err(|e| format!("{}: {e}", t.name))?;
    }
    let m = manifest_for(ds, &tables, "csv", "csv", &bytes);
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&m).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    Ok(m)
}

/// Read a CSV export back, checking every file's SHA-256 against the manifest.
pub fn read_csv(dir: &Path) -> Result<Dataset, String> {
    let m: Manifest = serde_json::from_slice(
        &std::fs::read(dir.join("manifest.json")).map_err(|e| format!("manifest: {e}"))?,
    )
    .map_err(|e| format!("manifest: {e}"))?;
    let mut tables = Vec::new();
    for ti in &m.tables {
        let b = std::fs::read(dir.join(&ti.file)).map_err(|e| format!("{}: {e}", ti.file))?;
        if sha256_hex(&b) != ti.sha256 {
            return Err(format!("{}: SHA-256 does not match the manifest", ti.file));
        }
        let text = String::from_utf8(b).map_err(|e| format!("{}: {e}", ti.file))?;
        let recs = parse_csv(&text)?;
        let mut it = recs.into_iter();
        let header: Vec<String> = it
            .next()
            .map(|h| h.into_iter().map(|x| x.0).collect())
            .unwrap_or_default();
        let want: Vec<&str> = ti.columns.iter().map(|c| c.name.as_str()).collect();
        if header.iter().map(String::as_str).collect::<Vec<_>>() != want
            && !(header == [""] && want.is_empty())
        {
            return Err(format!("{}: columns do not match the manifest", ti.file));
        }
        let mut rows = Vec::new();
        for r in it {
            if r.len() != want.len() {
                return Err(format!(
                    "{}: a row has {} fields, expected {}",
                    ti.file,
                    r.len(),
                    want.len()
                ));
            }
            rows.push(
                r.iter()
                    .zip(&ti.columns)
                    .map(|(f, c)| parse_cell(f, c.dtype))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        if rows.len() != ti.rows {
            return Err(format!(
                "{}: {} rows, manifest says {}",
                ti.file,
                rows.len(),
                ti.rows
            ));
        }
        tables.push(Table {
            name: ti.name.clone(),
            columns: want.iter().map(|s| s.to_string()).collect(),
            rows,
        });
    }
    unflatten(&tables)
}

#[cfg(feature = "parquet")]
pub mod parquet_io {
    //! Parquet encoding of the same tables (feature `parquet`).
    use super::*;
    use arrow_array::{
        Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    };
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use parquet::arrow::ArrowWriter;
    use std::sync::Arc;

    fn arrow_type(d: DType) -> DataType {
        match d {
            DType::Bool => DataType::Boolean,
            DType::Int => DataType::Int64,
            DType::Float => DataType::Float64,
            DType::Str | DType::Null => DataType::Utf8,
        }
    }

    fn encode(t: &Table) -> Result<Vec<u8>, String> {
        let types = t.dtypes();
        let fields: Vec<Field> = t
            .columns
            .iter()
            .zip(&types)
            .map(|(n, d)| Field::new(n, arrow_type(*d), true))
            .collect();
        let schema = Arc::new(Schema::new(fields));
        let cols: Vec<ArrayRef> = types
            .iter()
            .enumerate()
            .map(|(j, d)| -> ArrayRef {
                let it = t.rows.iter().map(|r| &r[j]);
                match d {
                    DType::Bool => Arc::new(
                        it.map(|c| {
                            if let Cell::Bool(b) = c {
                                Some(*b)
                            } else {
                                None
                            }
                        })
                        .collect::<BooleanArray>(),
                    ),
                    DType::Int => Arc::new(
                        it.map(|c| if let Cell::Int(i) = c { Some(*i) } else { None })
                            .collect::<Int64Array>(),
                    ),
                    DType::Float => Arc::new(
                        it.map(|c| match c {
                            Cell::Float(x) => Some(*x),
                            Cell::Int(i) => Some(*i as f64),
                            _ => None,
                        })
                        .collect::<Float64Array>(),
                    ),
                    DType::Str | DType::Null => Arc::new(
                        it.map(|c| {
                            if let Cell::Str(s) = c {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<StringArray>(),
                    ),
                }
            })
            .collect();
        let mut buf = Vec::new();
        {
            let mut w =
                ArrowWriter::try_new(&mut buf, schema.clone(), None).map_err(|e| e.to_string())?;
            if !t.columns.is_empty() {
                let batch = RecordBatch::try_new(schema, cols).map_err(|e| e.to_string())?;
                w.write(&batch).map_err(|e| e.to_string())?;
            }
            w.close().map_err(|e| e.to_string())?;
        }
        Ok(buf)
    }

    fn decode(name: &str, path: &Path, info: &TableInfo) -> Result<Table, String> {
        let columns: Vec<String> = info.columns.iter().map(|c| c.name.clone()).collect();
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        if columns.is_empty() {
            return Ok(Table {
                name: name.into(),
                columns,
                rows,
            });
        }
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| e.to_string())?
            .build()
            .map_err(|e| e.to_string())?;
        for batch in reader {
            let batch = batch.map_err(|e| e.to_string())?;
            let n = batch.num_rows();
            let mut block: Vec<Vec<Cell>> =
                (0..n).map(|_| Vec::with_capacity(columns.len())).collect();
            for (j, c) in info.columns.iter().enumerate() {
                let a = batch.column(j);
                for (i, row) in block.iter_mut().enumerate() {
                    row.push(if a.is_null(i) {
                        Cell::Null
                    } else {
                        match c.dtype {
                            DType::Bool => Cell::Bool(
                                a.as_any()
                                    .downcast_ref::<BooleanArray>()
                                    .ok_or("bool column")?
                                    .value(i),
                            ),
                            DType::Int => Cell::Int(
                                a.as_any()
                                    .downcast_ref::<Int64Array>()
                                    .ok_or("int column")?
                                    .value(i),
                            ),
                            DType::Float => Cell::Float(
                                a.as_any()
                                    .downcast_ref::<Float64Array>()
                                    .ok_or("float column")?
                                    .value(i),
                            ),
                            DType::Str | DType::Null => Cell::Str(
                                a.as_any()
                                    .downcast_ref::<StringArray>()
                                    .ok_or("string column")?
                                    .value(i)
                                    .to_string(),
                            ),
                        }
                    });
                }
            }
            rows.extend(block);
        }
        Ok(Table {
            name: name.into(),
            columns,
            rows,
        })
    }

    /// Write `<dir>/<table>.parquet` for every table and `<dir>/manifest.json`.
    pub fn write_parquet(ds: &Dataset, dir: &Path) -> Result<Manifest, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tables = flatten(ds);
        let bytes: Vec<Vec<u8>> = tables.iter().map(encode).collect::<Result<_, _>>()?;
        for (t, b) in tables.iter().zip(&bytes) {
            std::fs::write(dir.join(format!("{}.parquet", t.name)), b)
                .map_err(|e| e.to_string())?;
        }
        let m = manifest_for(ds, &tables, "parquet", "parquet", &bytes);
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&m).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        Ok(m)
    }

    /// Read a Parquet export back, checking SHA-256s against the manifest.
    pub fn read_parquet(dir: &Path) -> Result<Dataset, String> {
        let m: Manifest = serde_json::from_slice(
            &std::fs::read(dir.join("manifest.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut tables = Vec::new();
        for ti in &m.tables {
            let b = std::fs::read(dir.join(&ti.file)).map_err(|e| format!("{}: {e}", ti.file))?;
            if sha256_hex(&b) != ti.sha256 {
                return Err(format!("{}: SHA-256 does not match the manifest", ti.file));
            }
            let t = decode(&ti.name, &dir.join(&ti.file), ti)?;
            if t.rows.len() != ti.rows {
                return Err(format!(
                    "{}: {} rows, manifest says {}",
                    ti.file,
                    t.rows.len(),
                    ti.rows
                ));
            }
            tables.push(t);
        }
        unflatten(&tables)
    }
}
