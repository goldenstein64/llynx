# llynx

> [!WARNING]
>
> This project is still in early development! Pretty much everything is untested. Bug reports are appreciated!

a thin [LuaRocks](https://luarocks.org/) wrapper intended for installing addons for [Lua Language Server](https://github.com/LuaLS/lua-language-server)

## Usage

```console
$ llynx help
adds a LuaLS addon using LuaRocks

Usage: llynx.exe [OPTIONS] [COMMAND]

Commands:
  list     list all installed, online, or enabled addons
  install  install an addon
  remove   remove an addon
  enable   enable an addon for the current workspace
  disable  disable an addon for the current workspace
  help     Print this message or the help of the given subcommand(s)

Options:
  -t, --tree <dir-path>       set a custom rocks tree directory [default: .lls_addons]
  -s, --settings <file-path>  modify this settings file [default: .vscode/settings.json]
      --server <url>          make LuaRocks look for addons in this server only [default: https://luarocks.org/m/lls-addons]
  -v...                       increase verbosity; can be repeated
  -h, --help                  Print help
```

For development, you should [install a Rust toolchain](https://www.rust-lang.org/tools/install). I use `stable-gnu` on Windows.

## Installation

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
