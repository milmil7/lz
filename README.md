# lz

An advanced `ls` alternative with interactive browsing.

## Features

- Fast directory listing with colors
- Long format output (`-l`)
- Tree view (`--tree`)
- Optional icons (`--icons`)
- Optional rainbow coloring (`--rainbow`)
- Filter entries with a glob pattern (`--filter`)
- Only show directories or files (`--only-dirs`, `--only-files`)
- Optional summary: total bytes and per-extension breakdown (`--du`, `--extensions`)
- JSON output for scripting (`--json`)
- Watch mode that refreshes output (`--watch`)
- Interactive TUI browser (`interactive` subcommand)
- Folder picker mode (`fastls` subcommand)

## Installation

### From source (Cargo)

```bash
cargo install --path .
```

Or run without installing:

```bash
cargo run -- <PATH>
```

## Usage

### Basic listing

```bash
lz .
lz C:\Windows
```

### Options

```bash
lz -a --icons --rainbow .
lz -l --human .
lz --sort size .
lz --sort age --reverse .
lz --filter "**/*.rs" .
lz --only-dirs .
lz --only-files .
```

### Tree view

```bash
lz --tree .
lz --tree --only-dirs .
lz --tree --filter "**/*.toml" .
```

### Summary output

Total size summary:

```bash
lz --du .
```

Per-extension stats:

```bash
lz --extensions .
```

Both together:

```bash
lz --du --extensions .
```

### JSON output

```bash
lz --json .
lz --json --du --extensions .
```

The JSON includes:

- `root`: listing root path
- `entries`: array of entries with name, kind, size, modified time, and relative path
- `summary`: optional totals and per-extension stats (when requested)
- `error`: optional error string (used by watch mode if a refresh fails)

### Watch mode

Refreshes the listing every 2 seconds.

```bash
lz --watch .
lz --watch --json .
```

## Interactive mode

Launches a TUI browser for navigating directories and viewing a summary for the selected entry.

```bash
lz interactive
lz interactive .
```

Keys:

- Up/Down: move selection
- Enter: open directory / show file summary
- Backspace: go up to parent directory
- h: toggle hidden entries
- r: refresh
- q or Esc: quit

## fastls

Opens a native folder picker and then prints the listing for the selected folder.

```bash
lz fastls
```

## Sorting

`--sort` supports:

- `name` (default)
- `size`
- `age` (aliases: `time`, `mtime`)

## Notes

- Executable highlighting is based on file extension (`.exe`, `.bat`, `.cmd`).
- Filter patterns use glob syntax via `globset` (for example: `**/*.rs`).

