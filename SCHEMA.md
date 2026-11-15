# openWafer genealogy schema, version 0.1.0

A record format that joins a fab's recipes, tool traces, metrology and electrical results through one material genealogy, so that data from several sites can be pooled and trained on.

- Rust types: `src/record.rs` (the source of truth).
- JSON Schema: [`schema/owg-0.1.0.schema.json`](schema/owg-0.1.0.schema.json), generated from the types. A test fails if it drifts.
- Validator: `src/validate.rs`. CLI: `owg validate`.
- Contributor guide: [CONTRIBUTING.md](CONTRIBUTING.md).

## Versioning

`header.schema_version` is a semantic version. A minor bump adds optional fields or tables. A major bump changes the meaning of an existing field. The validator accepts any dataset with the same major version.

## Rules every record follows

1. **Provenance and licence on every record.** `provenance` names the origin, source (URL or path), exact source version (commit or file hash), locator inside the source, and the program that produced the record. `license.spdx` is an SPDX expression. A record without either is refused. `NOASSERTION` is allowed and marks the record as not redistributable: `export --redistributable-only` drops it.
2. **Origin.** Every record is `MEASURED` (an instrument read real material), `SPECIFIED` (a person wrote it down as an intention: a recipe, a traveler) or `SIMULATED` (a model produced it). The dataset header declares `MEASURED`, `SIMULATED` or `MIXED`.
   - A `MEASURED` dataset holds no `SIMULATED` record, and a `SIMULATED` dataset holds no `MEASURED` record.
   - A `MEASURED` or `SPECIFIED` record never links to a `SIMULATED` one, whatever the header says: real material cannot have been processed by a simulated run.
   - A `SIMULATED` record may describe real material (a virtual-metrology prediction for a measured wafer) only in a `MIXED` dataset. Queries label every result with the origins it touched, and a measured-only query fails if it reaches a simulated record.
3. **Units.** Every number is a `Quantity` with a `unit` from the unit table (SI and the units fabs use: `nm`, `um`, `angstrom`, `Torr`, `mTorr`, `hPa`, `sccm`, `rpm`, `degC`, `ohm/sq`, `ohm.m`, `cm-2`, ...). `C` is the coulomb; Celsius is `degC`. A declared `kind` (length, pressure, sheet resistance, ...) fixes the dimension the unit must have.
4. **One parameter, one unit.** Within a dataset, every value of one parameter uses the same unit spelling: a thickness in `nm` on one wafer and in `um` on the next is an error, even though both are lengths, because a training column that mixes them is wrong by 1000×. Convert at import.
5. **Uncertainty.** A measured value carries `standard` (k = 1) or `expanded` uncertainty with its basis, or `unknown` with the reason. A recipe parameter or as-run setting is a `setpoint`, optionally with a tolerance. A measurement may not be marked as a setpoint, nor a setpoint as a measurement.
6. **Methods.** A derived value carries a `derivation` with its method and inputs. Extracted electrical parameters and model estimates are always derived. A population statistic (`Summary`) keeps its spread (`sd`) apart from any uncertainty, and names how its members were obtained.
7. **People.** Runs record an operator *role* ("student", "technician", "automation"), never a name. Adapters drop user ids.

## Tables

| Table | One record is | Links |
|---|---|---|
| `units` | a lot, wafer, substrate (a chip, slide or coupon that is not a wafer), chip, die or test site | `parent` |
| `events` | a change to the hierarchy: `create`, `split` (cleave, lot split), `merge`, `rework` (with the run redone), `scrap` | `inputs`, `outputs`, `run` |
| `recipes` | a versioned recipe; `parameters` are setpoints with units, `attributes` are text (chemicals, resist names) | `supersedes` |
| `runs` | one execution of a step on a tool: tool, chamber, route, step, material, start and end (RFC 3339), operator role, status, as-run `settings`, tool-reported `actuals` | `recipe`, `material` |
| `traces` | one sensor's time series in one run: unit, `sample_rate_hz` or explicit `times_s`, `t0`, values (`null` is a missing sample), uncertainty, `calibration` (status, date, reference), per-sample flags | `run` |
| `metrology` | one parameter measured on one unit at one or more sites: method, tool, site positions, values with uncertainty, conditions (for example the refractive index a reflectometer assumed) | `material`, `after_run` |
| `electrical` | one test of one structure: geometry, instrument, bias per terminal (fixed or swept), raw curves, extracted parameters with their extraction method | `material` |
| `summaries` | a published population statistic without its members: n, mean, sd, min, median, max | `scope`, `members` |
| `yields` | a pass count at one stage with its criterion | `scope` |
| `defects` | a defect or a count of one class: method, position, size | `material`, `after_run` |
| `bin_maps` | a sort result: bin definitions and one bin per die | `material`, `after_run`, `dies[].die` |
| `observations` | a qualitative result (inspection grade, yes/no) with its scale. Never coerced into a number | `material`, `after_run` |
| `estimates` | a model estimate about some material (defect-density posterior, ground truth in a simulation) | `scope` |

Every field, with its type, is in the JSON Schema. Field documentation is in `record.rs`.

### The join

A run processed some units. Everything below them in the hierarchy, and everything a split or merge made from them, inherits the run. So an electrical test on a test site joins every run that touched its die, its chip and its wafer:

```
recipe ─▶ run ─▶ trace
             └─▶ material ─▶ (children, split and merge outputs) ─▶ metrology, electrical, bin map, yield
```

`owg lineage <file> --recipe <id>` walks it from a recipe and its versions. `owg history <file> --unit <id>` walks it back from one unit.

## Physics checks (warnings)

- Values physics forbids: a negative absolute temperature, pressure, resistance, gas flow, power, time or defect density; a VSWR below 1 in an unflagged trace sample.
- Relations that do not close, with the unit factor the mismatch matches:
  - deposited thickness against deposition rate × time in one recipe or run;
  - a measured value against its recipe target, when the ratio is near 10, 1000 or another unit factor.

## File forms

**JSON Lines** (interchange). The first line is `{"table": "header", "record": {...}}`. Then each line is `{"table": "<table>", "record": {...}}`. Lines can be appended and concatenated.

**Columnar** (training): `owg export --format csv|parquet`.

- One file per table, plus one per nested list, named `<table>.<path>`. A child table carries `_parent` (the parent's `_row`) and `_idx`. The parent keeps the list length in `<path>#len`.
- Nested objects become dotted columns: `value.unit`, `value.uncertainty.u`.
- A trace's samples are rows of `traces.values`.
- `manifest.json` lists every file's columns and types, row count and SHA-256, and the distinct units in each `<x>.value` column.
- In CSV every string is quoted and a missing value is an empty unquoted field, so an empty string and a missing value stay different.
- Parquet needs the `parquet` feature (`cargo build --features parquet`).

All three forms read back into identical Rust values. A test checks this, including a negative control: one dropped column must make the round trip fail.

## Worked examples

- [`examples/simulated-cell/`](examples/simulated-cell/): every record is SIMULATED. It is the closed etch-cell simulator in openWafer's fab stack, exported with 20 lots and seed 20261106:
  - 100 wafers in 20 lots, one recipe with its run-to-run setpoints, and 100 runs;
  - 400 raw sensor traces (OES, DC bias, pressure, chuck temperature at 1 Hz);
  - 26 CD-SEM readings and 17 virtual-metrology predictions, with 57 refused or dropped predictions recorded as observations;
  - 100 bin maps and sort yields;
  - the defect-density posterior, and each wafer's true CD from the simulator, labelled as ground truth that a fab cannot observe.
- [`hackerfab/dataset/`](hackerfab/dataset/): real Hacker Fab records converted from their public files. [hackerfab/SOURCES.md](hackerfab/SOURCES.md) lists what exists, its licences, and what joins. [hackerfab/build.sh](hackerfab/build.sh) rebuilds it.

## Relation to SEMI standards

These standards are paywalled. The mapping below uses only public descriptions of them. The schema does not claim conformance to either standard, and it does not implement their XML or SECS-II encodings.

| Public description | Where it maps here |
|---|---|
| SEMI E142 substrate mapping covers wafers, frames, strips and trays, with an XML schema (E142.1) and a SECS-II protocol (E142.2) ([SEMI store](https://store-us.semi.org/products/e14200-semi-e142-specification-for-substrate-mapping)) | `units` with `kind` wafer, or substrate for frames, strips, trays and cleaved pieces |
| E142 maps hold "layout, bin codes, device IDs, transfer data, and device data maps", with configurable origin points and axis directions ([PDF Solutions](https://www.pdf.com/standards/semi-e142-specification-for-substrate-mapping/)) | layout: `MapPosition` (`x_index`, `y_index`, `x`, `y`, `origin`). Bin codes: `bin_maps`. Device ids: die and test-site `units`. Transfer data: `events` (split and merge). Device data maps: `metrology.points[].position` |
| Origin locations LowerLeft, LowerRight, UpperLeft, UpperRight and Center, from one MES vendor's E142 implementation ([Critical Manufacturing](https://help.criticalmanufacturing.com/userguide/business-data/substrate-map/cm_semi_e142_schema.md)) | `MapPosition.origin` is free text. Using those spellings is recommended |
| SEMI E164 defines common EDA (Interface A) metadata on top of E125 equipment self-description and the E120 common equipment model. It maps variables, events and alarms from content standards such as E30 and E40 ([SEMI store](https://store-us.semi.org/products/e16400-semi-e164-specification-for-eda-common-metadata); [Cimetrix](https://www.cimetrix.com/semi-e164)) | `traces.tool` and `traces.sensor` are flat identifiers. A site whose equipment publishes E120/E125 metadata can put the equipment-model path in `sensor`. `runs` corresponds loosely to an E40 process job. Nothing here implements E125 or E164 |
