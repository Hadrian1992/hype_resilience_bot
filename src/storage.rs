//! Append-only JSONL persistence (paper signals) + offline replay summary.
//!
//! Files live under ./state/ (gitignored runtime artifacts).

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

const STATE_DIR: &str = "state";
pub const SIGNALS_FILE: &str = "signals.jsonl";

pub fn append_jsonl(file_name: &str, record: &Value) {
    if let Err(e) = write_inner(file_name, record) {
        eprintln!("storage: failed to append {}: {}", file_name, e);
    }
}

fn write_inner(file_name: &str, record: &Value) -> std::io::Result<()> {
    create_dir_all(STATE_DIR)?;
    let path = format!("{}/{}", STATE_DIR, file_name);
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(serde_json::to_string(record).unwrap_or_default().as_bytes())?;
    f.write_all(b"\n")
}

pub fn read_jsonl(file_name: &str) -> Vec<Value> {
    let path = format!("{}/{}", STATE_DIR, file_name);
    if !Path::new(&path).exists() {
        return Vec::new();
    }
    let f = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

/// Offline summary over closed paper signals: overall + per-kind hit rates.
pub fn replay_signals() -> Result<(), Box<dyn std::error::Error>> {
    let records = read_jsonl(SIGNALS_FILE);
    let closed: Vec<&Value> = records
        .iter()
        .filter(|r| r["record"].as_str() == Some("signal_closed"))
        .collect();

    if closed.is_empty() {
        println!("no closed paper signals found in state/{}", SIGNALS_FILE);
        return Ok(());
    }

    let mut total = 0usize;
    let mut hits = 0usize;
    let mut by_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();

    println!(
        "{:<12} {:<8} {:>10} {:>10} {:>8}",
        "KIND", "ASSET", "CHG%", "HORIZON", "VALID"
    );
    for r in &closed {
        let kind = r["kind"].as_str().unwrap_or("?").to_string();
        let change_pct = r["change_pct"].as_f64().unwrap_or(0.0) * 100.0;
        let valid = r["validated"].as_bool().unwrap_or(false);
        total += 1;
        if valid {
            hits += 1;
        }
        let e = by_kind.entry(kind.clone()).or_insert((0, 0));
        e.0 += 1;
        if valid {
            e.1 += 1;
        }
        println!(
            "{:<12} {:<8} {:>9.2}% {:>9}s {:>8}",
            kind,
            r["symbol"].as_str().unwrap_or("?"),
            change_pct,
            r["horizon_secs"].as_u64().unwrap_or(0),
            if valid { "YES" } else { "no" },
        );
    }

    let rate = if total > 0 {
        hits as f64 * 100.0 / total as f64
    } else {
        0.0
    };
    println!(
        "\nTotal: {} closed, {} validated ({:.1}% hit rate)",
        total, hits, rate
    );
    for (k, (t, h)) in &by_kind {
        let kr = if *t > 0 {
            *h as f64 * 100.0 / *t as f64
        } else {
            0.0
        };
        println!("  {:<12} {:>3}/{:<3} ({:.1}%)", k, h, t, kr);
    }
    Ok(())
}
