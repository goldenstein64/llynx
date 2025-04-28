# servers/ setup
if [[ -d servers ]]; then
    rm -r servers
fi

mkdir servers
cd servers

mkdir empty
luarocks-admin make-manifest empty

mkdir one_addon
luarocks-admin make-manifest one_addon

luarocks-admin add --server=file://./one_addon \
    ../assets/say-1.4.1-3.rockspec

luarocks-admin add --server=file://./one_addon \
    ../assets/say-1.4.1-3.src.rock

cd ..

# trees/ setup
if [[ -d trees ]]; then
    rm -r trees
fi

mkdir trees
cd trees

luarocks config lua_version 5.1
luarocks --only-server=file://../servers/one_addon install --tree one_addon say 1.4.1-3

# say doesn't have types, but we'll pretend it does by creating a types folder
mkdir one_addon/lib/luarocks/rocks-5.1/say/1.4.1-3/types

cd ..