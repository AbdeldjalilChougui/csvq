use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use csvq::ops::{self, CsvOpts, JoinKind};
use csvq::frame;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};

#[derive(Parser)]
#[command(name = "csvq", version, about = "Fast CSV toolkit: xsv-style commands, Parquet conversion and SQL")]
struct Cli {
    /// Field delimiter (a single character, or "tab")
    #[arg(short, long, global = true, default_value = ",")]
    delimiter: String,
    /// Treat the first row as data; columns are then named 1, 2, 3...
    #[arg(long, global = true)]
    no_headers: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List column names with their 1-based index
    Headers { input: Option<String> },
    /// Count data rows
    Count { input: Option<String> },
    /// Select/reorder columns: names, 1-based indices, ranges (2-4, a-c, 3-)
    Select { columns: String, input: Option<String> },
    /// Keep rows where a regex matches (in all or selected columns)
    Search {
        pattern: String,
        input: Option<String>,
        #[arg(short, long)]
        select: Option<String>,
        #[arg(short, long)]
        ignore_case: bool,
        /// Keep rows that do NOT match
        #[arg(short = 'v', long)]
        invert: bool,
    },
    /// Rows by position (0-based, header excluded)
    Slice {
        input: Option<String>,
        #[arg(short, long, default_value_t = 0)]
        start: usize,
        /// End (exclusive)
        #[arg(short, long)]
        end: Option<usize>,
        /// Number of rows from start
        #[arg(short, long)]
        len: Option<usize>,
        /// A single row
        #[arg(short, long)]
        index: Option<usize>,
    },
    /// Sort rows (in memory)
    Sort {
        input: Option<String>,
        #[arg(short, long)]
        select: Option<String>,
        /// Compare as numbers (non-numbers sort last)
        #[arg(short = 'N', long)]
        numeric: bool,
        #[arg(short = 'R', long)]
        reverse: bool,
    },
    /// Value counts per column
    Frequency {
        input: Option<String>,
        #[arg(short, long)]
        select: Option<String>,
        /// Top N values per column (0 = all)
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },
    /// Per-column type, nulls, min/max, lengths, mean and stddev
    Stats {
        input: Option<String>,
        #[arg(short, long)]
        select: Option<String>,
        /// Also count distinct values (uses memory)
        #[arg(short, long)]
        cardinality: bool,
    },
    /// Hash join two files on key columns (inner by default)
    Join {
        columns1: String,
        input1: String,
        columns2: String,
        input2: String,
        #[arg(long)]
        left: bool,
    },
    /// Convert between CSV/TSV and Parquet (by file extension)
    Convert { input: String, output: String },
    /// Run SQL where FROM/JOIN name files: csvq sql "SELECT city, count(*) FROM people.csv GROUP BY city"
    Sql { query: String },
}

fn open(input: &Option<String>) -> Result<Box<dyn Read>> {
    Ok(match input.as_deref() {
        None | Some("-") => Box::new(io::stdin().lock()),
        Some(p) => Box::new(BufReader::new(File::open(p).with_context(|| format!("opening {p}"))?)),
    })
}

fn parse_delimiter(d: &str) -> Result<u8> {
    match d {
        "tab" | "\\t" => Ok(b'\t'),
        _ if d.len() == 1 => Ok(d.as_bytes()[0]),
        _ => bail!("delimiter must be a single byte or 'tab'"),
    }
}

fn main() {
    if let Err(e) = run() {
        // A closed pipe (e.g. `| head`) is not an error.
        if e.chain().any(|c| c.downcast_ref::<io::Error>().is_some_and(|io| io.kind() == io::ErrorKind::BrokenPipe)) {
            return;
        }
        eprintln!("csvq: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let o = CsvOpts { delimiter: parse_delimiter(&cli.delimiter)?, no_headers: cli.no_headers };
    let stdout = io::stdout();
    let out = BufWriter::new(stdout.lock());
    match cli.cmd {
        Cmd::Headers { input } => ops::headers_cmd(open(&input)?, out, o)?,
        Cmd::Count { input } => {
            let mut out = out;
            writeln!(out, "{}", ops::count(open(&input)?, o)?)?;
        }
        Cmd::Select { columns, input } => ops::select_cmd(open(&input)?, out, o, &columns)?,
        Cmd::Search { pattern, input, select, ignore_case, invert } => {
            ops::search(open(&input)?, out, o, &pattern, select.as_deref(), ignore_case, invert)?;
        }
        Cmd::Slice { input, start, end, len, index } => {
            let (s, e) = match (index, len, end) {
                (Some(i), _, _) => (i, Some(i + 1)),
                (None, Some(l), _) => (start, Some(start + l)),
                (None, None, e) => (start, e),
            };
            ops::slice(open(&input)?, out, o, s, e)?
        }
        Cmd::Sort { input, select, numeric, reverse } => ops::sort(open(&input)?, out, o, select.as_deref(), numeric, reverse)?,
        Cmd::Frequency { input, select, limit } => ops::frequency(open(&input)?, out, o, select.as_deref(), limit)?,
        Cmd::Stats { input, select, cardinality } => ops::stats(open(&input)?, out, o, select.as_deref(), cardinality)?,
        Cmd::Join { columns1, input1, columns2, input2, left } => {
            let kind = if left { JoinKind::Left } else { JoinKind::Inner };
            ops::join(open(&Some(input1))?, &columns1, open(&Some(input2))?, &columns2, out, o, kind)?
        }
        Cmd::Convert { input, output } => {
            let n = frame::convert(&input, &output, o.delimiter)?;
            eprintln!("wrote {n} rows to {output}");
        }
        Cmd::Sql { query } => {
            let mut df = frame::sql(&query, o.delimiter)?;
            frame::write_csv(&mut df, out, b',')?;
        }
    }
    Ok(())
}
