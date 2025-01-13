//! Column selection syntax shared by commands:
//! `name`, `3` (1-based index), `2-4`, `name1-name2`, `5-` (to the end),
//! combined with commas: `id,3-5,email`.

use anyhow::{bail, Result};

fn resolve_one(item: &str, headers: &[String]) -> Result<usize> {
    if let Some(i) = headers.iter().position(|h| h == item) {
        return Ok(i);
    }
    if let Ok(n) = item.parse::<usize>() {
        if n == 0 || n > headers.len() {
            bail!("column index {n} out of range (1..={})", headers.len());
        }
        return Ok(n - 1);
    }
    bail!("no column named '{item}' (columns: {})", headers.join(", "))
}

/// Resolve a selection against the header row. Returns 0-based indices in selection order.
pub fn parse(spec: &str, headers: &[String]) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        // An exact header match wins over range syntax (headers may contain '-').
        if headers.iter().any(|h| h == item) {
            out.push(resolve_one(item, headers)?);
            continue;
        }
        if let Some((a, b)) = item.split_once('-') {
            let start = if a.is_empty() { 0 } else { resolve_one(a, headers)? };
            let end = if b.is_empty() { headers.len() - 1 } else { resolve_one(b, headers)? };
            if start <= end {
                out.extend(start..=end);
            } else {
                out.extend((end..=start).rev());
            }
        } else {
            out.push(resolve_one(item, headers)?);
        }
    }
    if out.is_empty() {
        bail!("empty column selection");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h() -> Vec<String> {
        ["id", "name", "e-mail", "age", "city"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_indices_and_ranges() {
        assert_eq!(parse("name", &h()).unwrap(), vec![1]);
        assert_eq!(parse("1,3", &h()).unwrap(), vec![0, 2]);
        assert_eq!(parse("2-4", &h()).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse("name-age", &h()).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse("4-", &h()).unwrap(), vec![3, 4]);
        assert_eq!(parse("3-1", &h()).unwrap(), vec![2, 1, 0]);
        assert_eq!(parse("e-mail", &h()).unwrap(), vec![2]);
    }

    #[test]
    fn errors() {
        assert!(parse("nope", &h()).is_err());
        assert!(parse("9", &h()).is_err());
        assert!(parse("0", &h()).is_err());
        assert!(parse("", &h()).is_err());
    }
}
