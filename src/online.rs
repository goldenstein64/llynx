use crate::Addon;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{io::Cursor, process::Command};

#[derive(Debug, Deserialize)]
struct OnlineAddonRecord {
    pub name: String,
    pub version: String,
    pub file_type: String,

    #[allow(dead_code)]
    pub source: String,
}
/// fetches from luarocks.org
pub fn list_online(server: &str, luarocks_path: &str, filter: Option<&str>) -> Result<Vec<Addon>> {
    let mut luarocks = Command::new(luarocks_path);
    luarocks.args([
        "--only-server",
        server,
        "search",
        "--porcelain",
        filter.unwrap_or("--all"),
    ]);
    log::info!("executing: {luarocks:?}");

    let output = luarocks.output().context("while executing luarocks")?;
    let stdout = std::str::from_utf8(&output.stdout).context("while decoding luarocks output")?;

    // because the CSV reader only reads files, a Cursor represents the string's
    // file handle
    // https://stackoverflow.com/a/41069910/13221687
    let cursor = Cursor::new(stdout.as_bytes());
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(b'\t')
        .from_reader(cursor);

    let addons = reader
        .deserialize::<OnlineAddonRecord>()
        .into_iter()
        .map(|row| row.expect("interpreting LuaRocks output"))
        .filter(|record| record.file_type == "rockspec")
        .map(|record| Addon {
            name: record.name,
            version: record.version,
            location: None,
        })
        .collect();

    Ok(addons)
}
