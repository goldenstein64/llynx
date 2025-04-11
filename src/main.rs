// Plan:
// Try to at least follow the behavior of the current addon manager:
// - `list` -> Result<Vec<Addon>, Error>
// - `install <name> [version]` -> Result<Version, Error>
// - `remove <name>` -> Result<(), Error>
// - `enable <name>` -> Result<(), Error>
// - `disable <name>` -> Result<(), Error>

// Assumptions:
// - Only one version of an addon can be enabled at any time

use anyhow::{Context, Result, anyhow, bail};
use clap::{CommandFactory, Parser, Subcommand};
use jsonc_parser::{ParseOptions, parse_to_serde_value};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self},
    io::{self, Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
};

const ADDONS_DIR: &str = ".lls_addons";
const ADDONS_MATCHER: &str = ".lls_addons/lib/luarocks";
const LUAROCKS_ENDPOINT: &str = "https://luarocks.org/m/lls-addons";
const SETTINGS_FILE: &str = ".vscode/settings.json";
const LIB_SETTINGS_KEY: &str = "Lua.workspace.library";

#[derive(Serialize, Deserialize, Debug)]
struct VSCodeSettings {
    #[serde(rename = "Lua.workspace.library")]
    library: Option<Vec<String>>,

    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

/// error type for showing multiple errors
#[derive(Debug)]
struct AggregateError(Vec<anyhow::Error>);

impl AggregateError {
    pub fn from_results<T: fmt::Debug>(
        results: impl Iterator<Item = Result<T>>,
    ) -> Result<Vec<T>, anyhow::Error> {
        let (oks, errs) =
            results.partition::<Vec<Result<T>>, fn(&Result<T>) -> bool>(|result| result.is_ok());
        if errs.len() > 1 {
            Err(AggregateError(errs.into_iter().map(|err| err.unwrap_err()).collect()).into())
        } else if !errs.is_empty() {
            Err(errs
                .into_iter()
                .nth(0)
                .expect("errs is non-empty")
                .expect_err("result was partitioned into an err"))
        } else {
            Ok(oks.into_iter().map(|ok| ok.unwrap()).collect())
        }
    }
}

impl fmt::Display for AggregateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for source in &self.0 {
            writeln!(f, "{source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AggregateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0[0].as_ref())
    }
}

#[derive(Debug)]
struct Addon {
    name: String,
    version: String,
    location: Option<String>,
}

/// adds a LuaLS addon using LuaRocks
#[derive(Parser, Debug)]
#[command(name = "lady", long_about = None)]
struct Cli {
    /// set a custom rocks tree directory
    #[arg(short, long, value_name = "dir", default_value = ".lls_addons", value_parser = clap::value_parser!(PathBuf))]
    tree: Option<PathBuf>,

    /// turn on debugging
    #[arg(short, action = clap::ArgAction::Count)]
    debug: u8,

    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand, PartialEq, Eq, Debug)]
enum ListSource {
    /// list every addon in the LuaRocks manifest
    Online,

    /// list every installed addon
    Installed,

    /// list every enabled addon
    Enabled,
}

#[derive(Subcommand, PartialEq, Eq, Debug)]
enum Action {
    /// list all installed, online, or enabled addons
    List {
        #[command(subcommand)]
        source: Option<ListSource>,

        /// only include addons with this string in their names
        #[arg(short, long)]
        filter: Option<String>,
    },

    /// install an addon
    Install {
        /// the addon to install
        name: String,
        /// the version to install
        version: Option<String>,
    },

    /// remove an addon
    Remove {
        /// the addon to remove
        name: String,
        /// the specific version of addon to remove
        version: Option<String>,
    },

    /// enable an addon for the current workspace
    Enable {
        /// the addon to enable
        name: String,
    },

    /// disable an addon for the current workspace
    Disable {
        /// the addon to disable
        name: String,
    },
}

/// fetches from .vscode/settings.json
fn list_enabled(filter: Option<String>) -> Result<Vec<Addon>> {
    let contents = match fs::read_to_string(SETTINGS_FILE) {
        Err(source) => match source.kind() {
            io::ErrorKind::NotFound => {
                println!("file '{SETTINGS_FILE}' was not found. Assuming empty...");
                return Ok(vec![]);
            }
            _ => return Err(source).with_context(|| format!("reading '{SETTINGS_FILE}' failed")),
        },
        Ok(contents) => contents,
    };

    let vscode_settings_parsed = match parse_to_serde_value(&contents, &ParseOptions::default()) {
        Err(source) => {
            return Err(source).with_context(|| format!("parsing '{SETTINGS_FILE}' failed"));
        }
        Ok(parsed) => match parsed {
            None => {
                println!("file '{SETTINGS_FILE}' is empty. Assuming empty...");
                return Ok(vec![]);
            }
            Some(vscode_settings) => vscode_settings,
        },
    };

    let vscode_settings = match serde_json::from_value::<VSCodeSettings>(vscode_settings_parsed) {
        Err(source) => {
            return Err(source).with_context(|| format!("compiling '{SETTINGS_FILE}' failed"));
        }
        Ok(vscode_settings) => vscode_settings,
    };

    let library = match vscode_settings.library {
        None => {
            println!("'{LIB_SETTINGS_KEY}' key doesn't exist. Assuming empty...");
            return Ok(vec![]);
        }
        Some(lib) => lib,
    };

    let addons_unfiltered: Vec<Addon> = AggregateError::from_results(
        library
            .into_iter()
            .map(|s| s)
            .filter(|s| {
                let path = Path::new(s);
                path.is_relative() && path.starts_with(ADDONS_MATCHER) && path.ends_with("types")
            })
            .map(|s| {
                let path = Path::new(&s);
                // we start at 'types', meaning the version is its parent
                let version = path.parent().expect("path has at least two parents");
                // which is a child of the rock name
                let name = version.parent().expect("path has at least one parent");
                Ok(Addon {
                    name: name
                        .file_name()
                        .expect("version must be a directory")
                        .to_str()
                        .ok_or(anyhow!("version directory is not valid UTF-8"))?
                        .to_string(),
                    version: version
                        .file_name()
                        .expect("version must be a directory")
                        .to_str()
                        .ok_or(anyhow!("version directory is not valid UTF-8"))?
                        .to_string(),
                    location: Some(s),
                })
            }),
    )?;

    let addons = match filter {
        Some(fil) => addons_unfiltered
            .into_iter()
            .filter(|addon: &Addon| addon.name.contains(&fil))
            .collect(),
        None => addons_unfiltered,
    };

    Ok(addons)
}

#[derive(Deserialize, Debug)]
struct InstalledAddonRecord {
    pub name: String,
    pub version: String,

    #[allow(dead_code)]
    pub status: String,

    pub location: String,
}

/// fetches from the .lls_addons tree
fn list_installed(filter: Option<String>) -> Result<Vec<Addon>> {
    let mut luarocks = Command::new("luarocks");
    luarocks.args(["--tree", ADDONS_DIR, "list", "--porcelain"]);
    if let Some(fil) = filter {
        luarocks.arg(fil);
    }

    let output = luarocks.output().context("execution of luarocks failed")?;

    let stdout =
        std::str::from_utf8(&output.stdout).context("decoding of luarocks output failed")?;

    // because the CSV reader only reads files, a Cursor represents the string's
    // file handle
    // https://stackoverflow.com/a/41069910/13221687
    let cursor = Cursor::new(stdout.as_bytes());
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(b'\t')
        .from_reader(cursor);

    let addons = reader
        .deserialize::<InstalledAddonRecord>()
        .into_iter()
        .map(|row| row.expect("interpreting LuaRocks output"))
        .map(|record| Addon {
            name: record.name,
            version: record.version,
            location: Some(record.location),
        })
        .collect();

    Ok(addons)
}

#[derive(Deserialize, Debug)]
struct OnlineAddonRecord {
    pub name: String,
    pub version: String,
    pub file_type: String,

    #[allow(dead_code)]
    pub source: String,
}

/// fetches from luarocks.org
fn list_online(filter: Option<String>) -> Result<Vec<Addon>> {
    let mut luarocks = Command::new("luarocks");
    luarocks.args(["--only-server", LUAROCKS_ENDPOINT, "search", "--porcelain"]);
    if let Some(fil) = filter {
        luarocks.arg(fil);
    } else {
        luarocks.arg("--all");
    }
    let output = luarocks.output().context("execution of luarocks failed")?;
    let stdout =
        std::str::from_utf8(&output.stdout).context("decoding of luarocks output failed")?;

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

// - list every addon featured on the LuaRocks lls-addons manifest
// - list every addon installed
// - list every addon enabled
fn list(source: Option<ListSource>, filter: Option<String>) -> Result<Vec<Addon>> {
    let used_source = match source {
        Some(src) => src,
        None => ListSource::Installed,
    };

    // get a list of addons from here
    match used_source {
        ListSource::Enabled => list_enabled(filter),
        ListSource::Installed => list_installed(filter),
        ListSource::Online => list_online(filter),
    }
}

/// forward installing to LuaRocks
fn install(name: String, version: Option<String>) -> Result<()> {
    let mut command = Command::new("luarocks");
    command.args(["--tree", ".lls_addons", "install", &name]);
    if let Some(ver) = version {
        command.arg(ver);
    }
    let result_output = command.output();
    match result_output {
        Ok(output) => {
            io::stdout()
                .write_all(&output.stdout)
                .context("error while writing out stdout")?;
            io::stderr()
                .write_all(&output.stderr)
                .context("error while writing to stderr")?;
        }
        Err(err) => {
            io::stderr()
                .write_all(format!("{err}").as_bytes())
                .context("error while writing to stderr")?;
        }
    }

    Ok(())
}

/// forward uninstalling to LuaRocks
fn remove(name: String, version: Option<String>) -> Result<()> {
    let mut command = Command::new("luarocks");
    command.args(["--tree", ".lls_addons", "remove", &name]);
    if let Some(ver) = version {
        command.arg(ver);
    }
    let result_output = command.output();
    match result_output {
        Ok(output) => {
            io::stdout()
                .write_all(&output.stdout)
                .context("error while writing to stdout")?;
            io::stderr()
                .write_all(&output.stderr)
                .context("error while writing to stderr")?;
        }
        Err(err) => {
            io::stderr()
                .write_all(format!("{err}").as_bytes())
                .context("error while writing to stderr")?;
        }
    }

    Ok(())
}

/// add the addon to .vscode/settings.json
fn enable(name: String) -> Result<()> {
    if list_enabled(None)?
        .into_iter()
        .any(|addon| addon.name == name)
    {
        bail!("addon '{name}' is already installed")
    }

    let addon = list_installed(None)?
        .into_iter()
        .find(|addon| addon.name == name);

    if let None = addon {
        bail!("addon '{name}' is not installed")
    }

    todo!()
}

/// remove the addon from .vscode/settings.json
fn disable(name: String) -> Result<()> {
    todo!()
}

fn main() -> Result<()> {
    let parsed = Cli::parse();

    match parsed.command {
        None => Cli::command().print_help().unwrap(),
        Some(action) => match action {
            Action::List { source, filter } => {
                let mut addons = list(source, filter)?;
                if addons.is_empty() {
                    return Err(anyhow!("no addons found matching criteria"));
                }
                addons.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
                let mut last_addon: &Addon = addons.first().expect("already checked if it's empty");
                println!("{}", last_addon.name);
                println!("\t{}", last_addon.version);
                for addon in addons.iter().skip(1) {
                    if last_addon.name != addon.name {
                        last_addon = &addon;
                        println!("\n{}", addon.name);
                    }
                    println!("\t{}", addon.version);
                }
            }
            Action::Install { name, version } => install(name, version)?,
            Action::Remove { name, version } => remove(name, version)?,
            Action::Enable { name } => enable(name)?,
            Action::Disable { name } => disable(name)?,
        },
    }

    Ok(())
}
