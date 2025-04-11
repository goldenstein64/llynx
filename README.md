# lynx

a thin [LuaRocks](https://luarocks.org/) wrapper intended for installing addons for [Lua Language Server](https://github.com/LuaLS/lua-language-server)

Lynx has six commands:

```console
$ lynx help
adds a LuaLS addon using LuaRocks

Usage: lynx.exe [OPTIONS] [COMMAND]

Commands:
  list     list all installed addons, or provide a search filter
  install  install an addon
  remove   remove an addon
  enable   enable an addon for the current workspace
  disable  disable an addon for the current workspace
  help     Print this message or the help of the given subcommand(s)

Options:
  -t, --tree <dir>  sets a custom rocks tree directory
  -d, --debug...    turn on debugging
  -h, --help        Print help
```
