# Hacker Fab public sources: inventory, licences, and what joins

Read on 2026-11-15. Hacker Fab's GitHub organization is [github.com/hacker-fab](https://github.com/hacker-fab). Its documentation is the GitBook at [docs.hackerfab.org](https://docs.hackerfab.org), whose source is the `gitbook` repository.

## Summary

**The data that joins is thin. Public Hacker Fab records hold recipes, a few metrology tables and electrical results, but no run joins recipe, trace, metrology and electrical result for the same piece of material.**

| What exists publicly | Records in the schema | Joined to |
|---|---|---|
| One process traveler (Sputtered Gate NMOS S26, 52 steps) | 52 recipes | nothing: it is a plan, with no runs |
| Film-characterization tables (RF-sputtered Al and Al2O3) | 11 units, 11 runs, 11 recipes, 10 metrology records (3 thickness, 7 AFM roughness) | recipe → run → metrology |
| One four-point-probe sweep (150 nm Al on glass, 25 readings) | 1 electrical test (sheet resistance 0.2281 Ω/sq, page 0.229; resistivity 3.42e-8 Ω·m from the stated 150 nm, page 3.44e-8) | recipe → run → electrical result, through the transcribed film run |
| March 2026 NMOS reports, groups 1–3 | 7 yield records, 66 population statistics (Vth, Ion, Ioff, Ron by chip and gate length) | chips only. No run, recipe, trace or metrology for those chips is public |
| Per-device extraction for group 1 (94 functional NMOS, Google Drive, licence not stated) | 94 test sites, 94 electrical tests (local only) | chips only |
| One MKS 901P pump-down log (1,129 readings) | 1 run, 1 trace | no material, no recipe |
| Three impedance-matcher logs from RF-sputter plasma tests (220,118 rows) | 3 runs, 18 traces, 1.32 M samples (local only: 23 MB as JSON Lines) | no material, no recipe |
| The data-management database (db.hackerfab.org) | the adapter exists. No records: the database is private, and the site returned HTTP 502 on 2026-11-15 | — |

So the 2023 plan for a shared database **was built**, contrary to earlier openWafer notes. It is a Django app built in spring 2024 and extended in fall 2024 (`hacker-fab/data-management`, last commit 2025-05-04). It stores per-chip process parameters and results for one site: `UNIVERSITY_CHOICES` lists only CMU. It integrates one tool: the spin coater, through an AWS job queue. Its contents have never been published. Its schema has units in form labels, no uncertainty fields and no trace storage. It stores a person (`chip_owner`) on every row, and the adapter drops that.

### Findings from converting the data

- **Unit errors in the published traveler.** Steps 27, 28 and 45 give "Deposition Rate" in Å/s. With the stated thickness and time, every one of them closes only in nm/min. For example, step 28 states 150 nm in 60 min, which needs 2.5 nm/min, but its rate reads 2.5 Å/s, which would deposit 900 nm. The validator flags all three with the ratio 6.000, the Å/s to nm/min factor. The film-characterization page measured 2.49 nm/min for Al, which agrees with nm/min.
- **Physically impossible VSWR values in the impedance logs.** 16,631 of 220,118 VSWR samples are below 1 (test1: 93, test2: 12,431, test3: 4,107), with values down to −3.0e7. Their `forward_v` and `reverse_v` columns are all `nan`. The firmware in the repository today computes VSWR = (V_f + V_r)/(V_f − V_r), which cannot fall below 1, so the logs come from an earlier firmware. The adapter keeps these samples and flags them.
- **test3 also has a firmware restart and a time step backwards.** `device_millis` drops twice (rows 172 and 2217), and the host clock steps back 5.4 ms at row 117,193. The adapter clamps the time (4 samples) and flags both.
- **The "94% yield" headline is one group's electrical yield on the devices it probed.** It is Group 1's 94 of 100 probed devices, out of 952 attempted. Its visual yield was 900 of 952. Over the three March 2026 groups, electrical yield on probed devices is 210 of 268 (78.4%): 94/100, 48/56 and 68/112.
- **Group 1's per-size tables are internally consistent.** The per-size device counts sum to each chip's functional devices (48 and 46). The count-weighted means reproduce the pooled Vth: 1.254 V against the reported 1.26 V, and 3.432 V against 3.43 V.

## Licences

| Source | Licence | Redistributed here |
|---|---|---|
| `hacker-fab/gitbook` (traveler, four-point-probe file, impedance logs, reports, film page) | The repository LICENSE file is CERN-OHL-S-2.0. The README's licence stack makes documentation CC BY-SA 4.0. Records carry `CC-BY-SA-4.0 AND CERN-OHL-S-2.0`, which meets both | yes: `raw/` excerpts, `transcribed/`, `dataset/`. The impedance logs are redistributable but are 13 MB, so only a 399-row excerpt is committed and `build.sh --local` fetches the rest |
| `hacker-fab/uw-p901-reader` (`output.csv`) | CERN-OHL-S-2.0 | yes: `raw/p901-output.csv` |
| `hacker-fab/data-management` (schema only) | MPL-2.0 | no code copied. The adapter re-implements the field mapping from the model definitions |
| `hacker-fab/smu`, `impedance-matcher`, `uw-tube-furnace_pid` (formats only) | none stated, NOASSERTION, none stated | no code copied. The adapters read the formats these tools write |
| Group 1 per-device CSVs on Google Drive (`nmos_summary.csv`, `nmos_summary_per_gate_voltage.csv`) | not stated. They are linked from a CC BY-SA page but are not part of the repository | **no.** Records carry `NOASSERTION`. `build.sh --local` fetches them and checks their SHA-256 |

Attribution for everything from the GitBook: Hacker Fab contributors, [hacker-fab/gitbook](https://github.com/hacker-fab/gitbook) at commit `6256296bb0531805a0207048ac86141c17ad72d9`. Excerpts and conversions are changed from the originals: rows truncated, values converted to schema records.

## Local-only sources (not committed)

| File | URL | SHA-256 |
|---|---|---|
| test1.csv | `https://raw.githubusercontent.com/hacker-fab/gitbook/6256296b…/.gitbook/assets/test1.csv` | `a343f9143da35cdd262efffeca216d3a6be1fbac6ed64b4d07de37f3c1788e30` |
| test2.csv | …/test2.csv | `b1aaf0e94b94b7c355a86e0e13beedfffc5b86d73c2444c6633baf8afd7f548d` |
| test3.csv | …/test3.csv | `128d004a4f3b5fbec31197c93514bf496046b4ecb7c268e0371e983d683d183b` |
| nmos_summary.csv | `https://drive.google.com/file/d/1MBbaGzsllM3dzdijOpUGj0idGdKoqtuR` | `aeb20c5e652e35c144258b94da4aae9424543d7cfad4e3d2291144fac962597a` |
| nmos_summary_per_gate_voltage.csv | `https://drive.google.com/file/d/1hWcgOi_VTi-tj84jaKlh_4Ab0Ib9imnd` | `26eeb44de242f707c29ef58be3a2300e675a7bf304e6aacb6cc3811a4459dc16` |

`build.sh --local <dir>` fetches all five, verifies the hashes and builds the full dataset: 385 records, 0 errors and 191 warnings (188 `NOASSERTION`, 3 traveler unit factors).

## Not inventoried

- Google Drive files linked from student pages. 6 of the 27 CMU student update pages link about 18 Drive files, whose contents and licences are unknown.
- The ten site pages under `individual-fab-information/` (CMU, Case Western, Georgia Tech, IIT Bombay, Ohio State, UBC, UC Irvine, UIUC, Waterloo, Wrocław). They are short descriptions with no tables or data links.
- Papers. The stepper paper ([arXiv 2510.15082](https://arxiv.org/abs/2510.15082)) reports stepper results with no released dataset. The ISCAS 2026 paper is paywalled. No thesis with released data was found.
- The XPS raw-data zip and its analysis notebook on the film page. They are spectra, not run records.

## Repositories (53, of which 5 are forks)

Licences as GitHub reports them: 9 CERN-OHL-S-2.0, 6 MIT, 1 MPL-2.0, 7 NOASSERTION, and 30 with no licence file. A repository with no licence grants no right to redistribute its contents, and none of those 30 holds fab data.

| Repository | Licence (GitHub) | Commit read | Contents |
|---|---|---|---|
| `2D_Crystal_Stacking` | MIT | `11db93bdc9` | Tool hardware, firmware or software. No fab data |
| `2D_Crystal_Stacking_Gitbook` | none | `b13eda79d3` | Tool hardware, firmware or software. No fab data |
| `afm` | none | `15187cda4d` | HW design & firmware for a low-cost Atomic Force Microscope. No fab data |
| `ald` | none | `8055a2eb78` | Repository for atomic layer deposition (ALD) tool. No fab data |
| `amscope-camera` | CERN-OHL-S-2.0 | `65944fb5b3` | Python code to control the AmScope camera. No fab data |
| `auto-scribe` | none | `cc38bc5966` | Code for controlling the automatic scriber that is in development. No fab data |
| `ccallpython-helloworld` | CERN-OHL-S-2.0 | `fae2ee7ddd` | This is a simple c call python code for future process control. No fab data |
| `computer-vision` | MIT | `cc2ad82485` | Vision experiments (images). No process data |
| `data-management` | MPL-2.0 | `ae5436a334` | **Schema, no data.** Django app behind db.hackerfab.org: per-chip process tables (Patterning, PlasmaEtch, AluminumEvaporation, GlassDeposition, KOHEtch, ...) with units in field labels; IV CSV uploads; spin-coater job queue (AWS Lambda + DynamoDB). The database itself is not public (login required; the site returned HTTP 502 on 2026-11-15) |
| `delta-stage-controller` | CERN-OHL-S-2.0 | `c76ff3e427` | Arduino and Pi code for OpenFlexure microscope. No fab data |
| `eda-device-modeling` | none | `a2457ee722` | Work Documents for Device Modeling. No fab data |
| `eda-mask-drc` | none | `b1e4ee9b6c` | Work Documents for Mask Design Programme and Design Rule Check Development. No fab data |
| `elegoo-mars-4-dlp-controller` | NOASSERTION | `019403ca0f` | Fork. Raspberry pi hat for the elegoo mars 4 dlp UV projector. Implements SPI data transfer for the DLPC1438. No fab data |
| `gitbook` | CERN-OHL-S-2.0 | `6256296bb0` | **Data.** Process traveler `Sputtered_Gate_NMOS_S26.xlsx` (52 steps, recipes only); four-point-probe sweep `4 point probe data 150nm thick Al.csv` (25 readings); impedance-matcher logs `test1-3.csv` (220,118 rows); March 2026 NMOS reports (yields, per-size Vth/Ion/Ioff/Ron statistics); film-characterization page (thickness, AFM roughness, XPS, sheet resistance tables); database docs. Licence: repo LICENSE CERN-OHL-S-2.0; README states documentation is CC BY-SA 4.0 |
| `grbl` | NOASSERTION | `1defae6cc6` | Fork. An open source, embedded, high performance g-code-parser and CNC milling controller written in optimized C that will run on a straight Arduino. No fab data |
| `hacker_fab_website` | none | `c3f136b975` | Tool hardware, firmware or software. No fab data |
| `impedance-matcher` | NOASSERTION | `11d9230cb2` | **Format.** `firmware/telemetry.py` defines the log the GitBook test files use. No logs in the repo |
| `kiri-docker` | none | `804a12847e` | Fork. Kiri Docker project to run Kiri Revision Inspector inside a Docker container. No fab data |
| `kiri-github-action` | none | `4cb2b9c1df` | Fork. Run Kiri Revision Inspector for KiCad In GitHub Actions. No fab data |
| `lab-communication` | none | `1e67bac6e8` | Client/server stubs for the lab-machine link (superseded by `data-management/lab_com`). No data |
| `lab-robot` | CERN-OHL-S-2.0 | `1683b5b444` | A lab robot, based on a 3D printer, made to automate various processes in the lab. No fab data |
| `Layout-tool-dev` | none | `27e7b53946` | Source code for Layout design using phidl package. No fab data |
| `MicroManipulatorStepper` | NOASSERTION | `fa9a1f8e6d` | Stage movement test CSVs (motion validation, not fab data). Fork |
| `packaging` | none | `c8df36eb1a` | Repository for IC packaging development. No fab data |
| `phoenix` | MIT | `d8379e6f5f` | tube furnace. No fab data |
| `piezo-nanopositioner` | none | `89b86295cc` | Fill me in later!. No fab data |
| `piezoelectric-nanopositioner` | CERN-OHL-S-2.0 | `8a6e27eaf7` | Arduino code for piezoelectric nanopositioner. No fab data |
| `probe-station` | none | `ba7ed23b50` | Fill me in later!. No fab data |
| `projector-pcb` | none | `ca7c68ebee` | Projector PCB for Stepper "V3?". No fab data |
| `QCM_Controller` | none | `d03fbb4676` | Teensy 4.1. No fab data |
| `raman-spectrometer` | CERN-OHL-S-2.0 | `158ad5c2a4` | STM32 and Pi code for Raman spectrometer. No fab data |
| `RF-sputtering-chamber-CMU-version-` | NOASSERTION | `fd9dbe7402` | Tool hardware, firmware or software. No fab data |
| `smu` | none | `e10afc1ff4` | **Format, no data.** `smu.py` writes `{chip}_{device}_currents.csv` and `_voltages.csv` (Analog Discovery 3) |
| `spin-coater` | none | `61d0b2ecac` | Repository for all spin coater versions. Keep things organized in folders and labeled. No fab data |
| `spin-on-glass` | none | `HEAD` | DIY spin-on glass/dopant. Based on ProjectsInFlight video. No fab data |
| `spincoater` | NOASSERTION | `08b733cb51` | Tool hardware, firmware or software. No fab data |
| `spincoater-rahim-jay` | none | `9fdab0b92c` | Modified Maasi enclosure with all-new hardware and software, supports PID control. No fab data |
| `Sputtering-Controls` | none | `f35bffbb08` | Microcontroller code to control sputtering parameters. No fab data |
| `step2` | MIT | `f029238a7e` | temporary repo for stepper experiments. No fab data |
| `stepper` | MIT | `1640b32d33` | Stepper control software (Python). No run logs written to files |
| `stepper-v2-led` | NOASSERTION | `4a960925f9` | Hacker Fab V2 Stepper UV LED PCB. No fab data |
| `stepper_attachments` | none | `72951b7399` | RF-DETR weights and training data for alignment marks (images). No process data |
| `stepper_cad` | none | `e6243f6b86` | Tool hardware, firmware or software. No fab data |
| `storage` | none | `3bb0dab824` | CAD, PCB and vendor docs archive. No fab data |
| `thermal-evaporator` | CERN-OHL-S-2.0 | `4a64087697` | Solidworks CAD for vacuum chamber (for now). No fab data |
| `ti_dlp471_led_board` | none | `6fde520920` | Tool hardware, firmware or software. No fab data |
| `TubeFurnace` | none | `28ede9a52c` | Includes code for the tube furnace operation. No fab data |
| `usb-pd-psu` | none | `548c956305` | Tool hardware, firmware or software. No fab data |
| `uw-p901-reader` | CERN-OHL-S-2.0 | `a1c6426824` | **Data.** `901p-serial/output.csv`: one MKS 901P pump-down log, 1,129 readings, no unit or wall time recorded |
| `uw-spincoater` | none | `54ded9bf54` | UW Spincoater implementation, started Fall 2025. No fab data |
| `uw-tube-furnace_pid` | none | `ed18e74782` | **Format, no data.** Ramp/hold recipe JSON (`furnace/recipe.py`) |
| `uw_mfc_controller` | none | `fd932a69f2` | UW MFC Controller PCB. No fab data |
| `waterloofab.com` | MIT | `79307b5068` | Tool hardware, firmware or software. No fab data |
