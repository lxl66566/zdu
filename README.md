# zdu

Yet another fast, parallel `du` — the ease of use of [dust](https://github.com/bootandy/dust) with throughput better than [pdu](https://github.com/XAMPPRocky/pdu).

```
6.9Mi         ┌── winapi                                     │█▓▓▓▓▓▓▓▓▓▓ │   5%
7.0Mi         │   ┌── out                                    │█▓▓▓▓▓▓▓▓▓▓ │   5%
7.0Mi         │ ┌─┴ 69da0266b9157236                         │█▓▓▓▓▓▓▓▓▓▓ │   5%
7.0Mi         ├─┴ regex-syntax                               │█▓▓▓▓▓▓▓▓▓▓ │   5%
7.1Mi         ├── serde_core                                 │█▓▓▓▓▓▓▓▓▓▓ │   5%
7.6Mi         ├── zdu                                        │█▓▓▓▓▓▓▓▓▓▓ │   5%
8.9Mi         ├── windows-sys                                │█▓▓▓▓▓▓▓▓▓▓ │   6%
6.3Mi         │     ┌── libclap_builder-3e8656942985ccb9.rlib│█▓▓▓▓▓▓▓▓▓▓ │   4%
8.3Mi         │   ┌─┴ out                                    │█▓▓▓▓▓▓▓▓▓▓ │   6%
8.3Mi         │ ┌─┴ 3e8656942985ccb9                         │█▓▓▓▓▓▓▓▓▓▓ │   6%
 12Mi         ├─┴ clap_builder                               │█▓▓▓▓▓▓▓▓▓▓ │   9%
6.5Mi         │   ┌── out                                    │█▓▓▓▓▓▓▓▓▓▓ │   5%
6.5Mi         │ ┌─┴ 0c4feb0c82d5fce5                         │█▓▓▓▓▓▓▓▓▓▓ │   5%
6.8Mi         │ │   ┌── libsyn-396b9df21564193d.rlib         │█▓▓▓▓▓▓▓▓▓▓ │   5%
9.5Mi         │ │ ┌─┴ out                                    │█▓▓▓▓▓▓▓▓▓▓ │   7%
9.5Mi         │ ├─┴ 396b9df21564193d                         │█▓▓▓▓▓▓▓▓▓▓ │   7%
 16Mi         ├─┴ syn                                        │██▓▓▓▓▓▓▓▓▓ │  11%
137Mi       ┌─┴ build                                        │███████████ │  97%
141Mi     ┌─┴ release                                        │███████████ │ 100%
141Mi   ┌─┴ target                                           │███████████ │ 100%
142Mi ┌─┴ .                                                  │███████████ │ 100%
```

## Features

- Parallel tree walk (rayon)
- Intuitive tree chart with percent bars
- Default: no symlink follow; hard links counted once; `-L` to follow links with cycle-safe dedup
- `-x` stay on one filesystem, `-s` apparent size, `-f` count files, `-m` show file times
- Windows default mode reports the on-disk size for sparse/cloud-placeholder files (where the file length is wildly off); plain and NTFS-compressed files report the file length, since du-style allocated size has no cheap per-file API on Windows
- `-j` JSON output, `--files-from` / `--files0-from` path lists
- Regex / file-type / min-size / time filters
- Config file (`~/.config/zdu/config.toml`) for defaults

## Install

Choose one:

- Manual install from the latest [Releases](https://github.com/lxl66566/zdu/Releases)
- use [bpm](https://github.com/lxl66566/bpm-rs): `bpm i https://github.com/lxl66566/zdu`
- NixOS user: install from my [NUR](https://github.com/lxl66566/NUR)
- compile from source: `cargo install --git https://github.com/lxl66566/zdu`

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

## Benchmark

Measured with [hyperfine](https://github.com/sharkdp/hyperfine) on Linux (32 cores, warm page cache, output redirected to `/dev/null`), against dust 1.2.4 and pdu 0.21.1. pdu runs with `-H` (`--deduplicate-hardlinks`) so all three tools deduplicate hard links the same way zdu and dust do by default:

| Tree                      | zdu      | dust     | pdu `-H` | zdu vs dust      | zdu vs pdu      |
| ------------------------- | -------- | -------- | -------- | ---------------- | --------------- |
| `~/.cargo` (57k files)    | 20.6 ms  | 222.8 ms | 14.3 ms  | **10.8x faster** | 1.44x slower    |
| `~/programs` (482k files) | 141.2 ms | 393.9 ms | 218.9 ms | **2.8x faster**  | **1.6x faster** |
| `/nix/store` (900k files) | 309.5 ms | 940.1 ms | 1.088 s  | **3.0x faster**  | **3.5x faster** |

zdu renders a full tree chart (like dust) while pdu prints a flat sorted list, so zdu does strictly more output work per run; pdu's hard-link dedup scales poorly on link-heavy trees (`/nix/store`), where zdu pulls ahead by 3.5x.

## Differences vs dust / pdu

|                         | dust       | pdu                 | zdu                                               |
| ----------------------- | ---------- | ------------------- | ------------------------------------------------- |
| Output                  | tree chart | flat/sorted list    | tree chart (dust-style)                           |
| Walker                  | parallel   | parallel, lock-free | parallel (rayon), no recursion                    |
| Windows stat per entry  | yes        | yes                 | none (`DirEntry::metadata()` reuses readdir data) |
| `-j` on non-UTF-8 names | panic      | —                   | lossy, never panics                               |

zdu started as a port of dust v1.2.5 with multiple bug fixed, then adopted pdu's performance lessons.

## Attribution

- [bootandy/dust](https://github.com/bootandy/dust) — the tree-chart UX and the original walker design; zdu is a port of dust v1.2.5.
- [XAMPPRocky/pdu](https://github.com/XAMPPRocky/pdu) — performance techniques and regression tests against its reviewed bugs.

Licensed under MIT OR Apache-2.0.
