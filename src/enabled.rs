use crate::{Addon, LIB_SETTINGS_KEY, installed::list_installed};
use anyhow::{Context, Result, anyhow};
use jsonc_parser::{ParseOptions, parse_to_serde_value};
use serde::{Deserialize, Serialize};
use std::{fmt, fs, io, iter, path::Path};

#[cfg(test)]
use crate::SAY_ADDON_LOCATION;

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

/// fetches from .vscode/settings.json
pub fn list_enabled(tree: &str, settings_file: &str, filter: Option<&str>) -> Result<Vec<Addon>> {
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
pub fn enable(tree: &str, luarocks_path: &str, settings_file: &str, name: &str) -> Result<()> {
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
pub fn disable(tree: &str, luarocks_path: &str, settings_file: &str, name: &str) -> Result<()> {
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
