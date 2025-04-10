// Plan:
// Try to at least follow the behavior of the current addon manager:
// - `list` -> [addon]
// - `install <name> [version]` -> Result<Version, String>
// - `remove <name>` -> Result<(), String>
// - `update <name> [version]` -> Result<Version, String>
// - `enable <name> [version]` -> Result<(), String>
// - `disable <name>` -> Result<(), String>

// Assumptions:
// - Only one version of an addon can be enabled at any time

use clap::{CommandFactory, Parser, Subcommand};
use jsonc_parser::{ParseOptions, parse_to_serde_value};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self, DirEntry},
    io::{self, Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
};

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
    /// sets a custom rocks tree directory
    #[arg(short, long, value_name = "dir", value_parser = clap::value_parser!(PathBuf))]
    tree: Option<PathBuf>,

    /// turn on debugging
    #[arg(short, long, action = clap::ArgAction::Count)]
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
    /// list all installed addons, or provide a search filter
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

#[derive(Serialize, Deserialize, Debug)]
struct VSCodeSettings {
    #[serde(rename = "Lua.workspace.library")]
    library: Option<Vec<String>>,

    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
enum ListEnabledError {
    IOError {
        source: io::Error,
    },
    ParseError {
        source: jsonc_parser::errors::ParseError,
    },
    CompileError {
        source: serde_json::Error,
    },
}

impl fmt::Display for ListEnabledError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListEnabledError::IOError { source } => {
                write!(f, "error while fetching settings file: {source}").unwrap()
            }
            ListEnabledError::ParseError { source } => {
                write!(f, "error while parsing settings: {source}").unwrap()
            }
            ListEnabledError::CompileError { source } => {
                write!(f, "error while compiling settings: {source}").unwrap()
            }
        }
        Ok(())
    }
}

impl std::error::Error for ListEnabledError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IOError { source } => Some(source),
            Self::ParseError { source } => Some(source),
            Self::CompileError { source } => Some(source),
        }
    }

    fn description(&self) -> &str {
        "description() is deprecated; use Display"
    }

    fn cause(&self) -> Option<&dyn std::error::Error> {
        self.source()
    }
}

/// reads from .vscode/settings.json
fn list_enabled(filter: Option<String>) -> Result<Vec<Addon>, ListEnabledError> {
    let contents = match fs::read_to_string(".vscode/settings.json") {
        Err(source) => match source.kind() {
            io::ErrorKind::NotFound => {
                println!("file '.vscode/settings.json' was not found. Assuming empty...");
                return Ok(vec![]);
            }
            _ => return Err(ListEnabledError::IOError { source }),
        },
        Ok(contents) => contents,
    };

    let vscode_settings_parsed = match parse_to_serde_value(&contents, &ParseOptions::default()) {
        Err(source) => return Err(ListEnabledError::ParseError { source }),
        Ok(parsed) => match parsed {
            None => {
                println!("file '.vscode/settings.json' is empty. Assuming empty...");
                return Ok(vec![]);
            }
            Some(vscode_settings) => vscode_settings,
        },
    };

    let vscode_settings = match serde_json::from_value::<VSCodeSettings>(vscode_settings_parsed) {
        Err(source) => return Err(ListEnabledError::CompileError { source }),
        Ok(vscode_settings) => vscode_settings,
    };

    let library = match vscode_settings.library {
        None => {
            println!("'Lua.workspace.library' key doesn't exist. Assuming empty...");
            return Ok(vec![]);
        }
        Some(lib) => lib,
    };

    let addons = library
        .iter()
        .map(|s| (s, Path::new(s)))
        .filter(|(_, path)| {
            path.is_relative()
                && path.starts_with(".lls_addons/luarocks")
                && path.ends_with("types")
        })
        .map(|(s, path)| {
            // we start at 'types', meaning the version is its parent
            let version = path.parent().expect("path has at least two parents");
            // which is a child of the rock name
            let name = version.parent().expect("path has at least one parent");
            Addon {
                name: name.display().to_string(),
                version: version.display().to_string(),
                location: Some(s.clone()),
            }
        });

    Ok(match filter {
        Some(fil) => addons.filter(|addon| addon.name.contains(&fil)).collect(),
        None => addons.collect(),
    })
}

#[derive(Deserialize)]
struct InstalledRecord {
    pub name: String,
    pub version: String,

    #[allow(dead_code)]
    pub status: String,

    pub location: String,
}

/// reads from the .lls_addons tree
fn list_installed(filter: Option<String>) -> Vec<Addon> {
    let mut luarocks = Command::new("luarocks");
    luarocks.args(["--tree", ".lls_addons", "list", "--porcelain"]);
    if let Some(fil) = filter {
        luarocks.arg(fil);
    }
    let output = luarocks
        .output()
        .unwrap_or_else(|err| panic!("LuaRocks failed to execute: {err}"));

    let stdout = std::str::from_utf8(&output.stdout)
        .unwrap_or_else(|err| panic!("LuaRocks output could not be decoded: {err}"));

    // because the CSV reader only reads files, a Cursor represents the string's
    // file handle
    // https://stackoverflow.com/a/41069910/13221687
    let cursor = Cursor::new(stdout.as_bytes());
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(b'\t')
        .from_reader(cursor);

    reader
        .deserialize::<InstalledRecord>()
        .into_iter()
        .map(|row| row.expect("interpreting LuaRocks output"))
        .map(|record| Addon {
            name: record.name,
            version: record.version,
            location: Some(record.location),
        })
        .collect()
}

#[derive(Deserialize)]
struct AddonRecord {
    pub name: String,
    pub version: String,
    pub file_type: String,

    #[allow(dead_code)]
    pub source: String,
}

/// reads from luarocks.org
fn list_online(filter: Option<String>) -> Vec<Addon> {
    let mut luarocks = Command::new("luarocks");
    luarocks.args([
        "--only-server",
        "https://luarocks.org/m/lls-addons",
        "search",
        "--porcelain",
    ]);
    if let Some(fil) = filter {
        luarocks.arg(fil);
    } else {
        luarocks.arg("--all");
    }
    let output = luarocks
        .output()
        .unwrap_or_else(|err| panic!("LuaRocks failed to execute: {err}"));

    let stdout = std::str::from_utf8(&output.stdout)
        .unwrap_or_else(|err| panic!("LuaRocks output could not be decoded: {err}"));

    // because the CSV reader only reads files, a Cursor represents the string's
    // file handle
    // https://stackoverflow.com/a/41069910/13221687
    let cursor = Cursor::new(stdout.as_bytes());
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(b'\t')
        .from_reader(cursor);

    reader
        .deserialize::<AddonRecord>()
        .into_iter()
        .map(|row| row.expect("interpreting LuaRocks output"))
        .filter(|record| record.file_type == "rockspec")
        .map(|record| Addon {
            name: record.name,
            version: record.version,
            location: None,
        })
        .collect()
}

// - list every addon featured on the LuaRocks lls-addons manifest
// - list every addon installed
// - list every addon enabled
fn list(source: Option<ListSource>, filter: Option<String>) -> Vec<Addon> {
    let used_source = match source {
        Some(src) => src,
        None => ListSource::Installed,
    };

    // get a list of addons from here
    match used_source {
        ListSource::Enabled => list_enabled(filter).unwrap(),
        ListSource::Installed => list_installed(filter),
        ListSource::Online => list_online(filter),
    }
}

/// forward installing to LuaRocks
fn install(name: String, version: Option<String>) -> () {
    let mut command = Command::new("luarocks");
    command.args(["--tree", ".lls_addons", "install", &name]);
    if let Some(ver) = version {
        command.arg(ver);
    }
    let result_output = command.output();
    match result_output {
        Ok(output) => {
            io::stdout().write_all(&output.stdout).unwrap();
            io::stderr().write_all(&output.stderr).unwrap();
        }
        Err(err) => {
            io::stderr().write_all(format!("{err}").as_bytes()).unwrap();
        }
    }
}

/// forward uninstalling to LuaRocks
fn remove(name: String, version: Option<String>) -> () {
    let mut command = Command::new("luarocks");
    command.args(["--tree", ".lls_addons", "remove", &name]);
    if let Some(ver) = version {
        command.arg(ver);
    }
    let result_output = command.output();
    match result_output {
        Ok(output) => {
            io::stdout().write_all(&output.stdout).unwrap();
            io::stderr().write_all(&output.stderr).unwrap();
        }
        Err(err) => {
            io::stderr().write_all(format!("{err}").as_bytes()).unwrap();
        }
    }
}

/// add the addon to .vscode/settings.json
fn enable(name: String) {
    todo!()
}

/// remove the addon from .vscode/settings.json
fn disable(name: String) {
    todo!()
}

fn main() -> () {
    let parsed = Cli::parse();

    match parsed.command {
        None => Cli::command().print_help().unwrap(),
        Some(action) => match action {
            Action::List { source, filter } => {
                let mut addons = list(source, filter);
                if addons.is_empty() {
                    println!("no addons found matching criteria");
                    return;
                }
                addons.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
                let mut last_addon: &Addon = addons.first().expect("already checked if it's empty");
                println!("{}", last_addon.name);
                println!("\t{}", last_addon.version);
                for addon in addons.iter() {
                    if last_addon.name != addon.name {
                        last_addon = &addon;
                        println!("\n{}", addon.name);
                    }
                    println!("\t{}", addon.version);
                }
            }
            Action::Install { name, version } => install(name, version),
            Action::Remove { name, version } => remove(name, version),
            Action::Enable { name } => {
                // add the
            }
            Action::Disable { name } => {}
        },
    }
}
