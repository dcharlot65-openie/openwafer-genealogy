#!/usr/bin/env bash
# Rebuild the Hacker Fab dataset in the openWafer genealogy schema.
#
#   build.sh            the redistributable dataset, from the files committed in raw/ and
#                       transcribed/, into dataset/ (JSON Lines + CSV)
#   build.sh --local D  also fetch the sources that are too large to commit (the three full
#                       impedance-matcher traces) or whose licence is not stated (the NMOS
#                       per-device CSVs on Google Drive), verify their SHA-256, and write the
#                       full dataset to D/. Nothing from --local is committed.
#
# Needs the `owg` binary (OWG=path to override) and, for --local, curl.
set -euo pipefail
cd "$(dirname "$0")"
M=${OWG:-owg}
G=6256296bb0531805a0207048ac86141c17ad72d9          # hacker-fab/gitbook commit
P=a1c6426824000ee08fed928dffc36ea01971127a          # hacker-fab/uw-p901-reader commit
GB="https://github.com/hacker-fab/gitbook/blob/$G"
RAW="https://raw.githubusercontent.com/hacker-fab/gitbook/$G"
DOCS="CC-BY-SA-4.0 AND CERN-OHL-S-2.0"
ATTR="Hacker Fab contributors, hacker-fab/gitbook (docs.hackerfab.org)"
DAY=2026-11-15
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

METHODS=(
  --method "Vth=linear extrapolation of Id-Vgs at peak transconductance, Vd_ref = 5 V; Vgs 0-6 V in 1 V steps, Vds 0-10 V (group 1 report section 5.2)"
  --method "Ion/Ioff=Ion / Ioff (group 1 report section 5.2)"
  --method "Ion_Ioff=Ion / Ioff (group 1 report section 5.2)"
  --method "Ion=drain current at Vgs = 6 V, Vds = 5 V (group 1 report section 5.2)"
  --method "Ioff=drain current at Vgs = 0 V, Vds = 5 V (group 1 report section 5.2)"
  --method "Ron=5 V / Ion (group 1 report section 5.2)"
  --method "Roff=5 V / Ioff (group 1 report section 5.2)"
)

# 1. Film characterization page, transcribed into a run sheet (recipes, runs, metrology).
"$M" genealogy import run-sheet transcribed/film-characterization.csv --prefix hf-film \
  --route "Hacker Fab RF-sputtered Al and Al2O3 film characterization (transcribed)" --out "$T/film.jsonl"

# 2. The four-point-probe file joined to the run that made the film.
"$M" genealogy import smu-sweep raw/4pp-150nm-al-glass.csv --merge-into "$T/film.jsonl" --out "$T/film-4pp.jsonl" \
  --url "$GB/.gitbook/assets/4%20point%20probe%20data%20150nm%20thick%20Al.csv" --commit "$G" --retrieved $DAY \
  --license "$DOCS" --attribution "$ATTR" --site CMU \
  --test-id hf-cmu/film/al-150nm-glass/4pp --material hf-cmu/film/al-150nm-glass \
  --thickness "150 nm" --thickness-ref hf-cmu/film/recipe/al-15sccm-4e-3hpa-150nm

# 3. The Sputtered Gate NMOS S26 traveler: recipes only.
"$M" genealogy import traveler raw/Sputtered_Gate_NMOS_S26.xlsx --origin specified \
  --url "$GB/.gitbook/assets/Sputtered_Gate_NMOS_S26.xlsx" --commit "$G" --retrieved $DAY \
  --license "$DOCS" --attribution "$ATTR" --site CMU \
  --prefix hf-cmu/sputtered-gate-nmos-s26 --route "Sputtered Gate NMOS S26" --out "$T/traveler.jsonl"

# 4. March 2026 NMOS reports: yields, and group 1's per-size statistics.
R=nmos-capabilities/current-fab-capabilities/march-2026-update
"$M" genealogy import report-md raw/march-2026-group-1-report.md \
  --url "$GB/$R/group-1-report.md" --commit "$G" --retrieved $DAY --license "$DOCS" --attribution "$ATTR" --site CMU \
  --prefix hf-cmu/s26-g1 --group-unit hf-cmu/s26-g1 \
  --chip "Chip 19=hf-cmu/s26-g1/chip-19" --chip "Chip 50=hf-cmu/s26-g1/chip-50" "${METHODS[@]}" --out "$T/g1.jsonl"
for g in 2 3; do
  "$M" genealogy import report-md raw/march-2026-group-$g-report.md \
    --url "$GB/$R/group-$g-report.md" --commit "$G" --retrieved $DAY --license "$DOCS" --attribution "$ATTR" --site CMU \
    --prefix hf-cmu/s26-g$g --group-unit hf-cmu/s26-g$g "${METHODS[@]}" --out "$T/g$g.jsonl"
done

# 5. MKS 901P pump-down log.
"$M" genealogy import p901 raw/p901-output.csv \
  --url "https://github.com/hacker-fab/uw-p901-reader/blob/$P/901p-serial/output.csv" --commit "$P" --retrieved $DAY \
  --license CERN-OHL-S-2.0 --attribution "Hacker Fab contributors, hacker-fab/uw-p901-reader" \
  --run-id hf-p901/pumpdown-1 --tool "vacuum chamber with MKS 901P transducer" --step "pump-down" \
  --unit Torr --unit-basis "the 901P's factory unit; the first reading, 741, is ambient pressure in Torr (988 hPa) and implausible as hPa or Pa" \
  --out "$T/p901.jsonl"

mkdir -p dataset
"$M" genealogy merge "$T/film-4pp.jsonl" "$T/traveler.jsonl" "$T/g1.jsonl" "$T/g2.jsonl" "$T/g3.jsonl" "$T/p901.jsonl" \
  --id owg-hackerfab-public-$DAY --declared measured \
  --title "Hacker Fab public records in the openWafer genealogy schema" \
  --description "Converted from Hacker Fab's public repositories and documentation (licences per record). Real records; not complete runs: see SOURCES.md for what joins." \
  --out dataset/owg-hackerfab-public.jsonl
rm -rf dataset/csv
"$M" genealogy export dataset/owg-hackerfab-public.jsonl --format csv --out dataset/csv
"$M" genealogy stats dataset/owg-hackerfab-public.jsonl

if [ "${1:-}" != "--local" ]; then exit 0; fi
L=${2:?usage: build.sh --local <dir>}
mkdir -p "$L"
fetch() { # url sha256 out
  [ -f "$3" ] || curl -sSL -A "genealogy-fetch" -o "$3" "$1"
  echo "$2  $3" | sha256sum -c --quiet - || { echo "SHA-256 mismatch: $3" >&2; exit 1; }
}
fetch "$RAW/.gitbook/assets/test1.csv" a343f9143da35cdd262efffeca216d3a6be1fbac6ed64b4d07de37f3c1788e30 "$L/test1.csv"
fetch "$RAW/.gitbook/assets/test2.csv" b1aaf0e94b94b7c355a86e0e13beedfffc5b86d73c2444c6633baf8afd7f548d "$L/test2.csv"
fetch "$RAW/.gitbook/assets/test3.csv" 128d004a4f3b5fbec31197c93514bf496046b4ecb7c268e0371e983d683d183b "$L/test3.csv"
fetch "https://drive.google.com/uc?export=download&id=1MBbaGzsllM3dzdijOpUGj0idGdKoqtuR" aeb20c5e652e35c144258b94da4aae9424543d7cfad4e3d2291144fac962597a "$L/nmos_summary.csv"
fetch "https://drive.google.com/uc?export=download&id=1hWcgOi_VTi-tj84jaKlh_4Ab0Ib9imnd" 26eeb44de242f707c29ef58be3a2300e675a7bf304e6aacb6cc3811a4459dc16 "$L/nmos_summary_per_gate_voltage.csv"

NOTES=("" "first test, manually stopped after 35 minutes" "second test, plasma went out after 16 minutes" "third test, plasma went out after 59 minutes")
STAT=("" completed faulted faulted)
for i in 1 2 3; do
  "$M" genealogy import impedance "$L/test$i.csv" --url "$GB/.gitbook/assets/test$i.csv" --commit "$G" --retrieved $DAY \
    --license "$DOCS" --attribution "$ATTR" --site CMU --run-id hf-cmu/impedance/test$i \
    --tool "CMU RF sputtering chamber with automated impedance matcher" --step "RF plasma, impedance-matching test" \
    --status "${STAT[$i]}" --note "${NOTES[$i]} (fab-toolkit/rf-sputtering/add-ons-and-wip/automated-impedance-matching.md)" \
    --out "$L/impedance-$i.jsonl"
done
# Licence not stated on the Drive files: NOASSERTION, so these records are never redistributed.
"$M" genealogy import nmos-summary "$L/nmos_summary.csv" "$L/nmos_summary_per_gate_voltage.csv" --merge-into "$T/g1.jsonl" \
  --url "https://drive.google.com/file/d/1MBbaGzsllM3dzdijOpUGj0idGdKoqtuR (linked from $GB/$R/group-1-report.md)" \
  --retrieved $DAY --license NOASSERTION --site CMU --prefix hf-cmu/s26-g1 \
  --chip "19=hf-cmu/s26-g1/chip-19" --chip "50=hf-cmu/s26-g1/chip-50" \
  --gate-length 1=5 --gate-length 2=10 --gate-length 3=15 --gate-length 4=20 --gate-length 5=25 --gate-length 6=30 --gate-length 7=30 \
  "${METHODS[@]}" --out "$L/g1-devices.jsonl"
"$M" genealogy merge "$T/film-4pp.jsonl" "$T/traveler.jsonl" "$L/g1-devices.jsonl" "$T/g2.jsonl" "$T/g3.jsonl" "$T/p901.jsonl" \
  "$L/impedance-1.jsonl" "$L/impedance-2.jsonl" "$L/impedance-3.jsonl" \
  --id owg-hackerfab-local-$DAY --declared measured --title "Hacker Fab records, including local-only sources" --out "$L/owg-hackerfab-local.jsonl"
"$M" genealogy stats "$L/owg-hackerfab-local.jsonl"
