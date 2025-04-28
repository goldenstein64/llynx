// Plan:
// Try to at least follow the behavior of the current addon manager:
// - `list` -> Result<Vec<Addon>, Error>
// - `install <name> [version]` -> Result<Version, Error>
// - `remove <name>` -> Result<(), Error>
// - `enable <name>` -> Result<(), Error>
// - `disable <name>` -> Result<(), Error>

// Assumptions:
// - Only one version of an addon can be enabled at any time

use anyhow::{Context, Result, anyhow};
use clap::{CommandFactory, Parser, Subcommand};
use jsonc_parser::{ParseOptions, parse_to_serde_value};
use serde::{Deserialize, Serialize};
use std::{
    env, fmt, fs,
    io::{self, Cursor, Write},
    iter,
    path::{Path, PathBuf},
    process::Command,
};
use toml;

#[cfg(test)]
use std::sync::LazyLock;

const CONFIG_PATH: &str = ".llynx.toml";
const LUAROCKS_PATH: &str = "luarocks";
const ADDONS_DIR: &str = ".lls_addons";
const LUAROCKS_ENDPOINT: &str = "https://luarocks.org/m/lls-addons";
const SETTINGS_FILE: &str = ".vscode/settings.json";
const LIB_SETTINGS_KEY: &str = "Lua.workspace.library";

#[derive(Debug, Serialize, Deserialize)]
struct VSCodeSettings {
    #[serde(rename = "Lua.workspace.library")]
    library: Option<Vec<String>>,

    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl Default for VSCodeSettings {
    fn default() -> Self {
        VSCodeSettings {
            library: None,
            rest: serde_json::Map::new(),
        }
    }
}

#[derive(Deserialize, Debug)]
struct MaybeConfig {
    #[allow(dead_code)]
    #[serde(rename = "$schema")]
    schema: Option<String>, // this is unused
    luarocks: Option<String>,
    tree: Option<String>,
    settings: Option<String>,
    server: Option<String>,
    verbose: Option<u8>,
}

impl From<&Cli> for MaybeConfig {
    fn from(cli: &Cli) -> Self {
        MaybeConfig {
            schema: None,
            luarocks: cli.luarocks.clone(),
            tree: cli.luarocks.clone(),
            settings: cli.settings.clone(),
            server: cli.server.clone(),
            verbose: match cli.verbose {
                0 => None,
                v => Some(v),
            },
        }
    }
}

#[derive(Debug)]
struct Config {
    luarocks: String,
    tree: String,
    settings: String,
    server: String,
    verbose: u8,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            luarocks: String::from(LUAROCKS_PATH),
            tree: String::from(ADDONS_DIR),
            settings: String::from(SETTINGS_FILE),
            server: String::from(LUAROCKS_ENDPOINT),
            verbose: 0,
        }
    }
}

impl Config {
    fn extend(self, maybe_config: MaybeConfig) -> Self {
        let MaybeConfig {
            schema: _,
            luarocks,
            tree,
            settings,
            server,
            verbose,
        } = maybe_config;
        Config {
            luarocks: luarocks.unwrap_or(self.luarocks),
            tree: tree.unwrap_or(self.tree),
            settings: settings.unwrap_or(self.settings),
            server: server.unwrap_or(self.server),
            verbose: verbose.unwrap_or(self.verbose),
        }
    }
}

/// error type for showing multiple errors
#[derive(Debug)]
struct AggregateError(Vec<anyhow::Error>);

impl AggregateError {
    pub fn from_results<T: fmt::Debug>(results: impl Iterator<Item = Result<T>>) -> Result<Vec<T>> {
        let (oks, errs) =
            results.partition::<Vec<Result<T>>, fn(&Result<T>) -> bool>(|result| result.is_ok());
        match errs.len() {
            0 => Ok(oks.into_iter().map(|ok| ok.unwrap()).collect()),
            1 => Err(errs
                .into_iter()
                .nth(0)
                .expect("errs is non-empty")
                .expect_err("result was partitioned into an err")),
            _ => Err(AggregateError(errs.into_iter().map(|err| err.unwrap_err()).collect()).into()),
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

#[derive(Debug, PartialEq, Eq, Clone)]
struct Addon {
    name: String,
    version: String,
    location: Option<String>,
}

/// adds a LuaLS addon using LuaRocks
#[derive(Debug, Parser)]
#[command(long_about = None)]
struct Cli {
    /// configuration file for specifying frequently used flags. Defaults to ".llynx.toml"
    #[arg(short, long, value_name = "file-path")]
    config: Option<String>,

    /// Set the path to the LuaRocks executable. Looks on PATH by default
    #[arg(short, long, value_name = "file-path")]
    luarocks: Option<String>,

    /// Set a custom rocks tree directory. Defaults to "./.lls_addons"
    #[arg(short, long, value_name = "dir-path")]
    tree: Option<String>,

    /// Modify this settings file. Defaults to "./.vscode/settings.json"
    #[arg(long, value_name = "file-path")]
    settings: Option<String>,

    /// Make LuaRocks look for addons in this server first. Defaults to "https://luarocks.org/m/lls-addons"
    #[arg(long, value_name = "url")]
    server: Option<String>,

    /// Increase verbosity; can be repeated
    #[arg(short, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
enum ListSource {
    /// List every addon in the LuaRocks manifest
    Online,

    /// List every installed addon
    Installed,

    /// List every enabled addon
    Enabled,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
enum Action {
    /// List all installed, online, or enabled addons
    List {
        #[command(subcommand)]
        source: Option<ListSource>,

        /// Only include addons with this string in their names
        #[arg(short, long)]
        filter: Option<String>,
    },

    /// Install an addon
    Install {
        /// The addon to install
        name: String,
        /// The version to install
        version: Option<String>,
    },

    /// Remove an addon
    Remove {
        /// The addon to remove
        name: String,
        /// The specific version of addon to remove
        version: Option<String>,
    },

    /// Enable an addon for the current workspace
    Enable {
        /// The addon to enable
        name: String,
    },

    /// Disable an addon for the current workspace
    Disable {
        /// The addon to disable
        name: String,
    },
}

/// fetches from .vscode/settings.json
fn list_enabled(tree: &str, settings_file: &str, filter: Option<&str>) -> Result<Vec<Addon>> {
    let contents = match fs::read_to_string(settings_file) {
        Err(source) => match source.kind() {
            io::ErrorKind::NotFound => {
                log::warn!("file '{settings_file}' was not found. Assuming empty...");
                return Ok(vec![]);
            }
            _ => return Err(source).with_context(|| format!("while reading '{settings_file}'")),
        },
        Ok(contents) => contents,
    };

    let maybe_value_parsed = parse_to_serde_value(&contents, &ParseOptions::default())
        .with_context(|| format!("while parsing '{settings_file}'"))?;
    let value_parsed = match maybe_value_parsed {
        None => {
            log::warn!("file '{settings_file}' is empty. Assuming empty...");
            return Ok(vec![]);
        }
        Some(vscode_settings_parsed) => vscode_settings_parsed,
    };

    let vscode_settings = serde_json::from_value::<VSCodeSettings>(value_parsed)
        .with_context(|| format!("while compiling '{settings_file}'"))?;

    let library = match vscode_settings.library {
        None => {
            log::warn!("key '{LIB_SETTINGS_KEY}' not found. Assuming empty...");
            return Ok(vec![]);
        }
        Some(lib) => lib,
    };

    let addons_matcher = format!("{tree}/lib/luarocks");

    let addons_unfiltered: Vec<Addon> = AggregateError::from_results(
        library
            .into_iter()
            .map(|s| s)
            .filter(|s| {
                let path = Path::new(s);
                path.is_relative() && path.starts_with(&addons_matcher) && path.ends_with("types")
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
                        .expect("directory starts with addons_matcher")
                        .to_str()
                        .ok_or(anyhow!("version directory is not valid UTF-8"))?
                        .to_string(),
                    version: version
                        .file_name()
                        .expect("directory starts with addons_matcher")
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

#[derive(Debug, Deserialize)]
struct InstalledAddonRecord {
    pub name: String,
    pub version: String,

    #[allow(dead_code)]
    pub status: String,

    pub location: String,
}

/// fetches from the .lls_addons tree
fn list_installed(tree: &str, luarocks_path: &str, filter: Option<&str>) -> Result<Vec<Addon>> {
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

#[derive(Debug, Deserialize)]
struct OnlineAddonRecord {
    pub name: String,
    pub version: String,
    pub file_type: String,

    #[allow(dead_code)]
    pub source: String,
}
/// fetches from luarocks.org
fn list_online(server: &str, luarocks_path: &str, filter: Option<&str>) -> Result<Vec<Addon>> {
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

fn print_addons_list(mut addons: Vec<Addon>) -> () {
    if addons.is_empty() {
        log::error!("no addons found matching criteria");
        return;
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
fn install(tree: &str, luarocks_path: &str, name: &str, version: Option<&str>) -> Result<()> {
    let mut install_command = Command::new(luarocks_path);
    install_command.args(["--tree", tree, "install", name]);
    if let Some(ver) = version {
        install_command.arg(ver);
    }
    execute_command(install_command)
}

/// forward uninstalling to LuaRocks
fn remove(tree: &str, luarocks_path: &str, name: &str, version: Option<&str>) -> Result<()> {
    let mut remove_command = Command::new(luarocks_path);
    remove_command.args(["--tree", tree, "remove", name]);
    if let Some(ver) = version {
        remove_command.arg(ver);
    }
    execute_command(remove_command)
}

/// read from a settings file and write to it again
fn update_library(settings_file: &str, f: impl FnOnce(Vec<String>) -> Vec<String>) -> Result<()> {
    let contents = match fs::read_to_string(settings_file) {
        Err(source) => match source.kind() {
            io::ErrorKind::NotFound => String::new(),
            _ => return Err(source).with_context(|| format!("while reading '{settings_file}'")),
        },
        Ok(contents) => contents,
    };

    let maybe_value_parsed = parse_to_serde_value(&contents, &ParseOptions::default())
        .with_context(|| format!("while parsing '{settings_file}'"))?;
    let mut vscode_settings = match maybe_value_parsed {
        None => VSCodeSettings::default(),
        Some(value_parsed) => serde_json::from_value::<VSCodeSettings>(value_parsed)
            .with_context(|| format!("while compiling '{settings_file}'"))?,
    };

    vscode_settings.library = Some(f(vscode_settings.library.unwrap_or_default()));

    let new_contents: String = serde_json::to_string(&vscode_settings)?;
    fs::write(settings_file, new_contents)?;
    Ok(())
}

fn get_addon_path(tree: &str, luarocks_path: &str, name: &str) -> Result<String> {
    let addon = list_installed(tree, luarocks_path, Some(name))?
        .into_iter()
        .find(|addon| addon.name == name)
        .ok_or_else(|| anyhow!("addon '{name}' is not installed"))?;

    let location = addon
        .location
        .expect("installed addons always have a location");

    let path = Path::new(&location);
    let types_path = path.join("types");
    types_path
        .into_os_string()
        .into_string()
        .map_err(|os_str| anyhow!("string {os_str:?} is not valid UTF-8"))
}

fn enable_in_library(path: String) -> impl FnOnce(Vec<String>) -> Vec<String> {
    move |library| {
        library
            .into_iter()
            .chain(iter::once(path)) // append this path to the end
            .collect()
    }
}

/// add the addon to .vscode/settings.json
fn enable(tree: &str, luarocks_path: &str, settings_file: &str, name: &str) -> Result<()> {
    if list_enabled(tree, settings_file, Some(name))?
        .into_iter()
        .any(|addon| addon.name == name)
    {
        log::info!("addon '{name}' is already enabled");
        return Ok(());
    }

    let addon_to_enable = get_addon_path(tree, luarocks_path, name)?;
    update_library(settings_file, enable_in_library(addon_to_enable))
}

fn disable_in_library(path: &str) -> impl FnOnce(Vec<String>) -> Vec<String> {
    move |library| library.into_iter().filter(|loc| loc != path).collect()
}

/// remove the addon from .vscode/settings.json
fn disable(tree: &str, luarocks_path: &str, settings_file: &str, name: &str) -> Result<()> {
    if list_enabled(tree, settings_file, Some(name))?
        .into_iter()
        .any(|addon| addon.name != name)
    {
        log::info!("addon '{name}' is already disabled");
        return Ok(());
    }

    let addon_to_disable = get_addon_path(tree, luarocks_path, name)?;
    update_library(settings_file, disable_in_library(&addon_to_disable))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // config should be calculated like this:
    // CLI args -> Config arg -> defaults
    let default_config = Config::default();
    let cli_overrides = MaybeConfig::from(&cli);
    let Config {
        luarocks,
        tree,
        settings,
        server,
        verbose,
    } = match cli.config {
        None => {
            log::debug!("config file not given, attempting to read from '{CONFIG_PATH}'...");
            match fs::read_to_string(CONFIG_PATH) {
                Err(err) => match err.kind() {
                    io::ErrorKind::NotFound => {
                        log::debug!("default config file not found, using defaults...");
                        default_config.extend(cli_overrides)
                    }
                    _ => {
                        return Err(anyhow::Error::from(err))
                            .with_context(|| format!("while opening config file '{CONFIG_PATH}'"));
                    }
                },
                Ok(contents) => {
                    let maybe_config = toml::from_str::<MaybeConfig>(&contents)
                        .with_context(|| format!("while parsing config file '{CONFIG_PATH}'"))?;
                    default_config.extend(maybe_config).extend(cli_overrides)
                }
            }
        }
        Some(config_path) => {
            let contents = fs::read_to_string(&config_path)
                .with_context(|| format!("while opening config file '{config_path}'"))?;
            let maybe_config = toml::from_str::<MaybeConfig>(&contents)
                .with_context(|| format!("while parsing config file '{config_path}'"))?;
            default_config.extend(maybe_config).extend(cli_overrides)
        }
    };

    stderrlog::new()
        .timestamp(stderrlog::Timestamp::Off)
        .verbosity(verbose as usize)
        .init()?;

    match cli.command {
        None => Cli::command().print_help().unwrap(),
        Some(action) => match action {
            Action::List { source, filter } => {
                let filter = filter.as_ref().map(String::as_str);
                let addons = match source.unwrap_or(ListSource::Installed) {
                    ListSource::Enabled => list_enabled(&tree, &settings, filter),
                    ListSource::Installed => list_installed(&tree, &luarocks, filter),
                    ListSource::Online => list_online(&server, &luarocks, filter),
                }
                .context("while listing addons")?;

                print_addons_list(addons);
            }
            Action::Install { name, version } => {
                let version = version.as_ref().map(String::as_str);
                install(&tree, &luarocks, &name, version)?;
            }
            Action::Remove { name, version } => {
                let version = version.as_ref().map(String::as_str);
                #[cfg(feature = "disable_before_remove")]
                {
                    log::info!("disabling '{name}' first...");
                    disable(&tree, &luarocks, &settings, &name)
                        .with_context(|| format!("while disabling '{name}' before uninstalling"))?;
                }
                remove(&tree, &luarocks, &name, version)?;
            }
            Action::Enable { name } => enable(&tree, &luarocks, &settings, &name)?,
            Action::Disable { name } => disable(&tree, &luarocks, &settings, &name)?,
        },
    }

    Ok(())
}

#[cfg(all(test, windows))]
static SAY_ADDON_LOCATION: &str =
    "tests\\trees\\one_addon\\lib\\luarocks\\rocks-5.1\\say\\1.4.1-3\\types";
#[cfg(all(test, unix))]
static SAY_ADDON_LOCATION: &str = "tests/trees/one_addon/lib/luarocks/rocks-5.1/say/1.4.1-3/types";

#[cfg(test)]
static SAY_ADDON: LazyLock<Addon, fn() -> Addon> = LazyLock::new(|| Addon {
    name: String::from("say"),
    version: String::from("1.4.1-3"),
    location: Some(String::from(SAY_ADDON_LOCATION)),
});

#[cfg(test)]
static ONLINE_SAY_ADDON: LazyLock<Addon, fn() -> Addon> = LazyLock::new(|| Addon {
    name: String::from("say"),
    version: String::from("1.4.1-3"),
    location: None,
});

#[cfg(test)]
mod test_list_online {
    use super::*;

    #[test]
    fn one_addon() {
        let addons =
            list_online("file://./tests/servers/one_addon", "luarocks", Some("say")).unwrap();
        assert_eq!(addons, vec![ONLINE_SAY_ADDON.clone()]);
    }

    #[test]
    fn empty() {
        let addons = list_online("file://./tests/servers/empty", "luarocks", Some("say")).unwrap();
        assert_eq!(addons, vec![]);
    }
}

#[cfg(test)]
mod test_list_installed {
    use super::*;

    #[test]
    fn one_addon() {
        let addons = list_installed("tests/trees/one_addon", "luarocks", None).unwrap();
        assert_eq!(addons, vec![SAY_ADDON.clone()]);
    }
}

#[cfg(test)]
mod test_list_enabled {
    use super::*;

    #[test]
    fn not_found() {
        let addons =
            list_enabled("tests/trees/one_addon", "tests/settings/fake.json", None).unwrap();
        assert_eq!(addons, vec![]);
    }

    #[test]
    fn empty() {
        let addons =
            list_enabled("tests/trees/one_addon", "tests/settings/empty.json", None).unwrap();
        assert_eq!(addons, vec![]);
    }

    #[test]
    fn no_library() {
        let addons = list_enabled(
            "tests/trees/one_addon",
            "tests/settings/no_library.json",
            None,
        )
        .unwrap();
        assert_eq!(addons, vec![]);
    }

    #[test]
    fn empty_library() {
        let addons = list_enabled(
            "tests/trees/one_addon",
            "tests/settings/no_library.json",
            None,
        )
        .unwrap();
        assert_eq!(addons, vec![]);
    }

    #[test]
    fn one_addon() {
        let addons = list_enabled(
            "tests/trees/one_addon",
            #[cfg(windows)]
            "tests/settings/one_addon_windows.json",
            #[cfg(unix)]
            "tests/settings/one_addon_linux.json",
            None,
        )
        .unwrap();
        assert_eq!(addons, vec![SAY_ADDON.clone()])
    }
}

#[cfg(test)]
mod test_enable {
    use super::*;

    #[test]
    fn add_from_empty() {
        let library: Vec<String> = vec![];
        let new_path = String::from(SAY_ADDON_LOCATION);
        let func = enable_in_library(new_path);
        let new_library = func(library);
        assert_eq!(new_library, vec![String::from(SAY_ADDON_LOCATION)]);
    }
}

#[cfg(test)]
mod test_disable {
    use super::*;

    #[test]
    fn remove_from_one() {
        let library = vec![String::from(SAY_ADDON_LOCATION)];
        let func = disable_in_library(SAY_ADDON_LOCATION);
        let new_library = func(library);
        assert_eq!(new_library, vec![] as Vec<String>);
    }
}
