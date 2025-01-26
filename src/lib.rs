//! csvq: an xsv-style CSV toolkit. Streaming commands live in [`ops`]
//! (built on the `csv` crate); Parquet conversion and SQL in [`frame`] (Polars).

pub mod frame;
pub mod ops;
pub mod select;
