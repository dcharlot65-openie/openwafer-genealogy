# Contributing fab records

This guide turns a site's logs, spreadsheets and tool files into validated records in the [genealogy schema](SCHEMA.md). Everything runs locally. Nothing is uploaded anywhere unless you publish the files yourself.

## Build the tool

```sh
cargo build --release --features parquet   # drop --features parquet for CSV only
alias owg=target/release/owg
```

## Choose a licence first

Every record carries the licence you give it. A record without a licence is refused.

| You want | SPDX id |
|---|---|
| anyone may use the data with no conditions | `CC0-1.0` |
| anyone may use it, with credit | `CC-BY-4.0` |
| credit, and derived datasets keep the same licence | `CC-BY-SA-4.0` |
| keep it private for now | `NOASSERTION`: the records validate, and `export --redistributable-only` drops them |

## Formats you can import today

| Your file | Command | Format defined by |
|---|---|---|
| data-management database dump (`python manage.py dumpdata data_management > dump.json`) | `import dm-dumpdata dump.json` | `hacker-fab/data-management` models |
| data-management portal search download (`webscraping_dataset.csv`) | `import dm-search-csv webscraping_dataset.csv` | `views.py` CSV export |
| `smu.py` output `{chip}_{device}_currents.csv` + `_voltages.csv` | `import smu-pair cur.csv volt.csv --test-id … --material … --r-drain 100` | `hacker-fab/smu` |
| SMU sweep export with `_Id,_Iteration,_Segment,_Elapsed,_DateTime,<name> (<unit>)` columns (four-point probe) | `import smu-sweep file.csv --test-id … --material … [--thickness "150 nm"]` | the four-point-probe file in the GitBook |
| impedance-matcher log (`data/csv/*.csv` from `live.py`) | `import impedance file.csv --run-id … --tool …` | `hacker-fab/impedance-matcher` `telemetry.py` |
| MKS 901P log (`output.csv` from `901p-serial/main.py`) | `import p901 output.csv --run-id … --tool … --unit Torr --unit-basis "…"` | `hacker-fab/uw-p901-reader` |
| tube-furnace recipe JSON | `import furnace-recipe recipe.json` | `hacker-fab/uw-tube-furnace_pid` `recipe.py` |
| process traveler (.xlsx or .csv: Step #, Step Name, Tool Name, Parameters as `Key: value unit` lines, Comments) | `import traveler flow.xlsx --origin specified --route "…"` | the Sputtered Gate NMOS S26 traveler |
| characterization report tables (`<name> ± σ (<unit>)` headers, `N_functional = … N_probed = …` lines) | `import report-md report.md --group-unit … --chip "Chip 19=…"` | the March 2026 NMOS reports |
| per-device extraction CSV (`nmos_summary.csv`) | `import nmos-summary nmos_summary.csv [per_gate.csv] --chip 19=…` | the group 1 analysis script |
| **anything else**: a run sheet (below) | `import run-sheet sheet.csv` | [templates/run-sheet-example.csv](templates/run-sheet-example.csv) |

Every import takes the provenance and licence of its source:

```sh
owg import smu-sweep 4pp.csv \
  --url "https://example.org/my-fab/4pp.csv" --commit <git commit or sha256> --retrieved 2026-11-15 \
  --license CC-BY-4.0 --attribution "My Fab" --site "My Fab" \
  --test-id myfab/run12/4pp --material myfab/chip-12 --thickness "150 nm" \
  --merge-into myfab.jsonl --out myfab.jsonl
```

`--merge-into` adds the new records to an existing dataset, so links to units and runs defined earlier resolve. The import writes nothing if the result has an error. Use `--drop-invalid` to keep the valid records and list the refused ones.

## The run sheet: for logs kept in a spreadsheet

One row per run and its results. Units live in the column headers, so a column cannot change unit halfway down. Copy [templates/run-sheet-example.csv](templates/run-sheet-example.csv), keep the columns you need, and export as CSV.

| Column | Meaning |
|---|---|
| `unit`, `unit_kind`, `parent`, `substrate`, `site` | the piece of material (`chip` by default), its parent, its starting material |
| `recipe`, `recipe_name`, `recipe_version`, `step_kind`, `tool_class` | the recipe the run followed. Change `recipe` and `recipe_version` when a setpoint changes |
| `run`, `tool`, `step`, `start`, `end`, `operator_role`, `status` | the run. Times are RFC 3339 with an offset (`2026-01-15T10:00:00-05:00`). A role, never a name |
| `set:<name> [<unit>]` | a setpoint, for example `set:rf_power [W]` |
| `meas:<p> [<unit>]` | a result on `unit` after `run`, for example `meas:film_thickness [nm]` |
| `u:<p>`, `u_basis:<p>` | its standard uncertainty and how you got it ("s.d. of 5 repeats / √5") |
| `u_reason:<p>` | if you have no uncertainty, say why. A blank uncertainty with no reason is refused |
| `method:<p>`, `mtool:<p>`, `cond:<p>:<name> [<unit>]` | how it was measured, on what, and under which conditions |
| `rattr:<name>`, `attr:<name>` | text about the recipe or the run (chemicals, notes) |
| `source`, `source_version`, `locator`, `license`, `attribution`, `origin`, `note` | provenance and licence of the row |

[hackerfab/transcribed/film-characterization.csv](hackerfab/transcribed/film-characterization.csv) is a real example: the Hacker Fab film-characterization page transcribed into 11 rows.

## Check, then publish

```sh
owg validate myfab.jsonl          # errors and warnings, by record and field
owg lineage myfab.jsonl --recipe myfab/recipe/al-sputter-v1
owg export myfab.jsonl --format csv --out myfab-csv --redistributable-only
owg export myfab.jsonl --format parquet --out myfab-parquet
```

What the validator refuses, and how to fix it:

| Code | Fix |
|---|---|
| `missing_provenance`, `missing_license` | pass `--url`/`--license`, or fill the `source` and `license` columns |
| `missing_unit`, `unknown_unit`, `unit_dimension` | put the unit in the header (`[nm]`). Use a unit from the table (`degC`, not `C`) |
| `unit_decl_mismatch` | the same parameter appears in two units in one dataset: convert one at import |
| `missing_uncertainty` | give `u:` or `u_reason:` |
| `missing_method` | name the extraction or derivation method |
| `broken_link` | the unit, run or recipe a record names is not in the dataset. Import it first, or use `--merge-into` |
| `simulated_in_measured_join`, `origin_mix` | simulated and measured records are mixed. Keep simulations in their own dataset |
| `missing_sample_rate`, `missing_calibration` | give a trace its sample rate (or times) and its sensor's calibration date. State `unknown` if it is unknown |

Warnings do not block an import. `scale_suspect` means a relation such as thickness = rate × time does not close, and it names the unit factor the mismatch matches. Check that unit label.

## Your data stays yours

- The tool runs on your machine. It sends nothing anywhere.
- You choose the licence of every record, and `NOASSERTION` keeps records out of any redistributable export.
- Each record keeps your source URL, commit and attribution. Anyone who uses it can see where it came from.
- Records never carry personal names. Adapters drop user ids, and runs carry roles.
