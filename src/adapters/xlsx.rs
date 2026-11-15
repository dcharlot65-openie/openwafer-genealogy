//! A minimal .xlsx reader: enough to read the cell text of the first worksheet.
//!
//! An .xlsx file is a zip archive of XML parts. This reads the central directory, inflates
//! `xl/sharedStrings.xml` and the first `xl/worksheets/sheet*.xml`, and returns the grid of
//! cell strings. It ignores styles, formulas (it takes the cached value) and images. That is
//! all a process traveler needs, and it avoids a spreadsheet dependency.

use flate2::read::DeflateDecoder;
use std::io::Read;

fn u16le(b: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as usize)
}
fn u32le(b: &[u8], i: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize)
}

/// Every member of a zip archive as (name, bytes). Stored and deflated members only.
pub fn unzip(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    // End of central directory: signature 0x06054b50 within the last 64 KiB + 22 bytes.
    let start = b.len().saturating_sub(22 + 65_535);
    let eocd = (start..b.len().saturating_sub(21))
        .rev()
        .find(|&i| b[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or("not a zip archive")?;
    let n = u16le(b, eocd + 10).ok_or("truncated zip")?;
    let mut p = u32le(b, eocd + 16).ok_or("truncated zip")?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if b.get(p..p + 4) != Some(&[0x50, 0x4b, 0x01, 0x02]) {
            return Err("bad central directory entry".into());
        }
        let method = u16le(b, p + 10).ok_or("truncated")?;
        let csize = u32le(b, p + 20).ok_or("truncated")?;
        let nlen = u16le(b, p + 28).ok_or("truncated")?;
        let xlen = u16le(b, p + 30).ok_or("truncated")?;
        let clen = u16le(b, p + 32).ok_or("truncated")?;
        let local = u32le(b, p + 42).ok_or("truncated")?;
        let name =
            String::from_utf8_lossy(b.get(p + 46..p + 46 + nlen).ok_or("truncated")?).to_string();
        p += 46 + nlen + xlen + clen;
        if b.get(local..local + 4) != Some(&[0x50, 0x4b, 0x03, 0x04]) {
            return Err(format!("{name}: bad local header"));
        }
        let lnlen = u16le(b, local + 26).ok_or("truncated")?;
        let lxlen = u16le(b, local + 28).ok_or("truncated")?;
        let data = b
            .get(local + 30 + lnlen + lxlen..local + 30 + lnlen + lxlen + csize)
            .ok_or("truncated member")?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => {
                let mut v = Vec::new();
                DeflateDecoder::new(data)
                    .read_to_end(&mut v)
                    .map_err(|e| format!("{name}: {e}"))?;
                v
            }
            m => return Err(format!("{name}: compression method {m} not supported")),
        };
        out.push((name, bytes));
    }
    Ok(out)
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(j) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let ent = &tail[1..j];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=j]),
        }
        rest = &tail[j + 1..];
    }
    out.push_str(rest);
    out
}

/// The concatenated text of every `<t>` element inside `xml` (one shared string or inline string).
fn text_runs(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<t") {
        let after = &rest[i + 2..];
        // `<t>` or `<t xml:space="preserve">`, not `<tc...>` etc.
        if !(after.starts_with('>') || after.starts_with(' ')) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        if after[..gt].ends_with('/') {
            rest = &after[gt + 1..];
            continue;
        }
        let body = &after[gt + 1..];
        let Some(end) = body.find("</t>") else { break };
        out.push_str(&unescape(&body[..end]));
        rest = &body[end + 4..];
    }
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let i = tag.find(&key)? + key.len();
    let j = tag[i..].find('"')?;
    Some(&tag[i..i + j])
}

/// Column index of a cell reference like `AB12` (A = 0).
fn col_of(r: &str) -> Option<usize> {
    let n = r
        .bytes()
        .take_while(|c| c.is_ascii_alphabetic())
        .fold(0usize, |a, c| {
            a * 26 + (c.to_ascii_uppercase() - b'A' + 1) as usize
        });
    n.checked_sub(1)
}

/// The first worksheet as rows of cell strings (empty cells are empty strings).
pub fn first_sheet(b: &[u8]) -> Result<Vec<Vec<String>>, String> {
    let parts = unzip(b)?;
    let get = |n: &str| {
        parts
            .iter()
            .find(|p| p.0 == n)
            .map(|p| String::from_utf8_lossy(&p.1).to_string())
    };
    let mut shared = Vec::new();
    if let Some(ss) = get("xl/sharedStrings.xml") {
        let mut rest = ss.as_str();
        while let Some(i) = rest.find("<si") {
            let body = &rest[i..];
            let Some(end) = body.find("</si>") else { break };
            shared.push(text_runs(&body[..end]));
            rest = &body[end + 5..];
        }
    }
    let mut sheets: Vec<&(String, Vec<u8>)> = parts
        .iter()
        .filter(|p| p.0.starts_with("xl/worksheets/sheet") && p.0.ends_with(".xml"))
        .collect();
    sheets.sort_by_key(|p| {
        p.0.trim_start_matches("xl/worksheets/sheet")
            .trim_end_matches(".xml")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    let sheet = sheets.first().ok_or("no worksheet")?;
    let xml = String::from_utf8_lossy(&sheet.1).to_string();
    let data = match (xml.find("<sheetData"), xml.find("</sheetData>")) {
        (Some(a), Some(b)) if b > a => &xml[a..b],
        _ => "",
    };
    let mut rows = Vec::new();
    let mut rest = data;
    while let Some(i) = rest.find("<row") {
        let body = &rest[i..];
        let end = body.find("</row>").unwrap_or(body.len());
        let row_xml = &body[..end];
        let mut cells: Vec<String> = Vec::new();
        let mut r = row_xml;
        while let Some(ci) = r.find("<c ") {
            let c = &r[ci..];
            let gt = c.find('>').ok_or("bad cell")?;
            let tag = &c[..gt];
            let (inner, next) = if tag.ends_with('/') {
                ("", &c[gt + 1..])
            } else {
                let close = c.find("</c>").ok_or("unclosed cell")?;
                (&c[gt + 1..close], &c[close + 4..])
            };
            let col = attr(tag, "r").and_then(col_of).unwrap_or(cells.len());
            let t = attr(tag, "t").unwrap_or("");
            let v = inner.find("<v>").and_then(|a| {
                inner[a + 3..]
                    .find("</v>")
                    .map(|b| unescape(&inner[a + 3..a + 3 + b]))
            });
            let val = match t {
                "s" => v
                    .and_then(|x| x.parse::<usize>().ok())
                    .and_then(|k| shared.get(k).cloned())
                    .unwrap_or_default(),
                "inlineStr" => text_runs(inner),
                _ => v.unwrap_or_default(),
            };
            if cells.len() <= col {
                cells.resize(col + 1, String::new());
            }
            cells[col] = val;
            r = next;
        }
        rows.push(cells);
        rest = &body[end.min(body.len())..];
        if end == body.len() {
            break;
        }
    }
    Ok(rows)
}
