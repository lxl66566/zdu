# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] 0.1.0

### Added

- Initial release: port of dust v1.2.5 with all upstream review bugs fixed
  (junction/symlink cycle dedup with `-L`, EINTR retry bound, DST-midnight
  fallback, `-I`/`-z`/`-M` input validation, JSON size formats for `-f`/`-m`,
  width-overflow guards, dim/ANSI gating).
- Performance: single `stat` per entry with `is_file` carried in `Node`,
  `DirEntry::metadata()` reuse on Windows (zero extra syscall per file),
  per-root parallel walk with scoped cycle detection, incremental indent
  cleaning.
- pdu-inspired hardening: `u128` exact rounding in `human_readable_number`
  with cross-unit carry, bounded `--terminal-width`, lossy JSON serialization
  for non-UTF-8 paths (never panics).
- `--files-from` / `--files0-from`, `--collapse`, `-m` filetime aggregation,
  generated shell completions and man page.
