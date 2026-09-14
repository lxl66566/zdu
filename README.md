# zdu

A fast, parallel `du` with an intuitive tree view — the ease of use of
[dust](https://github.com/bootandy/dust) with the throughput of
[pdu](https://github.com/XAMPPRocky/pdu).

```
4.1Gi ┌┴─ target                 100%
 692M └─── release               16%
 682M     ┌┴─ deps               99%
 477M     │ ┌┴─ zdu.d            69%
```

## Features

- Parallel tree walk (rayon), no recursion — large trees scan fast
- Intuitive tree chart with percent bars, biggest entries first
- Default: no symlink follow; hard links counted once; `-L` to follow links
  with cycle-safe dedup
- `-x` stay on one filesystem, `-s` apparent size, `-f` count files, `-m` show
  file times
- `-j` JSON output, `--files-from` / `--files0-from` path lists
- Regex / file-type / min-size / time filters
- Shell completions (bash, zsh, fish, elvish, PowerShell) and a man page,
  generated at build time
- Config file (`~/.zdu.toml`) for defaults

## Usage

```sh
zdu                        # current directory
zdu -d 2 /usr              # limit depth
zdu -n 10 -d 3 ~           # 10 largest entries
zdu -F .                   # largest files only
zdu -f -d 1 src            # count files instead of bytes
zdu -s /var                # apparent size (file length, not blocks)
zdu -L -x /mnt/data        # follow symlinks, same filesystem only
zdu -e '\.rs$' src         # only .rs files
zdu -v '\.git' .           # exclude .git
zdu -j -d 1 /etc           # JSON output
zdu --files-from list.txt  # scan paths from a file
```

## Install

```sh
cargo install --git https://github.com/lxl66566/zdu
# or from a checkout
cargo install --path .
```

Prebuilt binaries and completions/man page land in `completions/` and
`man-page/` on release builds (via `build.rs`).

## Differences vs dust / pdu

| | dust | pdu | zdu |
|---|---|---|---|
| Output | tree chart | flat/sorted list | tree chart (dust-style) |
| Walker | parallel | parallel, lock-free | parallel (rayon), no recursion |
| Windows stat per entry | yes | yes | none (`DirEntry::metadata()` reuses readdir data) |
| `-j` on non-UTF-8 names | panic | — | lossy, never panics |

zdu started as a port of dust v1.2.5 with the upstream review findings
(junction/symlink cycles, DST panics, JSON size formats, width overflows, …)
fixed, then adopted pdu's performance lessons (single `stat` per entry,
metadata reuse, per-root parallel walk). On a 30k-file tree on Windows it scans
~3x faster than pdu.

## Attribution

- [bootandy/dust](https://github.com/bootandy/dust) — the tree-chart UX and the
  original walker design; zdu is a port of dust v1.2.5.
- [XAMPPRocky/pdu](https://github.com/XAMPPRocky/pdu) — performance
  techniques and regression tests against its reviewed bugs.

Licensed under MIT OR Apache-2.0.
