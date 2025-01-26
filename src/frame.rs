//! Polars-backed commands: CSV <-> Parquet conversion and SQL over files.

use anyhow::{bail, Context, Result};
use polars::prelude::*;
use polars::sql::SQLContext;
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Parquet,
}

pub fn format_of(path: &str) -> Option<Format> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "csv" | "txt" => Some(Format::Csv),
        "tsv" | "tab" => Some(Format::Tsv),
        "parquet" | "pq" => Some(Format::Parquet),
        _ => None,
    }
}

/// Lazily scan a CSV/TSV/Parquet file.
pub fn scan(path: &str, csv_delimiter: u8) -> Result<LazyFrame> {
    if !Path::new(path).exists() {
        bail!("file not found: {path}");
    }
    let lf = match format_of(path) {
        Some(Format::Parquet) => LazyFrame::scan_parquet(PlRefPath::from(path), ScanArgsParquet::default())?,
        fmt => {
            let sep = if fmt == Some(Format::Tsv) { b'\t' } else { csv_delimiter };
            LazyCsvReader::new(PlRefPath::from(path))
                .with_has_header(true)
                .with_separator(sep)
                .with_infer_schema_length(Some(10_000))
                .finish()?
        }
    };
    Ok(lf)
}

pub fn write_csv<W: Write>(df: &mut DataFrame, out: W, delimiter: u8) -> Result<()> {
    CsvWriter::new(out).include_header(true).with_separator(delimiter).finish(df)?;
    Ok(())
}

/// Convert between CSV/TSV and Parquet (any direction). Returns the row count.
pub fn convert(input: &str, output: &str, delimiter: u8) -> Result<usize> {
    let mut df = scan(input, delimiter)?.collect().map_err(anyhow::Error::from).with_context(|| format!("reading {input}"))?;
    let file = File::create(output).with_context(|| format!("creating {output}"))?;
    match format_of(output) {
        Some(Format::Parquet) => {
            ParquetWriter::new(file).finish(&mut df)?;
        }
        Some(Format::Tsv) => write_csv(&mut df, file, b'\t')?,
        Some(Format::Csv) => write_csv(&mut df, file, delimiter)?,
        None => bail!("unknown output format for '{output}' (use .csv, .tsv or .parquet)"),
    }
    Ok(df.height())
}

/// A file referenced in a query, and the table name it was replaced with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub name: String,
    pub path: String,
}

fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || "_-./~:\\".contains(c)
}

fn table_name_for(path: &str, taken: &[TableRef]) -> String {
    let stem = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("t");
    let mut base: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) {
        base.insert(0, 't');
    }
    let mut name = base.clone();
    let mut i = 2;
    while taken.iter().any(|t| t.name == name && t.path != path) {
        name = format!("{base}_{i}");
        i += 1;
    }
    name
}

/// Replace file paths that follow `FROM` / `JOIN` with table names.
/// Paths may be bare (`data/people.csv`) or quoted (`'my file.csv'`, `"x.parquet"`, `` `y.tsv` ``).
pub fn rewrite_query(query: &str) -> (String, Vec<TableRef>) {
    let chars: Vec<char> = query.chars().collect();
    let mut out = String::with_capacity(query.len());
    let mut tables: Vec<TableRef> = Vec::new();
    let mut prev_word = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let after_kw = prev_word == "FROM" || prev_word == "JOIN";
        if after_kw && !c.is_whitespace() {
            // Candidate table reference.
            let (raw, end) = if c == '\'' || c == '"' || c == '`' {
                let close = chars[i + 1..].iter().position(|&x| x == c).map(|p| i + 1 + p);
                match close {
                    Some(j) => (chars[i + 1..j].iter().collect::<String>(), j + 1),
                    None => (String::new(), i),
                }
            } else {
                let mut j = i;
                while j < chars.len() && is_path_char(chars[j]) {
                    j += 1;
                }
                (chars[i..j].iter().collect::<String>(), j)
            };
            prev_word.clear();
            if end > i && format_of(&raw).is_some() {
                let name = match tables.iter().find(|t| t.path == raw) {
                    Some(t) => t.name.clone(),
                    None => {
                        let name = table_name_for(&raw, &tables);
                        tables.push(TableRef { name: name.clone(), path: raw });
                        name
                    }
                };
                out.push_str(&name);
                i = end;
                continue;
            }
        }
        if c == '\'' {
            // Copy string literals verbatim so words inside them aren't keywords.
            let close = chars[i + 1..].iter().position(|&x| x == '\'').map_or(chars.len(), |p| i + 2 + p);
            out.extend(&chars[i..close.min(chars.len())]);
            i = close;
            prev_word.clear();
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            prev_word = chars[i..j].iter().collect::<String>().to_ascii_uppercase();
            out.extend(&chars[i..j]);
            i = j;
            continue;
        }
        if !c.is_whitespace() {
            prev_word.clear();
        }
        out.push(c);
        i += 1;
    }
    (out, tables)
}

/// Run a SQL query whose FROM/JOIN clauses name files; returns the result frame.
pub fn sql(query: &str, delimiter: u8) -> Result<DataFrame> {
    let (rewritten, tables) = rewrite_query(query);
    if tables.is_empty() {
        bail!("no .csv/.tsv/.parquet file found after FROM or JOIN in the query");
    }
    let mut ctx = SQLContext::new();
    for t in &tables {
        ctx.register(&t.name, scan(&t.path, delimiter)?);
    }
    let df = ctx.execute(&rewritten).map_err(anyhow::Error::from).with_context(|| format!("executing: {rewritten}"))?.collect()?;
    Ok(df)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_bare_and_quoted_paths() {
        let (q, t) = rewrite_query("SELECT a FROM data/people.csv p JOIN 'my cities.parquet' c ON p.x = c.x");
        assert_eq!(q, "SELECT a FROM people p JOIN my_cities c ON p.x = c.x");
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].path, "my cities.parquet");
    }

    #[test]
    fn leaves_strings_and_non_files_alone() {
        let (q, t) = rewrite_query("select * from x.csv where name = 'from y.csv' and k in (select k from x.csv)");
        assert_eq!(q, "select * from x where name = 'from y.csv' and k in (select k from x)");
        assert_eq!(t.len(), 1);
        let (q, t) = rewrite_query("SELECT 1 FROM some_table");
        assert_eq!((q.as_str(), t.len()), ("SELECT 1 FROM some_table", 0));
    }

    #[test]
    fn unique_names_for_same_stem() {
        let (q, _) = rewrite_query("SELECT * FROM a/x.csv JOIN b/x.csv USING (id)");
        assert_eq!(q, "SELECT * FROM x JOIN x_2 USING (id)");
    }
}
