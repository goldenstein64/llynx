# llynx

> [!WARNING]
>
> This project is still in early development! Tests are only halfway there. Bug reports are appreciated!

a thin [LuaRocks](https://luarocks.org/) wrapper intended for installing addons for [Lua Language Server](https://github.com/LuaLS/lua-language-server)

## Usage

```console
$ llynx help
adds a LuaLS addon using LuaRocks

Usage: llynx.exe [OPTIONS] [COMMAND]

Commands:
  list     List all installed, online, or enabled addons
  install  Install an addon
  remove   Remove an addon
  enable   Enable an addon for the current workspace
  disable  Disable an addon for the current workspace
  help     Print this message or the help of the given subcommand(s)

Options:
  -c, --config <file-path>    configuration file for specifying frequently used flags. Defaults to ".llynx.toml"
  -l, --luarocks <file-path>  Set the path to the LuaRocks executable. Looks on PATH by default
  -t, --tree <dir-path>       Set a custom rocks tree directory. Defaults to "./.lls_addons"
      --settings <file-path>  Modify this settings file. Defaults to "./.vscode/settings.json"
      --server <url>          Make LuaRocks look for addons in this server first. Defaults to "https://luarocks.org/m/lls-addons"
  -v...                       Increase verbosity; can be repeated
  -h, --help                  Print help
```

You can add a `.llynx.toml` to the current directory to save configuration options without adding arguments in the CLI. Here are the defaults, for example.

```toml
luarocks = "luarocks"
tree = ".lls_addons"
settings = ".vscode/settings.json"
server = "https://luarocks.org/m/lls-addons"
```

You can also provide a path to a config file in the CLI using the `-c` or `--config` option.

## Installation

For development, you should [install a Rust toolchain](https://www.rust-lang.org/tools/install). I use `stable-gnu` on Windows.

Run `cargo install` (this is not set up yet).

```bash
cargo install llynx
```

## Building

Run `cargo build`.

```bash
cargo build --release
```

## Testing

Run `cargo test`.

```bash
cargo test
```
