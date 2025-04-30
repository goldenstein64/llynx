// Plan:
// Try to at least follow the behavior of the current addon manager:
// - `list` -> Result<Vec<Addon>, Error>
// - `install <name> [version]` -> Result<Version, Error>
// - `remove <name>` -> Result<(), Error>
// - `enable <name>` -> Result<(), Error>
// - `disable <name>` -> Result<(), Error>

// Assumptions:
// - Only one version of an addon can be enabled at any time

mod enabled;
mod installed;
mod online;

use crate::enabled::{disable, enable, list_enabled};
use crate::installed::{install, list_installed, remove};
use crate::online::list_online;
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use serde::Deserialize;
use std::{fs, io};
use toml;

#[cfg(test)]
use std::sync::LazyLock;

const CONFIG_PATH: &str = ".llynx.toml";
const LUAROCKS_PATH: &str = "luarocks";
const ADDONS_DIR: &str = ".lls_addons";
const LUAROCKS_ENDPOINT: &str = "https://luarocks.org/m/lls-addons";
const SETTINGS_FILE: &str = ".vscode/settings.json";
const LIB_SETTINGS_KEY: &str = "Lua.workspace.library";

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug)]
struct Config<'a> {
    luarocks: &'a str,
    tree: &'a str,
    settings: &'a str,
    server: &'a str,
    verbose: u8,
}

impl<'a> Default for Config<'a> {
    fn default() -> Self {
        Config {
            luarocks: LUAROCKS_PATH,
            tree: ADDONS_DIR,
            settings: SETTINGS_FILE,
            server: LUAROCKS_ENDPOINT,
            verbose: 0,
        }
    }
}

impl<'a> Config<'a> {
    fn extend<'b: 'a>(self, maybe_config: &'b MaybeConfig) -> Self {
        fn choose_str<'a>(try_str: &'a Option<String>, otherwise: &'a str) -> &'a str {
            try_str.as_ref().map(String::as_str).unwrap_or(otherwise)
        }

        let MaybeConfig {
            schema: _,
            luarocks,
            tree,
            settings,
            server,
            verbose,
        } = maybe_config;
        Config {
            luarocks: choose_str(luarocks, self.luarocks),
            tree: choose_str(tree, self.tree),
            settings: choose_str(settings, self.settings),
            server: choose_str(server, self.server),
            verbose: verbose.unwrap_or(self.verbose),
        }
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
    command: Option<Command>,
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
enum Command {
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

fn get_cli_config_file_overrides(path: &str) -> Result<MaybeConfig> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("while opening config file '{path}'"))?;
    toml::from_str::<MaybeConfig>(&contents)
        .with_context(|| format!("while parsing config file '{path}'"))
}

fn get_default_config_file_overrides() -> Option<Result<MaybeConfig>> {
    match fs::read_to_string(CONFIG_PATH) {
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound => {
                log::debug!("default config file not found, using defaults...");
                None
            }
            _ => Some(
                Err(anyhow::Error::from(err))
                    .with_context(|| format!("while opening config file '{CONFIG_PATH}'")),
            ),
        },
        Ok(contents) => Some(
            toml::from_str::<MaybeConfig>(&contents)
                .with_context(|| format!("while parsing config file '{CONFIG_PATH}'")),
        ),
    }
}

fn get_file_overrides(path: Option<&str>) -> Result<Option<MaybeConfig>> {
    path.map(get_cli_config_file_overrides)
        .or_else(get_default_config_file_overrides)
        .transpose()
}

fn run_command(action: Option<Command>, config: Config) -> Result<()> {
    let Config {
        luarocks,
        tree,
        settings,
        server,
        verbose,
    } = config;

    stderrlog::new()
        .timestamp(stderrlog::Timestamp::Off)
        .verbosity(verbose as usize)
        .init()?;

    match action {
        None => Cli::command().print_help().unwrap(),
        Some(action) => match action {
            Command::List { source, filter } => {
                let filter = filter.as_ref().map(String::as_str);
                let addons = match source.unwrap_or(ListSource::Installed) {
                    ListSource::Enabled => list_enabled(tree, settings, filter),
                    ListSource::Installed => list_installed(tree, luarocks, filter),
                    ListSource::Online => list_online(server, luarocks, filter),
                }
                .context("while listing addons")?;

                print_addons_list(addons);
            }
            Command::Install { name, version } => {
                let version = version.as_ref().map(String::as_str);
                install(tree, luarocks, &name, version)?;
            }
            Command::Remove { name, version } => {
                let version = version.as_ref().map(String::as_str);
                #[cfg(feature = "disable_before_remove")]
                {
                    log::info!("disabling '{name}' first...");
                    disable(&tree, &luarocks, &settings, &name)
                        .with_context(|| format!("while disabling '{name}' before uninstalling"))?;
                }
                remove(tree, luarocks, &name, version)?;
            }
            Command::Enable { name } => enable(tree, luarocks, settings, &name)?,
            Command::Disable { name } => disable(tree, luarocks, settings, &name)?,
        },
    };

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // config should be calculated like this:
    // (CLI args) overrides (Config args) overrides (defaults)
    let default_config = Config::default();
    let file_overrides: Option<MaybeConfig> =
        get_file_overrides(cli.config.as_ref().map(String::as_str))?;
    let cli_overrides = MaybeConfig {
        schema: None,
        luarocks: cli.luarocks,
        tree: cli.tree,
        settings: cli.settings,
        server: cli.server,
        verbose: match cli.verbose {
            0 => None,
            _ => Some(cli.verbose),
        },
    };

    let config = match file_overrides {
        Some(ref overrides) => default_config.extend(overrides),
        None => default_config,
    }
    .extend(&cli_overrides);

    run_command(cli.command, config)?;

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
mod test_config {
    use super::*;

    #[test]
    fn empty() {
        let empty_config = get_cli_config_file_overrides("tests/configs/empty.toml").unwrap();
        assert_eq!(empty_config.schema, None);
        assert_eq!(empty_config.luarocks, None);
        assert_eq!(empty_config.server, None);
        assert_eq!(empty_config.settings, None);
        assert_eq!(empty_config.tree, None);
        assert_eq!(empty_config.verbose, None);
    }

    #[test]
    fn schema() {
        let schema_config =
            get_cli_config_file_overrides("tests/configs/empty_schema.toml").unwrap();
        assert_eq!(schema_config.schema, Some(String::from("")));
        assert_eq!(schema_config.luarocks, None);
        assert_eq!(schema_config.server, None);
        assert_eq!(schema_config.settings, None);
        assert_eq!(schema_config.tree, None);
        assert_eq!(schema_config.verbose, None);
    }

    #[test]
    fn some_args() {
        let some_config = get_cli_config_file_overrides("tests/configs/some_args.toml").unwrap();
        assert_eq!(some_config.schema, Some(String::from("some_schema")));
        assert_eq!(some_config.luarocks, Some(String::from("some_luarocks")));
        assert_eq!(some_config.tree, Some(String::from("some_tree")));
        assert_eq!(some_config.server, None);
        assert_eq!(some_config.settings, None);
        assert_eq!(some_config.verbose, None);
    }

    #[test]
    fn all_args() {
        let all_config = get_cli_config_file_overrides("tests/configs/all_args.toml").unwrap();
        assert_eq!(all_config.schema, Some(String::from("some_schema")));
        assert_eq!(all_config.luarocks, Some(String::from("some_luarocks")));
        assert_eq!(all_config.server, Some(String::from("some_server")));
        assert_eq!(all_config.settings, Some(String::from("some_settings")));
        assert_eq!(all_config.tree, Some(String::from("some_tree")));
        assert_eq!(all_config.verbose, Some(8));
    }

    #[test]
    fn test_illegal() {
        get_cli_config_file_overrides("tests/configs/illegal.toml").unwrap_err();
    }
}

#[cfg(test)]
mod test_config_extend {
    use super::*;

    #[test]
    fn default() {
        let default_config = Config::default();
        assert_eq!(default_config.luarocks, LUAROCKS_PATH);
        assert_eq!(default_config.server, LUAROCKS_ENDPOINT);
        assert_eq!(default_config.settings, SETTINGS_FILE);
        assert_eq!(default_config.tree, ADDONS_DIR);
        assert_eq!(default_config.verbose, 0);
    }

    #[test]
    fn extend_some_args() {
        let some_config = get_cli_config_file_overrides("tests/configs/some_args.toml").unwrap();
        let config = Config::default().extend(&some_config);
        assert_eq!(config.luarocks, String::from("some_luarocks"));
        assert_eq!(config.tree, String::from("some_tree"));
        assert_eq!(config.server, LUAROCKS_ENDPOINT);
        assert_eq!(config.settings, SETTINGS_FILE);
        assert_eq!(config.verbose, 0);
    }

    #[test]
    fn extend_all_args() {
        let all_config = get_cli_config_file_overrides("tests/configs/all_args.toml").unwrap();
        let config = Config::default().extend(&all_config);
        assert_eq!(config.luarocks, String::from("some_luarocks"));
        assert_eq!(config.server, String::from("some_server"));
        assert_eq!(config.settings, String::from("some_settings"));
        assert_eq!(config.tree, String::from("some_tree"));
        assert_eq!(config.verbose, 8);
    }
}

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
