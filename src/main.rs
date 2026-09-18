mod cli;
mod config;
mod dir_walker;
mod display;
mod display_node;
mod filter;
mod filter_type;
mod node;
mod platform;
mod progress;
mod utils;

use std::{
    cmp::max,
    collections::HashSet,
    env,
    fs::{read, read_to_string},
    io,
    io::{Read, Write},
    panic,
    path::PathBuf,
    process,
    sync::{Arc, Mutex},
};

use clap::Parser;
use config::get_config;
use dir_walker::{WalkData, walk_it};
use display::InitialDisplayData;
use display_node::{JsonSizeFormat, OUTPUT_TYPE};
use filter::{AggregateData, get_biggest};
use filter_type::get_all_file_types;
use progress::PIndicator;
use regex::Regex;
use terminal_size::{Height, Width, terminal_size};
use utils::{canonicalize_absolute_path, get_filesystem_devices, simplify_dir_names};

use self::display::draw_it;
use crate::{
    cli::Cli, config::Config, display_node::DisplayNode, node::FileTime, progress::RuntimeErrors,
};

static DEFAULT_NUMBER_OF_LINES: usize = 30;
static DEFAULT_TERMINAL_WIDTH: usize = 80;

fn should_init_color(no_color: bool, force_color: bool) -> bool {
    if force_color {
        return true;
    }
    if no_color {
        return false;
    }
    // check if NO_COLOR is set
    // https://no-color.org/
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if terminal_size().is_none() {
        // we are not in a terminal, color may not be needed
        return false;
    }
    // we are in a terminal
    #[cfg(windows)]
    {
        // Required for windows 10
        // Fails to resolve for windows 8 so disable color
        if let Ok(()) = nu_ansi_term::enable_ansi_support() {
            true
        } else {
            eprintln!("This version of Windows does not support ANSI colors");
            false
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

fn get_height_of_terminal() -> usize {
    terminal_size()
        // Windows CI runners detect a terminal height of 0
        .map_or(DEFAULT_NUMBER_OF_LINES, |(_, Height(h))| max(h.into(), DEFAULT_NUMBER_OF_LINES))
        - 10
}

fn get_width_of_terminal() -> usize {
    // BUG-18: the old Windows-only max(w, 80) clamp assumed the detected
    // width was unreliable; terminal_size 0.4 reads the actual console
    // buffer size on Windows, so the clamp only forced misaligned wrapping
    // in terminals narrower than 80 columns. --width/-w remains the explicit
    // override for terminals that do report nonsense.
    terminal_size().map_or(DEFAULT_TERMINAL_WIDTH, |(Width(w), _)| w.into())
}

fn get_regex_value(maybe_value: Option<&Vec<String>>) -> Vec<Regex> {
    maybe_value
        .unwrap_or(&Vec::new())
        .iter()
        .map(|reg| {
            // Unlike a bad line in an -I ignore file (skipped with a
            // warning), an invalid CLI regex is fatal; the message must
            // say so instead of claiming the value is ignored
            Regex::new(reg).unwrap_or_else(|err| {
                eprintln!("Invalid regex {reg:?}: {err}; exiting");
                process::exit(1)
            })
        })
        .collect()
}

fn main() {
    let options = Cli::parse();
    let config = get_config(options.config.as_ref());

    // The ctrl-c handler just exits, so a single shared error sink suffices
    let errors_for_rayon = Arc::new(Mutex::new(RuntimeErrors::default()));

    ctrlc::set_handler(move || {
        println!("\nAborting");
        process::exit(1);
    })
    .expect("Error setting Ctrl-C handler");

    // clap only guards the command line; both keys set in the config file
    // would silently resolve to files0_from
    if config.files0_from.is_some() && config.files_from.is_some() {
        eprintln!("Warning: config sets both files0-from and files-from; using files0-from");
    }

    // BUG-6's clap conflicts cannot see the config file, so a config
    // files-from used to silently drop positional params with exit 0: the
    // scanned roots then have nothing to do with what the user just typed.
    // GNU du rejects the same combination on the CLI ("extra operand"), so
    // fail fast here too. One-run override: pass --files-from/--files0-from
    // explicitly (they take precedence over the config, BUG-5) or drop the
    // config key.
    if files_from_config_conflicts_with_params(&config, &options) {
        eprintln!(
            "positional arguments cannot be combined with files-from/files0-from set in the \
             config file (the arguments would be ignored); pass --files-from/--files0-from to \
             override the config for this run, or remove the config key"
        );
        process::exit(1);
    }

    let target_dirs = if let Some(path) = config.get_files0_from(&options) {
        read_paths_from_source(&path, true)
    } else if let Some(path) = config.get_files_from(&options) {
        read_paths_from_source(&path, false)
    } else {
        match options.params {
            Some(ref values) => values.clone(),
            None => vec![".".to_owned()],
        }
    }
    .into_iter()
    .filter(|path| !path.is_empty())
    .collect::<Vec<_>>();

    let summarize_file_types = options.file_types;

    let filter_regexs = get_regex_value(options.filter.as_ref());
    let invert_filter_regexs = get_regex_value(options.invert_filter.as_ref());

    let terminal_width: usize = match options.terminal_width {
        Some(val) => val,
        None => get_width_of_terminal(),
    };

    let depth = config.get_depth(&options);

    // If depth is set, or the output is json (which is not rendered to a
    // terminal), then we set the default number_of_lines to be max
    // instead of screen height

    let number_of_lines = match config.get_number_of_lines(&options) {
        Some(val) => val,
        None => {
            if depth != usize::MAX || config.get_output_json(&options) {
                usize::MAX
            } else {
                get_height_of_terminal()
            }
        },
    };

    let is_colors = should_init_color(
        config.get_no_colors(&options),
        config.get_force_colors(&options),
    );

    let ignore_directories = match options.ignore_directory {
        Some(ref values) => values
            .iter()
            .map(PathBuf::from)
            .map(canonicalize_absolute_path)
            .collect::<Vec<PathBuf>>(),
        None => vec![],
    };

    let ignore_from_file_result = match options.ignore_all_in_file {
        Some(ref val) => match read_to_string(val) {
            // BUG-8: a blank line compiles to an empty regex that matches
            // every path, zeroing the whole tree via the invert filter;
            // skip blank lines and '#' comments like gitignore does
            Ok(content) => content
                .lines()
                .filter(|line| {
                    let trimmed = line.trim_start();
                    !trimmed.is_empty() && !trimmed.starts_with('#')
                })
                .map(Regex::new)
                .collect::<Vec<_>>(),
            Err(e) => {
                eprintln!("Failed to read ignore file '{val}': {e}");
                process::exit(1)
            },
        },
        None => vec![],
    };
    let ignore_from_file = ignore_from_file_result
        .into_iter()
        .filter_map(|result| {
            // Warn on bad regex lines instead of silently dropping them
            result
                .map_err(|e| eprintln!("Ignoring invalid regex in ignore file: {e}"))
                .ok()
        })
        .collect::<Vec<Regex>>();

    let invert_filter_regexs = invert_filter_regexs
        .into_iter()
        .chain(ignore_from_file)
        .collect::<Vec<Regex>>();

    let by_filecount = options.filecount;
    let by_filetime = Config::get_filetime(&options);
    let limit_filesystem = config.get_limit_filesystem(&options);
    let follow_links = options.dereference_links;

    let allowed_filesystems = if limit_filesystem {
        get_filesystem_devices(&target_dirs)
    } else {
        HashSet::default()
    };

    let simplified_dirs = simplify_dir_names(&target_dirs);

    let ignored_full_path: HashSet<PathBuf> = ignore_directories
        .into_iter()
        .flat_map(|x| simplified_dirs.iter().map(move |d| d.join(&x)))
        .collect();

    // PERF-6: absolute ignores pre-extracted once so the per-entry ignore
    // check never rescans the whole set for this run-constant
    let absolute_ignore_directories: Vec<PathBuf> = ignored_full_path
        .iter()
        .filter(|p| p.is_absolute())
        .cloned()
        .collect();

    let output_format = config.get_output_format(&options);

    let ignore_hidden = config.get_ignore_hidden(&options);

    let mut indicator = PIndicator::build_me();
    if !config.get_disable_progress(&options) {
        indicator.spawn(output_format.clone());
    }

    let keep_collapsed: HashSet<PathBuf> = match config.get_collapse(&options) {
        Some(ref collapse) => {
            let mut combined_dirs = HashSet::new();
            for collapse_dir in collapse {
                for target_dir in &target_dirs {
                    combined_dirs.insert(PathBuf::from(target_dir).join(collapse_dir));
                }
            }
            combined_dirs
        },
        None => HashSet::new(),
    };

    let filter_modified_time = Config::get_modified_time_operator(&options);
    let filter_accessed_time = Config::get_accessed_time_operator(&options);
    let filter_changed_time = Config::get_changed_time_operator(&options);

    let walk_data = WalkData {
        ignore_directories: ignored_full_path,
        absolute_ignore_directories,
        filter_regex: &filter_regexs,
        invert_filter_regex: &invert_filter_regexs,
        allowed_filesystems,
        filter_modified_time,
        filter_accessed_time,
        filter_changed_time,
        use_apparent_size: config.get_apparent_size(&options),
        by_filecount,
        by_filetime: &by_filetime,
        ignore_hidden,
        follow_links,
        progress_data: indicator.data.clone(),
        errors: errors_for_rayon,
    };

    let threads_to_use = config.get_threads(&options);

    if options.stack_size.is_some() {
        eprintln!(
            "warning: --stack-size/-S is deprecated and ignored; the walker no longer recurses, \
             so a custom stack is unnecessary."
        );
    }

    init_rayon(threads_to_use.as_ref()).install(|| {
        let top_level_nodes = walk_it(simplified_dirs, &walk_data);

        let tree = if summarize_file_types {
            get_all_file_types(
                &top_level_nodes,
                number_of_lines,
                walk_data.by_filetime.as_ref(),
            )
        } else {
            let agg_data = AggregateData {
                min_size: config.get_min_size(&options),
                only_dir: config.get_only_dir(&options),
                only_file: config.get_only_file(&options),
                number_of_lines,
                depth,
                using_a_filter: !filter_regexs.is_empty() || !invert_filter_regexs.is_empty(),
                short_paths: !config.get_full_paths(&options),
            };
            get_biggest(
                top_level_nodes,
                &agg_data,
                walk_data.by_filetime.as_ref(),
                &keep_collapsed,
            )
        };

        // Must have stopped indicator before we print to stderr
        indicator.stop();

        let print_errors = config.get_print_errors(&options);
        let final_errors = walk_data.errors.lock().unwrap();
        print_any_errors(print_errors, &final_errors);

        // Exit 1 only when nothing was scanned and a root argument was
        // unusable. not_a_directory/eintr_exhausted join file_not_found:
        // an empty tree next to "successful" exit 0 hides the failure
        // (BUG-17).
        if tree.children.is_empty()
            && !(final_errors.file_not_found.is_empty()
                && final_errors.not_a_directory.is_empty()
                && final_errors.eintr_exhausted.is_empty())
        {
            process::exit(1)
        }
        print_output(
            &config,
            &options,
            &tree,
            walk_data.by_filecount,
            by_filetime,
            is_colors,
            terminal_width,
            output_format,
        );
    });
}

// PERF-8: by_filetime and output_format are computed once in main and
// passed in; they used to be recomputed here (get_filetime, get_output_format)
fn print_output(
    config: &Config,
    options: &Cli,
    tree: &DisplayNode,
    by_filecount: bool,
    by_filetime: Option<FileTime>,
    is_colors: bool,
    terminal_width: usize,
    output_format: String,
) {
    if config.get_output_json(options) {
        OUTPUT_TYPE.with(|wrapped| {
            // -m: raw integer timestamps (BUG-10); -f: raw integer counts;
            // otherwise human-readable sizes per the output format
            if by_filetime.is_some() {
                wrapped.replace(JsonSizeFormat::Timestamp);
            } else if by_filecount {
                wrapped.replace(JsonSizeFormat::Count);
            } else {
                wrapped.replace(JsonSizeFormat::Human(output_format));
            }
        });
        // PERF-10: stream the JSON instead of materializing the whole
        // String first (~tens of MB for big trees, doubling peak memory and
        // delaying all output until serialization finishes). to_writer +
        // trailing newline is byte-identical to the old
        // println!(to_string(...)).
        let stdout = io::stdout();
        let mut out = io::BufWriter::with_capacity(64 * 1024, stdout.lock());
        serde_json::to_writer(&mut out, &tree).unwrap();
        writeln!(out).unwrap();
        out.flush().unwrap();
    } else {
        let idd = InitialDisplayData {
            short_paths: !config.get_full_paths(options),
            is_reversed: !config.get_reverse(options),
            colors_on: is_colors,
            dim: config.get_dim(options),
            by_filecount,
            by_filetime,
            is_screen_reader: config.get_screen_reader(options),
            output_format,
            bars_on_right: config.get_bars_on_right(options),
        };

        draw_it(
            idd,
            tree,
            config.get_no_bars(options),
            terminal_width,
            config.get_skip_total(options),
        );
    }
}

fn print_any_errors(print_errors: bool, final_errors: &RuntimeErrors) {
    if !final_errors.file_not_found.is_empty() {
        let err = final_errors
            .file_not_found
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
            .join(", ");
        eprintln!("No such file or directory: {err}");
    }
    if !final_errors.not_a_directory.is_empty() {
        // BUG-17: exists but is neither a directory nor a regular file
        // (FIFO/socket/device); must not be reported as "not found"
        let err = final_errors
            .not_a_directory
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
            .join(", ");
        eprintln!("Not a directory: {err}");
    }
    if !final_errors.eintr_exhausted.is_empty() {
        // BUG-17: listing kept failing with EINTR after all retries
        let err = final_errors
            .eintr_exhausted
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
            .join(", ");
        eprintln!("Gave up listing after repeated interruptions (subtrees not counted): {err}");
    }
    if !final_errors.no_permissions.is_empty() {
        if print_errors {
            let err = final_errors
                .no_permissions
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<&str>>()
                .join(", ");
            eprintln!("Did not have permissions for directories: {err}");
        } else {
            eprintln!(
                "Did not have permissions for all directories (add --print-errors to see errors)"
            );
        }
    }
    if !final_errors.unknown_error.is_empty() {
        let err = final_errors
            .unknown_error
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
            .join(", ");
        eprintln!("Unknown Error: {err}");
    }
    if !final_errors.metadata_unavailable.is_empty() {
        // BUG-13: their subtree was walked but dropped from the totals
        let err = final_errors
            .metadata_unavailable
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
            .join(", ");
        eprintln!("Could not get metadata for (subtrees not counted): {err}");
    }
}

fn read_paths_from_source(path: &str, null_terminated: bool) -> Vec<String> {
    let from_stdin = path == "-";

    // Fatal on real read/decode failures: GNU du also exits 1 when it cannot
    // read its file list, and falling back to scanning the cwd turns a
    // typo into an unbounded scan with a misleading result.
    let bytes = if from_stdin {
        let mut b = Vec::new();
        io::stdin()
            .lock()
            .read_to_end(&mut b)
            .unwrap_or_else(|e| exit_read_error(path, e));
        b
    } else {
        read(path).unwrap_or_else(|e| exit_read_error(path, e))
    };

    // Invalid UTF-8 is a decode failure of the requested input, fatal even
    // on stdin (it used to be misreported as "No files provided")
    let text = std::str::from_utf8(&bytes).unwrap_or_else(|e| exit_read_error(path, e));

    if null_terminated {
        text.split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        text.lines().map(str::to_owned).collect()
    }
    // An empty source yields no roots; like GNU du, do not silently fall
    // back to the cwd (get_biggest renders an empty total and we exit 0)
}

fn exit_read_error(path: &str, msg: impl std::fmt::Display) -> ! {
    eprintln!("Failed to read paths from '{path}': {msg}");
    process::exit(1)
}

fn init_rayon(threads: Option<&usize>) -> rayon::ThreadPool {
    let mut builder = rayon::ThreadPoolBuilder::new();
    if let Some(t) = threads {
        builder = builder.num_threads(*t);
    }
    builder.build().unwrap_or_else(|err| {
        eprintln!("Problem initializing rayon, try: export RAYON_NUM_THREADS=1");
        panic!("{err}")
    })
}

// True when the effective files-from source is the config (CLI gave neither
// flag) and positional params were also passed. A CLI flag pair with params
// never gets this far: clap rejects it first.
fn files_from_config_conflicts_with_params(config: &Config, options: &Cli) -> bool {
    options.files0_from.is_none()
        && options.files_from.is_none()
        && options.params.as_ref().is_some_and(|p| !p.is_empty())
        && (config.files0_from.is_some() || config.files_from.is_some())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn test_files_from_config_conflicts_with_params() {
        let with_params = || Cli::parse_from(["zdu", "some/dir"]);
        let without_params = || Cli::parse_from(["zdu"]);

        // Either config key plus params is the silent-drop conflict
        for config in [
            Config {
                files_from: Some("list.txt".to_owned()),
                ..Default::default()
            },
            Config {
                files0_from: Some("list0.txt".to_owned()),
                ..Default::default()
            },
        ] {
            assert!(files_from_config_conflicts_with_params(
                &config,
                &with_params()
            ));
            assert!(!files_from_config_conflicts_with_params(
                &config,
                &without_params()
            ));
        }

        // No config files-from: params are simply the roots
        assert!(!files_from_config_conflicts_with_params(
            &Config::default(),
            &with_params()
        ));
    }
}
