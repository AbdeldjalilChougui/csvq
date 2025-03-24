# csvq

A fast CSV command-line toolkit in Rust: xsv-style streaming commands, CSV ↔ Parquet
conversion, and SQL over files (`csvq sql "SELECT … FROM data.csv"`) powered by
[Polars](https://pola.rs).

## Install

Prebuilt binaries (Linux x86_64/aarch64, macOS x86_64/arm64, Windows x86_64) are
attached to every [GitHub release](https://github.com/AbdeldjalilChougui/csvq/releases), with installers:

```sh
# Linux / macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/AbdeldjalilChougui/csvq/releases/latest/download/csvq-installer.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/AbdeldjalilChougui/csvq/releases/latest/download/csvq-installer.ps1 | iex"
```

Or build from crates.io:

```sh
cargo install csvq --locked
```

## Quick start

```sh
cargo install --path .

csvq headers people.csv
csvq count people.csv
csvq select name,3-4 people.csv            # names, 1-based indices, ranges (a-c, 3-)
csvq search -s city -i '^oran$' people.csv  # regex filter (-v to invert)
csvq slice -s 10 -l 5 people.csv            # rows 10..15  (-i N for one row)
csvq sort -s age -N -R people.csv           # numeric, reversed (multi-key: -s city,name)
csvq frequency -s city -l 5 people.csv      # top values per column
csvq stats --cardinality people.csv         # type, nulls, min/max, lengths, mean, stddev
csvq join city people.csv city cities.csv   # hash join (--left for left join)
csvq convert people.csv people.parquet      # and back: .parquet -> .csv/.tsv
csvq sql "SELECT city, count(*) n, avg(age) FROM people.csv GROUP BY city ORDER BY n DESC"
csvq sql "SELECT p.name, c.code FROM 'data/people.parquet' p JOIN cities.csv c ON p.city = c.city"
cat x.csv | csvq -d ';' select 2            # stdin, custom delimiter (-d tab), --no-headers
```

## How it works

- **Streaming commands** (`ops.rs`) use the `csv` crate. `select`, `search`, `slice`,
  `count`, `stats`, `frequency` and the probe side of `join` stream row by row in
  constant memory, apart from per-column counters. `sort` and the build side of `join`
  (the second file) are held in memory.
- **stats** infers the column type (Integer ⊂ Float ⊂ String, ignoring empty cells),
  computes mean and population standard deviation in one pass (Welford's algorithm),
  and counts distinct values on request.
- **convert / sql** (`frame.rs`) scan files lazily with Polars. For `sql`, file paths
  after `FROM`/`JOIN` (bare or quoted) are rewritten to table names taken from the file
  stem and registered in a Polars `SQLContext`. String literals are left alone. The
  result is written as CSV to stdout.

## Tests

`cargo test`: unit tests for column selection, every streaming command, the SQL
rewriter, and CLI tests (`tests/cli.rs`) covering stdin/delimiters, joins, error
messages, a CSV → Parquet → CSV byte-for-byte round trip, and SQL aggregation and a
Parquet+CSV join.

## Status / limitations

- `sort` and `join` load data into memory. There is no external sort or index (xsv's
  `index` command is not implemented).
- Type inference in `stats` is per-cell `i64`/`f64` parsing. Dates are reported as
  strings.
- `sql` supports what Polars SQL supports, and only files with `.csv`, `.tsv` or
  `.parquet` extensions are recognized after `FROM`/`JOIN`.
- The first build is slow: Polars is a large dependency.

## License

MIT, see [LICENSE](LICENSE).
