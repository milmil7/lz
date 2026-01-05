use std::{
    cmp::Ordering,
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    hash::{Hash, Hasher},
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
    time::SystemTime,
};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use crossterm::{
    ExecutableCommand,
    cursor::MoveTo,
    terminal::{Clear, ClearType},
};
use cursive::{
    Cursive,
    event::{Event, Key},
    theme::{BaseColor, Color, PaletteColor, Theme},
    traits::{Nameable, Resizable, Scrollable},
    views::{
        Dialog, DummyView, LinearLayout, Panel, ResizedView, ScrollView, SelectView, TextView,
    },
};
use globset::{Glob, GlobMatcher};
use owo_colors::OwoColorize;
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(
    name = "lz",
    version,
    about = "An advanced ls alternative with interactive browsing.",
    subcommand_precedence_over_arg = true
)]
struct Cli {
    #[command(flatten)]
    options: ListOptions,

    #[command(subcommand)]
    command: Option<Command>,

    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Interactive(InteractiveArgs),
    Fastls,
}

#[derive(Args, Debug)]
struct InteractiveArgs {
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
struct ListOptions {
    #[arg(global = true, short = 'a', long = "all")]
    all: bool,

    #[arg(global = true, short = 'l', long = "long")]
    long: bool,

    #[arg(global = true, long = "icons")]
    icons: bool,

    #[arg(global = true, long = "tree")]
    tree: bool,

    #[arg(global = true, long = "rainbow")]
    rainbow: bool,

    #[arg(global = true, long = "filter", value_name = "PATTERN")]
    filter: Option<String>,

    #[arg(global = true, long = "only-dirs")]
    only_dirs: bool,

    #[arg(global = true, long = "only-files")]
    only_files: bool,

    #[arg(global = true, long = "json")]
    json: bool,

    #[arg(global = true, long = "du")]
    du: bool,

    #[arg(global = true, long = "extensions")]
    extensions: bool,

    #[arg(global = true, long = "watch")]
    watch: bool,

    #[arg(global = true, long = "human")]
    human: bool,

    #[arg(global = true, long = "sort", value_enum, default_value_t = SortKey::Name)]
    sort: SortKey,

    #[arg(global = true, short = 'r', long = "reverse")]
    reverse: bool,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Name,
    Size,
    #[value(alias = "time", alias = "mtime")]
    Age,
}

#[derive(Debug, Clone)]
struct EntryInfo {
    name: OsString,
    path: PathBuf,
    file_type: fs::FileType,
    metadata: fs::Metadata,
    modified: Option<SystemTime>,
}

impl EntryInfo {
    fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    fn is_symlink(&self) -> bool {
        self.file_type.is_symlink()
    }

    fn size(&self) -> u64 {
        if self.file_type.is_file() {
            self.metadata.len()
        } else {
            0
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{}", format!("{err:#}").bright_red());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Interactive(args)) => {
            let start = args.path.unwrap_or_else(|| PathBuf::from("."));
            run_interactive(start, cli.options)?;
        }
        Some(Command::Fastls) => {
            run_fastls(cli.options)?;
        }
        None => {
            let path = cli.path.unwrap_or_else(|| PathBuf::from("."));
            list_path(&path, &cli.options)?;
        }
    }

    Ok(())
}

fn run_fastls(options: ListOptions) -> Result<()> {
    let picked = rfd::FileDialog::new()
        .set_title("Choose a folder to list")
        .pick_folder();
    if let Some(dir) = picked {
        list_path(&dir, &options)?;
    }
    Ok(())
}

fn list_path(path: &Path, options: &ListOptions) -> Result<()> {
    if options.watch {
        loop {
            if !options.json {
                let mut stdout = io::stdout();
                stdout.execute(Clear(ClearType::All))?;
                stdout.execute(MoveTo(0, 0))?;
            }

            if let Err(err) = list_path_once(path, options) {
                if options.json {
                    let out = JsonOutput {
                        root: path.display().to_string(),
                        entries: Vec::new(),
                        summary: None,
                        error: Some(format!("{err:#}")),
                    };
                    println!("{}", serde_json::to_string(&out)?);
                } else {
                    eprintln!("{}", format!("{err:#}").bright_red());
                }
            }

            if options.json {
                io::stdout().flush()?;
            }

            thread::sleep(Duration::from_secs(2));
        }
    } else {
        list_path_once(path, options)
    }
}

fn list_path_once(path: &Path, options: &ListOptions) -> Result<()> {
    let matcher = compile_filter(options)?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to read metadata for {}", path.display()))?;

    let summary = if options.du || options.extensions {
        Some(compute_summary(path, options, matcher.as_ref())?)
    } else {
        None
    };

    if metadata.is_dir() {
        let entries = build_display_entries_for_dir(path, path, options, matcher.as_ref())?;
        output_entries(path, &entries, summary.as_ref(), options)
    } else {
        let file_type = metadata.file_type();
        let name = path
            .file_name()
            .map(OsString::from)
            .unwrap_or_else(|| OsString::from(path.as_os_str()));
        let entry = EntryInfo {
            name,
            path: path.to_path_buf(),
            file_type,
            modified: metadata.modified().ok(),
            metadata,
        };
        let rel_path = path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf());
        let display = DisplayEntry {
            entry,
            prefix: String::new(),
            rel_path,
        };
        output_entries(path, &[display], summary.as_ref(), options)
    }
}

fn output_entries(
    root: &Path,
    entries: &[DisplayEntry],
    summary: Option<&ListingSummary>,
    options: &ListOptions,
) -> Result<()> {
    if options.json {
        let out = JsonOutput {
            root: root.display().to_string(),
            entries: entries.iter().map(|e| e.to_json()).collect(),
            summary: summary.map(|s| s.to_json(options.extensions)),
            error: None,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if options.long {
        print_long(entries, options)?;
    } else {
        for entry in entries {
            let prefix = if entry.prefix.is_empty() {
                String::new()
            } else {
                format!("{}", entry.prefix.bright_black())
            };
            println!(
                "{prefix}{}",
                format_name(&entry.entry, &entry.rel_path, options)
            );
        }
    }

    if let Some(summary) = summary {
        if options.du {
            println!(
                "{} {}",
                "Total:".bright_yellow(),
                format_size(summary.total_bytes, true).bright_yellow()
            );
        }
        if options.extensions {
            for (ext, s) in &summary.ext {
                let ext_label = if ext.is_empty() {
                    "(none)".to_string()
                } else {
                    format!(".{ext}")
                };
                let files = format!("{} files", s.files);
                let bytes = format_size(s.bytes, true);
                println!(
                    "{}  {}  {}",
                    ext_label.bright_blue(),
                    files.bright_white(),
                    bytes.bright_magenta()
                );
            }
        }
    }

    Ok(())
}

fn read_entries(dir: &Path, all: bool) -> Result<Vec<EntryInfo>> {
    let mut out = Vec::new();
    let read_dir =
        fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))?;
    for entry in read_dir {
        let entry = entry?;
        let name = entry.file_name();
        if !all && is_hidden(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        out.push(EntryInfo {
            name,
            path,
            file_type,
            modified: metadata.modified().ok(),
            metadata,
        });
    }
    Ok(out)
}

fn is_hidden(name: &OsString) -> bool {
    let s = name.to_string_lossy();
    s.starts_with('.')
}

fn sort_entries(entries: &mut [EntryInfo], key: SortKey, reverse: bool) {
    entries.sort_by(|a, b| {
        let dir_cmp = b.is_dir().cmp(&a.is_dir());
        if dir_cmp != Ordering::Equal {
            return dir_cmp;
        }

        let cmp = match key {
            SortKey::Name => a
                .name
                .to_string_lossy()
                .to_lowercase()
                .cmp(&b.name.to_string_lossy().to_lowercase()),
            SortKey::Size => b.size().cmp(&a.size()),
            SortKey::Age => b.modified.cmp(&a.modified),
        };

        if reverse { cmp.reverse() } else { cmp }
    });
}

#[derive(Debug, Clone)]
struct DisplayEntry {
    entry: EntryInfo,
    prefix: String,
    rel_path: PathBuf,
}

fn compile_filter(options: &ListOptions) -> Result<Option<GlobMatcher>> {
    let Some(pattern) = options.filter.as_deref() else {
        return Ok(None);
    };
    let glob = Glob::new(pattern).with_context(|| format!("Invalid glob: {pattern}"))?;
    Ok(Some(glob.compile_matcher()))
}

fn build_display_entries_for_dir(
    dir: &Path,
    root: &Path,
    options: &ListOptions,
    matcher: Option<&GlobMatcher>,
) -> Result<Vec<DisplayEntry>> {
    if options.tree {
        let md = fs::symlink_metadata(dir)?;
        let root_entry = EntryInfo {
            name: dir
                .file_name()
                .map(OsString::from)
                .unwrap_or_else(|| OsString::from(dir.as_os_str())),
            path: dir.to_path_buf(),
            file_type: md.file_type(),
            modified: md.modified().ok(),
            metadata: md,
        };

        let mut out = Vec::new();
        let root_rel = PathBuf::from(".");
        if !options.only_files {
            out.push(DisplayEntry {
                entry: root_entry,
                prefix: String::new(),
                rel_path: root_rel,
            });
        }
        let mut ancestor_more = Vec::new();
        collect_tree_children(dir, root, options, matcher, &mut ancestor_more, &mut out)?;
        Ok(out)
    } else {
        let mut entries = read_entries(dir, options.all)?;
        sort_entries(&mut entries, options.sort, options.reverse);
        let mut out = Vec::new();
        for entry in entries {
            let rel_path = entry
                .path
                .strip_prefix(root)
                .unwrap_or(&entry.path)
                .to_path_buf();

            if !should_print_entry(&entry, &rel_path, options, matcher) {
                continue;
            }

            out.push(DisplayEntry {
                entry,
                prefix: String::new(),
                rel_path,
            });
        }
        Ok(out)
    }
}

fn collect_tree_children(
    dir: &Path,
    root: &Path,
    options: &ListOptions,
    matcher: Option<&GlobMatcher>,
    ancestor_more: &mut Vec<bool>,
    out: &mut Vec<DisplayEntry>,
) -> Result<bool> {
    let mut entries = read_entries(dir, options.all)?;
    sort_entries(&mut entries, options.sort, options.reverse);

    let mut printable: Vec<(EntryInfo, PathBuf, bool)> = Vec::new();
    for entry in entries {
        let rel_path = entry
            .path
            .strip_prefix(root)
            .unwrap_or(&entry.path)
            .to_path_buf();
        let child_has = if entry.is_dir() {
            subtree_has_printables(&entry.path, root, options, matcher)?
        } else {
            false
        };
        let direct = should_print_entry(&entry, &rel_path, options, matcher);
        let context = entry.is_dir() && !options.only_files && child_has;
        if direct || context {
            printable.push((entry, rel_path, child_has));
        }
    }

    let total = printable.len();
    let mut any_printed = false;
    for (idx, (entry, rel_path, _child_has)) in printable.into_iter().enumerate() {
        let is_last = idx + 1 == total;
        let prefix = tree_prefix(ancestor_more, is_last);
        out.push(DisplayEntry {
            entry: entry.clone(),
            prefix,
            rel_path: rel_path.clone(),
        });
        any_printed = true;

        if entry.is_dir() {
            ancestor_more.push(!is_last);
            collect_tree_children(&entry.path, root, options, matcher, ancestor_more, out)?;
            ancestor_more.pop();
        }
    }

    Ok(any_printed)
}

fn tree_prefix(ancestor_more: &[bool], is_last: bool) -> String {
    let mut s = String::new();
    for &more in ancestor_more {
        if more {
            s.push_str("│   ");
        } else {
            s.push_str("    ");
        }
    }
    if is_last {
        s.push_str("└── ");
    } else {
        s.push_str("├── ");
    }
    s
}

fn subtree_has_printables(
    dir: &Path,
    root: &Path,
    options: &ListOptions,
    matcher: Option<&GlobMatcher>,
) -> Result<bool> {
    let entries = read_entries(dir, options.all)?;
    for entry in entries {
        let rel_path = entry
            .path
            .strip_prefix(root)
            .unwrap_or(&entry.path)
            .to_path_buf();
        if should_print_entry(&entry, &rel_path, options, matcher) {
            return Ok(true);
        }
        if entry.is_dir() && subtree_has_printables(&entry.path, root, options, matcher)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn should_print_entry(
    entry: &EntryInfo,
    rel_path: &Path,
    options: &ListOptions,
    matcher: Option<&GlobMatcher>,
) -> bool {
    if options.only_dirs && !entry.is_dir() {
        return false;
    }
    if options.only_files && entry.is_dir() {
        return false;
    }

    let Some(matcher) = matcher else {
        return true;
    };
    matcher.is_match(normalize_match_path(rel_path))
}

fn normalize_match_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn print_long(entries: &[DisplayEntry], options: &ListOptions) -> Result<()> {
    let mut mode_w = 0usize;
    let mut size_w = 0usize;
    let mut time_w = 0usize;

    let mut rows = Vec::with_capacity(entries.len());
    for entry in entries {
        let mode_raw = format_mode(&entry.entry);
        let size_raw = format_size(entry.entry.size(), options.human);
        let time_raw = entry
            .entry
            .modified
            .map(humantime::format_rfc3339)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());

        let prefix = if entry.prefix.is_empty() {
            String::new()
        } else {
            format!("{}", entry.prefix.bright_black())
        };
        let name = format!(
            "{prefix}{}",
            format_name(&entry.entry, &entry.rel_path, options)
        );

        mode_w = mode_w.max(mode_raw.len());
        size_w = size_w.max(size_raw.len());
        time_w = time_w.max(time_raw.len());

        let mode = format!("{}", mode_raw.bright_yellow());
        let size = format!("{}", size_raw.bright_magenta());
        let time = format!("{}", time_raw.bright_black());
        rows.push((mode_raw, mode, size_raw, size, time_raw, time, name));
    }

    for (mode_raw, mode, size_raw, size, time_raw, time, name) in rows {
        println!(
            "{mode:>mode_w$}  {size:>size_w$}  {time:>time_w$}  {name}",
            mode_w = mode_w,
            size_w = size_w,
            time_w = time_w
        );
        let _ = (&mode_raw, &size_raw, &time_raw);
    }

    Ok(())
}

fn format_mode(entry: &EntryInfo) -> String {
    let type_char = if entry.is_dir() {
        'd'
    } else if entry.is_symlink() {
        'l'
    } else {
        '-'
    };

    let writable = if entry.metadata.permissions().readonly() {
        '-'
    } else {
        'w'
    };

    format!("{type_char}r{writable}")
}

fn format_size(size: u64, human: bool) -> String {
    if !human {
        return size.to_string();
    }
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut f = size as f64;
    let mut idx = 0usize;
    while f >= 1024.0 && idx + 1 < UNITS.len() {
        f /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{size} {}", UNITS[idx])
    } else {
        format!("{:.1} {}", f, UNITS[idx])
    }
}

fn format_name(entry: &EntryInfo, rel_path: &Path, options: &ListOptions) -> String {
    let name = entry.name.to_string_lossy();
    let icon = if options.icons {
        if entry.is_dir() {
            "📁 "
        } else if entry.is_symlink() {
            "🔗 "
        } else {
            "📄 "
        }
    } else {
        ""
    };

    let suffix = if entry.is_dir() {
        std::path::MAIN_SEPARATOR.to_string()
    } else {
        String::new()
    };

    let full = format!("{icon}{name}{suffix}");

    if options.rainbow {
        let (r, g, b) = rainbow_rgb(rel_path);
        return format!("{}", full.truecolor(r, g, b));
    }

    if entry.is_dir() {
        format!("{}", full.bright_blue())
    } else if entry.is_symlink() {
        format!("{}", full.bright_cyan())
    } else if is_probably_executable(&entry.path) {
        format!("{}", full.bright_green())
    } else {
        format!("{}", full.bright_white())
    }
}

fn is_probably_executable(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(ext.to_ascii_lowercase().as_str(), "exe" | "bat" | "cmd")
}

fn rainbow_rgb(path: &Path) -> (u8, u8, u8) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalize_match_path(path).hash(&mut hasher);
    let h = hasher.finish();
    let r = (h & 0xFF) as u8;
    let g = ((h >> 8) & 0xFF) as u8;
    let b = ((h >> 16) & 0xFF) as u8;
    let r = 64u8.saturating_add(r % 160);
    let g = 64u8.saturating_add(g % 160);
    let b = 64u8.saturating_add(b % 160);
    (r, g, b)
}

#[derive(Debug, Clone, Default)]
struct ListingSummary {
    total_bytes: u64,
    total_files: u64,
    total_dirs: u64,
    ext: BTreeMap<String, ExtSummary>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct ExtSummary {
    files: u64,
    bytes: u64,
}

fn compute_summary(
    path: &Path,
    options: &ListOptions,
    matcher: Option<&GlobMatcher>,
) -> Result<ListingSummary> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        let file_type = metadata.file_type();
        let entry = EntryInfo {
            name: path
                .file_name()
                .map(OsString::from)
                .unwrap_or_else(|| OsString::from(path.as_os_str())),
            path: path.to_path_buf(),
            file_type,
            modified: metadata.modified().ok(),
            metadata,
        };
        let rel = path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf());
        let mut summary = ListingSummary::default();
        if should_print_entry(&entry, &rel, options, matcher) {
            summary.total_files = 1;
            summary.total_bytes = entry.size();
            add_extension_stat(&mut summary, &entry);
        }
        return Ok(summary);
    }

    let mut summary = ListingSummary::default();
    walk_summary_dir(path, path, options, matcher, &mut summary)?;
    Ok(summary)
}

fn walk_summary_dir(
    dir: &Path,
    root: &Path,
    options: &ListOptions,
    matcher: Option<&GlobMatcher>,
    summary: &mut ListingSummary,
) -> Result<()> {
    let entries = read_entries(dir, options.all)?;
    for entry in entries {
        let rel_path = entry
            .path
            .strip_prefix(root)
            .unwrap_or(&entry.path)
            .to_path_buf();

        if entry.is_dir() {
            walk_summary_dir(&entry.path, root, options, matcher, summary)?;
            if should_print_entry(&entry, &rel_path, options, matcher) {
                summary.total_dirs += 1;
            }
        } else if should_print_entry(&entry, &rel_path, options, matcher) {
            summary.total_files += 1;
            summary.total_bytes += entry.size();
            add_extension_stat(summary, &entry);
        }
    }
    Ok(())
}

fn add_extension_stat(summary: &mut ListingSummary, entry: &EntryInfo) {
    if !entry.file_type.is_file() {
        return;
    }
    let ext = entry
        .path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let s = summary.ext.entry(ext).or_default();
    s.files += 1;
    s.bytes += entry.size();
}

#[derive(Debug, Serialize)]
struct JsonOutput {
    root: String,
    entries: Vec<JsonEntry>,
    summary: Option<JsonSummary>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct JsonEntry {
    rel_path: String,
    name: String,
    kind: String,
    size: u64,
    modified: Option<String>,
    depth: usize,
}

#[derive(Debug, Serialize)]
struct JsonSummary {
    total_bytes: u64,
    total_files: u64,
    total_dirs: u64,
    extensions: Option<BTreeMap<String, ExtSummary>>,
}

impl DisplayEntry {
    fn to_json(&self) -> JsonEntry {
        let kind = if self.entry.is_dir() {
            "dir"
        } else if self.entry.is_symlink() {
            "symlink"
        } else {
            "file"
        };
        let rel = normalize_match_path(&self.rel_path);
        let name = self.entry.name.to_string_lossy().to_string();
        let depth = if self.rel_path == Path::new(".") {
            0
        } else {
            self.rel_path.components().count().saturating_sub(1)
        };
        let modified = self
            .entry
            .modified
            .map(humantime::format_rfc3339)
            .map(|s| s.to_string());
        JsonEntry {
            rel_path: rel,
            name,
            kind: kind.to_string(),
            size: self.entry.size(),
            modified,
            depth,
        }
    }
}

impl ListingSummary {
    fn to_json(&self, include_extensions: bool) -> JsonSummary {
        JsonSummary {
            total_bytes: self.total_bytes,
            total_files: self.total_files,
            total_dirs: self.total_dirs,
            extensions: if include_extensions {
                Some(self.ext.clone())
            } else {
                None
            },
        }
    }
}

#[derive(Debug)]
struct BrowserState {
    cwd: PathBuf,
    options: ListOptions,
}

type EntriesScrollView = ScrollView<cursive::views::NamedView<SelectView<PathBuf>>>;

fn run_interactive(start: PathBuf, options: ListOptions) -> Result<()> {
    let mut siv = cursive::crossterm();
    siv.set_theme(tui_theme());

    let start = normalize_interactive_start(start)?;
    siv.set_user_data(BrowserState {
        cwd: start,
        options,
    });

    let list = SelectView::<PathBuf>::new()
        .on_select(|siv, path| {
            if let Err(err) = update_summary(siv, path) {
                set_summary_text(siv, &format!("{err:#}"));
            }
            siv.call_on_name("entries_scroll", |view: &mut EntriesScrollView| {
                view.scroll_to_important_area();
            });
        })
        .on_submit(|siv, path| {
            if let Err(err) = interactive_open_or_select(siv, path) {
                set_summary_text(siv, &format!("{err:#}"));
            }
        })
        .with_name("entries")
        .full_height()
        .scrollable()
        .with_name("entries_scroll");

    let summary = TextView::new("Select an entry")
        .with_name("summary")
        .full_height();

    let content = LinearLayout::horizontal()
        .child(Panel::new(list).title("Entries").full_height())
        .child(ResizedView::with_min_width(
            42,
            Panel::new(summary).title("Summary"),
        ));

    let keybar = ResizedView::with_fixed_height(
        1,
        TextView::new("Enter: open   Backspace: up   h: hidden   r: refresh   q/Esc: quit"),
    );

    let layout = LinearLayout::vertical().child(content).child(keybar);

    let layout = Dialog::around(layout).title("lz");
    let root = LinearLayout::vertical()
        .child(layout)
        .child(DummyView.fixed_height(2));
    siv.add_layer(root);

    siv.add_global_callback('q', |s| s.quit());
    siv.add_global_callback(Event::Key(Key::Esc), |s| s.quit());
    siv.add_global_callback(Event::Key(Key::Backspace), |s| {
        if let Err(err) = interactive_go_up(s) {
            set_summary_text(s, &format!("{err:#}"));
        }
    });
    siv.add_global_callback('h', |s| {
        if let Err(err) = interactive_toggle_hidden(s) {
            set_summary_text(s, &format!("{err:#}"));
        }
    });
    siv.add_global_callback('r', |s| {
        if let Err(err) = interactive_reload(s) {
            set_summary_text(s, &format!("{err:#}"));
        }
    });

    interactive_reload(&mut siv)?;
    siv.run();
    Ok(())
}

fn normalize_interactive_start(start: PathBuf) -> Result<PathBuf> {
    let start = if start.is_absolute() {
        start
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(start)
    };

    Ok(fs::canonicalize(&start).unwrap_or(start))
}

fn tui_theme() -> Theme {
    let mut theme = Theme {
        shadow: true,
        ..Theme::default()
    };
    theme.palette[PaletteColor::Background] = Color::Dark(BaseColor::White);
    theme.palette[PaletteColor::View] = Color::Dark(BaseColor::Green);
    theme.palette[PaletteColor::Primary] = Color::Light(BaseColor::Black);
    theme.palette[PaletteColor::TitlePrimary] = Color::Light(BaseColor::White);
    theme.palette[PaletteColor::Highlight] = Color::Dark(BaseColor::Red);
    theme.palette[PaletteColor::HighlightText] = Color::Dark(BaseColor::Blue);
    theme.shadow = true;
    theme
}

fn interactive_reload(siv: &mut Cursive) -> Result<()> {
    let (cwd, options) = siv
        .user_data::<BrowserState>()
        .map(|s| (s.cwd.clone(), s.options.clone()))
        .context("Missing browser state")?;

    let mut entries = read_entries(&cwd, options.all)?;
    sort_entries(&mut entries, options.sort, options.reverse);

    let mut select = siv
        .find_name::<SelectView<PathBuf>>("entries")
        .context("Missing entries view")?;
    select.clear();
    for entry in &entries {
        let label = tui_label(entry, &options);
        select.add_item(label, entry.path.clone());
    }

    siv.set_window_title(format!("lz interactive - {}", cwd.display()));

    if let Some(first) = entries.first() {
        update_summary(siv, &first.path)?;
    } else {
        set_summary_text(siv, "(empty)");
    }

    Ok(())
}

fn tui_label(entry: &EntryInfo, options: &ListOptions) -> String {
    let icon = if options.icons {
        if entry.is_dir() {
            "📁 "
        } else if entry.is_symlink() {
            "🔗 "
        } else {
            "📄 "
        }
    } else {
        ""
    };

    let mut label = format!("{icon}{}", entry.name.to_string_lossy());
    if entry.is_dir() {
        label.push(std::path::MAIN_SEPARATOR);
    }
    label
}

fn interactive_toggle_hidden(siv: &mut Cursive) -> Result<()> {
    siv.with_user_data(|state: &mut BrowserState| {
        state.options.all = !state.options.all;
    })
    .context("Missing browser state")?;
    interactive_reload(siv)
}

fn interactive_go_up(siv: &mut Cursive) -> Result<()> {
    siv.with_user_data(|state: &mut BrowserState| {
        if let Some(parent) = state.cwd.parent() {
            state.cwd = parent.to_path_buf();
        }
    })
    .context("Missing browser state")?;
    interactive_reload(siv)
}

fn interactive_open_or_select(siv: &mut Cursive, path: &Path) -> Result<()> {
    let md = fs::symlink_metadata(path)?;
    if md.is_dir() {
        siv.with_user_data(|state: &mut BrowserState| state.cwd = path.to_path_buf())
            .context("Missing browser state")?;
        interactive_reload(siv)?;
    } else {
        update_summary(siv, path)?;
    }
    Ok(())
}

fn update_summary(siv: &mut Cursive, path: &Path) -> Result<()> {
    let md = fs::symlink_metadata(path)?;
    let file_type = md.file_type();

    let kind = if file_type.is_dir() {
        "Directory"
    } else if file_type.is_symlink() {
        "Symlink"
    } else {
        "File"
    };

    let mut text = String::new();
    text.push_str(&format!("Path: {}\n", path.display()));
    text.push_str(&format!("Type: {kind}\n"));
    text.push_str(&format!(
        "Modified: {}\n",
        md.modified()
            .ok()
            .map(humantime::format_rfc3339)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));

    if file_type.is_file() {
        text.push_str(&format!("Size: {}\n", format_size(md.len(), true)));
    } else if file_type.is_dir() {
        let (dirs, files) = count_children(path)?;
        text.push_str(&format!("Children: {dirs} dirs, {files} files\n"));
    }

    text.push_str(&format!(
        "Writable: {}\n",
        if md.permissions().readonly() {
            "no"
        } else {
            "yes"
        }
    ));

    set_summary_text(siv, &text);
    Ok(())
}

fn count_children(dir: &Path) -> Result<(u64, u64)> {
    let mut dirs = 0u64;
    let mut files = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let md = entry.metadata()?;
        if md.is_dir() {
            dirs += 1;
        } else {
            files += 1;
        }
    }
    Ok((dirs, files))
}

fn set_summary_text(siv: &mut Cursive, text: &str) {
    if let Some(mut view) = siv.find_name::<TextView>("summary") {
        view.set_content(text.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(path: &Path) -> EntryInfo {
        let metadata = fs::symlink_metadata(path).unwrap();
        let name = path.file_name().unwrap().to_os_string();
        EntryInfo {
            name,
            path: path.to_path_buf(),
            file_type: metadata.file_type(),
            modified: metadata.modified().ok(),
            metadata,
        }
    }

    #[test]
    fn sort_dirs_first() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("adir");
        let file = td.path().join("bfile");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&file, b"x").unwrap();

        let mut entries = vec![make_entry(&file), make_entry(&dir)];
        sort_entries(&mut entries, SortKey::Name, false);

        assert!(entries[0].is_dir());
        assert!(!entries[1].is_dir());
    }

    #[test]
    fn format_size_human() {
        assert_eq!(format_size(0, true), "0 B");
        assert_eq!(format_size(1024, true), "1.0 KiB");
        assert_eq!(format_size(1536, true), "1.5 KiB");
    }
}
