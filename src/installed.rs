use crate::Addon;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    env,
    io::{self, Cursor, Write},
    path::PathBuf,
    process::Command,
};

#[derive(Debug, Deserialize)]
struct InstalledAddonRecord {
    pub name: String,
    pub version: String,

    #[allow(dead_code)]
    pub status: String,

    pub location: String,
}

/// fetches from the .lls_addons tree
pub fn list_installed(tree: &str, luarocks_path: &str, filter: Option<&str>) -> Result<Vec<Addon>> {
    let mut luarocks = Command::new(luarocks_path);
    luarocks.args(["--tree", tree, "list", "--porcelain"]);
    if let Some(fil) = filter {
        luarocks.arg(fil);
    }
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

    let cwd = env::current_dir()?;

    let addons = reader
        .deserialize::<InstalledAddonRecord>()
        .into_iter()
        .map(|row| row.expect("interpreting LuaRocks output"))
        .map(|record| {
            let name = record.name;
            let version = record.version;
            let location = record.location;
            let mut path = PathBuf::from(location);
            path.push(&name);
            path.push(&version);
            path.push("types");
            let relative_path = path.strip_prefix(&cwd).unwrap_or(&path);
            Addon {
                name,
                version,
                location: Some(
                    relative_path
                        .to_str()
                        .expect("path is not UTF-8")
                        .to_string(),
                ),
            }
        })
        .collect();

    Ok(addons)
}

fn execute_command(mut command: Command) -> Result<()> {
    log::info!("executing: {command:?}");

    let result_output = command.output();
    match result_output {
        Ok(output) => {
            io::stdout()
                .write_all(&output.stdout)
                .context("while writing out stdout")?;
            io::stderr()
                .write_all(&output.stderr)
                .context("while writing to stderr")?;
        }
        Err(err) => {
            io::stderr()
                .write_all(format!("{err}").as_bytes())
                .context("while writing to stderr")?;
        }
    }

    Ok(())
}

/// forward installing to LuaRocks
pub fn install(tree: &str, luarocks_path: &str, name: &str, version: Option<&str>) -> Result<()> {
    let mut install_command = Command::new(luarocks_path);
    install_command.args(["--tree", tree, "install", name]);
    if let Some(ver) = version {
        install_command.arg(ver);
    }
    execute_command(install_command)
}

/// forward uninstalling to LuaRocks
pub fn remove(tree: &str, luarocks_path: &str, name: &str, version: Option<&str>) -> Result<()> {
    let mut remove_command = Command::new(luarocks_path);
    remove_command.args(["--tree", tree, "remove", name]);
    if let Some(ver) = version {
        remove_command.arg(ver);
    }
    execute_command(remove_command)
}
