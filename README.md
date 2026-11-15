# openwafer-genealogy

The open genealogy schema for semiconductor fabs, with its validator, import adapters and CLI (`owg`).

One record format joins a fab's **recipes, process runs, tool traces, metrology and electrical results** through the material genealogy (lot → wafer or substrate → chip → die → test site, with splits, merges and rework). Several sites' data can then be pooled and trained on.

- **Units on every quantity**, from a closed table. One parameter has one unit per dataset.
- **Uncertainty on every measured value**, or an explicit `unknown` with a reason.
- **Provenance and licence on every record.** A record without them is refused. Records marked `NOASSERTION` never enter a redistributable export.
- **MEASURED, SPECIFIED and SIMULATED never mix silently.** A real record cannot link to a simulated one, and measured-only queries refuse simulated input.
- JSON Lines for interchange. CSV with a SHA-256 manifest, or Parquet (`--features parquet`), for training.

The specification is [SCHEMA.md](SCHEMA.md), and the JSON Schema is [schema/owg-0.1.0.schema.json](schema/owg-0.1.0.schema.json).

## Use

```sh
cargo build --release --features parquet   # drop --features parquet for CSV only
alias owg=target/release/owg
owg validate myfab.jsonl          # errors and warnings, by record and field
owg lineage myfab.jsonl --recipe myfab/recipe/al-sputter-v1
owg export myfab.jsonl --format csv --out myfab-csv --redistributable-only
owg export myfab.jsonl --format parquet --out myfab-parquet
```

Adapters read: Hacker Fab's `data-management` dump and portal CSV, `smu.py` sweeps, the impedance matcher's telemetry, MKS 901P logs, the tube-furnace recipe format, travelers (.xlsx) and report tables, plus a generic run sheet. [CONTRIBUTING.md](CONTRIBUTING.md) shows how to turn a site's logs into validated records.

## Data in this repository

| Directory | What it is | Licence |
|---|---|---|
| `hackerfab/dataset/` | 176 records converted from Hacker Fab's public documentation: 63 recipes, 12 runs, 10 metrology records, 1 electrical test, 7 yields, 1 trace. 10 chains run recipe → run → metrology, and 1 runs recipe → run → electrical. | Per record: CC BY-SA 4.0 or CERN-OHL-S-2.0 ([sources](hackerfab/SOURCES.md)) |
| `examples/simulated-cell/` | 1,042 SIMULATED records from a closed-loop etch-cell simulator. Every table is filled, including traces, virtual metrology with refusals, bin maps, and the simulator's ground truth. | Apache-2.0 |

No public dataset yet joins recipe, tool trace, metrology and electrical result on the same material. Contributions of real runs are welcome, under a licence the contributor chooses.

## Licence

The code and schema are Apache-2.0 ([LICENSE](LICENSE)). Third-party data keeps its own licence ([NOTICE](NOTICE)).
