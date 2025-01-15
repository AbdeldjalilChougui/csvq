//! Streaming CSV commands built on the `csv` crate. Each takes a reader
//! and a writer so it can be tested without files.

use crate::select;
use anyhow::{bail, Context, Result};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

#[derive(Debug, Clone, Copy)]
pub struct CsvOpts {
    pub delimiter: u8,
    pub no_headers: bool,
}

impl Default for CsvOpts {
    fn default() -> Self {
        CsvOpts { delimiter: b',', no_headers: false }
    }
}

pub fn reader<R: Read>(r: R, o: CsvOpts) -> csv::Reader<R> {
    csv::ReaderBuilder::new().delimiter(o.delimiter).has_headers(!o.no_headers).flexible(false).from_reader(r)
}

pub fn writer<W: Write>(w: W, o: CsvOpts) -> csv::Writer<W> {
    csv::WriterBuilder::new().delimiter(o.delimiter).from_writer(w)
}

/// Header names; with `--no-headers`, synthesized as 1, 2, 3...
fn headers<R: Read>(rdr: &mut csv::Reader<R>, o: CsvOpts) -> Result<Vec<String>> {
    let h = rdr.headers().context("reading header row")?;
    Ok(if o.no_headers { (1..=h.len()).map(|i| i.to_string()).collect() } else { h.iter().map(String::from).collect() })
}

fn selection(spec: Option<&str>, headers: &[String]) -> Result<Vec<usize>> {
    match spec {
        Some(s) => select::parse(s, headers),
        None => Ok((0..headers.len()).collect()),
    }
}

// ---- headers / count ------------------------------------------------------

pub fn count<R: Read>(r: R, o: CsvOpts) -> Result<u64> {
    let mut rdr = reader(r, o);
    let mut n = 0;
    let mut rec = csv::ByteRecord::new();
    while rdr.read_byte_record(&mut rec)? {
        n += 1;
    }
    Ok(n)
}

pub fn headers_cmd<R: Read, W: Write>(r: R, out: W, o: CsvOpts) -> Result<()> {
    let mut rdr = reader(r, o);
    let mut out = out;
    for (i, h) in headers(&mut rdr, o)?.iter().enumerate() {
        writeln!(out, "{}\t{h}", i + 1)?;
    }
    Ok(())
}

// ---- select ---------------------------------------------------------------

pub fn select_cmd<R: Read, W: Write>(r: R, out: W, o: CsvOpts, spec: &str) -> Result<()> {
    let mut rdr = reader(r, o);
    let h = headers(&mut rdr, o)?;
    let cols = select::parse(spec, &h)?;
    let mut wtr = writer(out, o);
    if !o.no_headers {
        wtr.write_record(cols.iter().map(|&i| &h[i]))?;
    }
    let mut rec = csv::ByteRecord::new();
    while rdr.read_byte_record(&mut rec)? {
        wtr.write_record(cols.iter().map(|&i| rec.get(i).unwrap_or(b"")))?;
    }
    wtr.flush()?;
    Ok(())
}

// ---- search ---------------------------------------------------------------

pub fn search<R: Read, W: Write>(
    r: R,
    out: W,
    o: CsvOpts,
    pattern: &str,
    cols: Option<&str>,
    ignore_case: bool,
    invert: bool,
) -> Result<u64> {
    let re = regex::RegexBuilder::new(pattern).case_insensitive(ignore_case).build()?;
    let mut rdr = reader(r, o);
    let h = headers(&mut rdr, o)?;
    let cols = selection(cols, &h)?;
    let mut wtr = writer(out, o);
    if !o.no_headers {
        wtr.write_record(&h)?;
    }
    let mut matched = 0;
    for rec in rdr.records() {
        let rec = rec?;
        let hit = cols.iter().any(|&i| rec.get(i).is_some_and(|f| re.is_match(f)));
        if hit != invert {
            wtr.write_record(&rec)?;
            matched += 1;
        }
    }
    wtr.flush()?;
    Ok(matched)
}

// ---- slice ----------------------------------------------------------------

/// Rows `[start, end)` (0-based, header excluded).
pub fn slice<R: Read, W: Write>(r: R, out: W, o: CsvOpts, start: usize, end: Option<usize>) -> Result<()> {
    let mut rdr = reader(r, o);
    let h = headers(&mut rdr, o)?;
    let mut wtr = writer(out, o);
    if !o.no_headers {
        wtr.write_record(&h)?;
    }
    let mut rec = csv::ByteRecord::new();
    let mut i = 0;
    while rdr.read_byte_record(&mut rec)? {
        if end.is_some_and(|e| i >= e) {
            break;
        }
        if i >= start {
            wtr.write_byte_record(&rec)?;
        }
        i += 1;
    }
    wtr.flush()?;
    Ok(())
}

// ---- sort -----------------------------------------------------------------

fn cmp_numeric(a: &str, b: &str) -> Ordering {
    // Numbers first in numeric order; non-numeric values after, as text.
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.total_cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

pub fn sort<R: Read, W: Write>(r: R, out: W, o: CsvOpts, cols: Option<&str>, numeric: bool, reverse: bool) -> Result<()> {
    let mut rdr = reader(r, o);
    let h = headers(&mut rdr, o)?;
    let cols = selection(cols, &h)?;
    let mut rows: Vec<csv::StringRecord> = rdr.records().collect::<Result<_, _>>()?;
    rows.sort_by(|a, b| {
        let ord = cols
            .iter()
            .map(|&i| {
                let (x, y) = (a.get(i).unwrap_or(""), b.get(i).unwrap_or(""));
                if numeric {
                    cmp_numeric(x, y)
                } else {
                    x.cmp(y)
                }
            })
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal);
        if reverse {
            ord.reverse()
        } else {
            ord
        }
    });
    let mut wtr = writer(out, o);
    if !o.no_headers {
        wtr.write_record(&h)?;
    }
    for r in &rows {
        wtr.write_record(r)?;
    }
    wtr.flush()?;
    Ok(())
}

// ---- frequency ------------------------------------------------------------

pub fn frequency<R: Read, W: Write>(r: R, out: W, o: CsvOpts, cols: Option<&str>, limit: usize) -> Result<()> {
    let mut rdr = reader(r, o);
    let h = headers(&mut rdr, o)?;
    let cols = selection(cols, &h)?;
    let mut tables: Vec<HashMap<String, u64>> = vec![HashMap::new(); cols.len()];
    for rec in rdr.records() {
        let rec = rec?;
        for (t, &i) in tables.iter_mut().zip(&cols) {
            *t.entry(rec.get(i).unwrap_or("").to_string()).or_default() += 1;
        }
    }
    let mut wtr = writer(out, CsvOpts { delimiter: b',', ..o });
    wtr.write_record(["field", "value", "count"])?;
    for (t, &i) in tables.into_iter().zip(&cols) {
        let mut v: Vec<(String, u64)> = t.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if limit > 0 {
            v.truncate(limit);
        }
        for (val, n) in v {
            let shown = if val.is_empty() { "(NULL)".to_string() } else { val };
            wtr.write_record([h[i].as_str(), &shown, &n.to_string()])?;
        }
    }
    wtr.flush()?;
    Ok(())
}

// ---- stats ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Empty,
    Integer,
    Float,
    String,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Empty => "Empty",
            Kind::Integer => "Integer",
            Kind::Float => "Float",
            Kind::String => "String",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColumnStats {
    pub kind: Kind,
    pub count: u64,
    pub nulls: u64,
    pub min_num: f64,
    pub max_num: f64,
    pub min_str: Option<String>,
    pub max_str: Option<String>,
    pub min_len: usize,
    pub max_len: usize,
    // Welford's online mean / variance
    n_num: u64,
    mean: f64,
    m2: f64,
    distinct: Option<HashSet<String>>,
}

impl ColumnStats {
    fn new(cardinality: bool) -> Self {
        ColumnStats {
            kind: Kind::Empty,
            count: 0,
            nulls: 0,
            min_num: f64::INFINITY,
            max_num: f64::NEG_INFINITY,
            min_str: None,
            max_str: None,
            min_len: usize::MAX,
            max_len: 0,
            n_num: 0,
            mean: 0.0,
            m2: 0.0,
            distinct: cardinality.then(HashSet::new),
        }
    }

    fn add(&mut self, field: &str) {
        self.count += 1;
        if let Some(d) = &mut self.distinct {
            if !d.contains(field) {
                d.insert(field.to_string());
            }
        }
        let f = field.trim();
        if f.is_empty() {
            self.nulls += 1;
            return;
        }
        let len = field.chars().count();
        self.min_len = self.min_len.min(len);
        self.max_len = self.max_len.max(len);
        if self.min_str.as_deref().is_none_or(|m| field < m) {
            self.min_str = Some(field.to_string());
        }
        if self.max_str.as_deref().is_none_or(|m| field > m) {
            self.max_str = Some(field.to_string());
        }
        let kind = if f.parse::<i64>().is_ok() {
            Kind::Integer
        } else if f.parse::<f64>().is_ok_and(f64::is_finite) {
            Kind::Float
        } else {
            Kind::String
        };
        self.kind = self.kind.max(kind);
        if kind != Kind::String {
            let x: f64 = f.parse().unwrap();
            self.min_num = self.min_num.min(x);
            self.max_num = self.max_num.max(x);
            self.n_num += 1;
            let d = x - self.mean;
            self.mean += d / self.n_num as f64;
            self.m2 += d * (x - self.mean);
        }
    }

    pub fn mean(&self) -> Option<f64> {
        matches!(self.kind, Kind::Integer | Kind::Float).then_some(self.mean)
    }

    /// Population standard deviation.
    pub fn stddev(&self) -> Option<f64> {
        self.mean().map(|_| (self.m2 / self.n_num as f64).sqrt())
    }

    pub fn cardinality(&self) -> Option<usize> {
        self.distinct.as_ref().map(HashSet::len)
    }
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        let s = format!("{x:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

pub fn compute_stats<R: Read>(r: R, o: CsvOpts, cols: Option<&str>, cardinality: bool) -> Result<(Vec<String>, Vec<ColumnStats>)> {
    let mut rdr = reader(r, o);
    let h = headers(&mut rdr, o)?;
    let cols = selection(cols, &h)?;
    let mut stats: Vec<ColumnStats> = cols.iter().map(|_| ColumnStats::new(cardinality)).collect();
    for rec in rdr.records() {
        let rec = rec?;
        for (s, &i) in stats.iter_mut().zip(&cols) {
            s.add(rec.get(i).unwrap_or(""));
        }
    }
    Ok((cols.iter().map(|&i| h[i].clone()).collect(), stats))
}

pub fn stats<R: Read, W: Write>(r: R, out: W, o: CsvOpts, cols: Option<&str>, cardinality: bool) -> Result<()> {
    let (names, stats) = compute_stats(r, o, cols, cardinality)?;
    let mut wtr = csv::Writer::from_writer(out);
    let mut header = vec!["field", "type", "count", "nulls", "min", "max", "min_length", "max_length", "mean", "stddev"];
    if cardinality {
        header.push("cardinality");
    }
    wtr.write_record(&header)?;
    for (name, s) in names.iter().zip(&stats) {
        let numeric = matches!(s.kind, Kind::Integer | Kind::Float);
        let (min, max) = if numeric {
            (fmt_num(s.min_num), fmt_num(s.max_num))
        } else {
            (s.min_str.clone().unwrap_or_default(), s.max_str.clone().unwrap_or_default())
        };
        let opt = |x: Option<f64>| x.map(fmt_num).unwrap_or_default();
        let mut row = vec![
            name.clone(),
            s.kind.name().to_string(),
            s.count.to_string(),
            s.nulls.to_string(),
            min,
            max,
            if s.max_len > 0 { s.min_len.to_string() } else { String::new() },
            if s.max_len > 0 { s.max_len.to_string() } else { String::new() },
            opt(s.mean()),
            opt(s.stddev()),
        ];
        if cardinality {
            row.push(s.cardinality().unwrap_or(0).to_string());
        }
        wtr.write_record(&row)?;
    }
    wtr.flush()?;
    Ok(())
}

// ---- join -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
}

/// Hash join: the right input is loaded into memory, the left one streamed.
pub fn join<R1: Read, R2: Read, W: Write>(
    left: R1,
    left_cols: &str,
    right: R2,
    right_cols: &str,
    out: W,
    o: CsvOpts,
    kind: JoinKind,
) -> Result<()> {
    let mut lr = reader(left, o);
    let mut rr = reader(right, o);
    let lh = headers(&mut lr, o)?;
    let rh = headers(&mut rr, o)?;
    let lk = select::parse(left_cols, &lh)?;
    let rk = select::parse(right_cols, &rh)?;
    if lk.len() != rk.len() {
        bail!("join needs the same number of key columns on both sides ({} vs {})", lk.len(), rk.len());
    }
    let mut index: HashMap<Vec<String>, Vec<csv::StringRecord>> = HashMap::new();
    for rec in rr.records() {
        let rec = rec?;
        let key: Vec<String> = rk.iter().map(|&i| rec.get(i).unwrap_or("").to_string()).collect();
        index.entry(key).or_default().push(rec);
    }
    let mut wtr = writer(out, o);
    if !o.no_headers {
        wtr.write_record(lh.iter().chain(rh.iter()))?;
    }
    let empty_right: Vec<&str> = vec![""; rh.len()];
    for rec in lr.records() {
        let rec = rec?;
        let key: Vec<String> = lk.iter().map(|&i| rec.get(i).unwrap_or("").to_string()).collect();
        match index.get(&key) {
            Some(matches) => {
                for m in matches {
                    wtr.write_record(rec.iter().chain(m.iter()))?;
                }
            }
            None if kind == JoinKind::Left => wtr.write_record(rec.iter().chain(empty_right.iter().copied()))?,
            None => {}
        }
    }
    wtr.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEOPLE: &str = "id,name,age,city\n1,Amina,34,Oran\n2,Karim,28,Algiers\n3,Sara,,Oran\n4,Yacine,41,Constantine\n5,Lina,28,Algiers\n";

    fn run(f: impl FnOnce(&mut Vec<u8>) -> Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn count_and_select() {
        assert_eq!(count(PEOPLE.as_bytes(), CsvOpts::default()).unwrap(), 5);
        let s = run(|o| select_cmd(PEOPLE.as_bytes(), o, CsvOpts::default(), "name,1"));
        assert_eq!(s.lines().take(2).collect::<Vec<_>>(), vec!["name,id", "Amina,1"]);
    }

    #[test]
    fn search_with_columns_case_and_invert() {
        let s = run(|o| search(PEOPLE.as_bytes(), o, CsvOpts::default(), "^oran$", Some("city"), true, false).map(|_| ()));
        assert_eq!(s.lines().count(), 3);
        let s = run(|o| search(PEOPLE.as_bytes(), o, CsvOpts::default(), "Oran", Some("city"), false, true).map(|_| ()));
        assert_eq!(s.lines().count(), 4); // header + 3 non-Oran rows
    }

    #[test]
    fn slice_rows() {
        let s = run(|o| slice(PEOPLE.as_bytes(), o, CsvOpts::default(), 1, Some(3)));
        assert_eq!(s, "id,name,age,city\n2,Karim,28,Algiers\n3,Sara,,Oran\n");
    }

    #[test]
    fn sort_numeric_reverse_and_multi_key() {
        let s = run(|o| sort(PEOPLE.as_bytes(), o, CsvOpts::default(), Some("age"), true, true));
        let names: Vec<&str> = s.lines().skip(1).map(|l| l.split(',').nth(1).unwrap()).collect();
        assert_eq!(names, vec!["Sara", "Yacine", "Amina", "Karim", "Lina"]); // empty age sorts last ascending, so first reversed
        let s = run(|o| sort(PEOPLE.as_bytes(), o, CsvOpts::default(), Some("city,name"), false, false));
        assert_eq!(s.lines().nth(1).unwrap(), "2,Karim,28,Algiers");
    }

    #[test]
    fn frequency_table() {
        let s = run(|o| frequency(PEOPLE.as_bytes(), o, CsvOpts::default(), Some("city,age"), 0));
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines[0], "field,value,count");
        assert_eq!(lines[1], "city,Algiers,2");
        assert_eq!(lines[2], "city,Oran,2");
        assert!(lines.contains(&"age,(NULL),1"));
        assert!(lines.contains(&"age,28,2"));
    }

    #[test]
    fn stats_numbers_and_strings() {
        let (_, st) = compute_stats(PEOPLE.as_bytes(), CsvOpts::default(), None, true).unwrap();
        let age = &st[2];
        assert_eq!(age.kind, Kind::Integer);
        assert_eq!((age.count, age.nulls), (5, 1));
        assert_eq!((age.min_num, age.max_num), (28.0, 41.0));
        assert!((age.mean().unwrap() - 32.75).abs() < 1e-9);
        assert!((age.stddev().unwrap() - 5.35607).abs() < 1e-4);
        assert_eq!(st[3].kind, Kind::String);
        assert_eq!(st[3].min_str.as_deref(), Some("Algiers"));
        assert_eq!(st[3].cardinality(), Some(3));
        let mixed = "x\n1\n2.5\n";
        assert_eq!(compute_stats(mixed.as_bytes(), CsvOpts::default(), None, false).unwrap().1[0].kind, Kind::Float);
    }

    #[test]
    fn inner_and_left_join() {
        let cities = "city,wilaya_code\nOran,31\nAlgiers,16\n";
        let s = run(|o| join(PEOPLE.as_bytes(), "city", cities.as_bytes(), "city", o, CsvOpts::default(), JoinKind::Inner));
        assert_eq!(s.lines().count(), 5); // header + 4 matches (Constantine dropped)
        assert!(s.contains("1,Amina,34,Oran,Oran,31"));
        let s = run(|o| join(PEOPLE.as_bytes(), "city", cities.as_bytes(), "city", o, CsvOpts::default(), JoinKind::Left));
        assert!(s.contains("4,Yacine,41,Constantine,,"));
    }

    #[test]
    fn custom_delimiter_and_no_headers() {
        let o = CsvOpts { delimiter: b';', no_headers: true };
        let s = run(|w| select_cmd("a;b\nc;d\n".as_bytes(), w, o, "2"));
        assert_eq!(s, "b\nd\n");
    }
}
