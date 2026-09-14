use std::{
    path::{Path, PathBuf},
    process,
};

use chrono::{Local, TimeZone};
use config_file::FromConfigFile;
use regex::Regex;
use serde::Deserialize;

use crate::{cli::Cli, dir_walker::Operator, display::get_number_format, node::FileTime};

pub static DAY_SECONDS: i64 = 24 * 60 * 60;

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub display_full_paths: Option<bool>,
    pub display_apparent_size: Option<bool>,
    pub reverse: Option<bool>,
    pub no_colors: Option<bool>,
    pub force_colors: Option<bool>,
    pub dim: Option<bool>,
    pub no_bars: Option<bool>,
    pub skip_total: Option<bool>,
    pub screen_reader: Option<bool>,
    pub ignore_hidden: Option<bool>,
    pub limit_filesystem: Option<bool>,
    pub output_format: Option<String>,
    pub min_size: Option<String>,
    pub only_dir: Option<bool>,
    pub only_file: Option<bool>,
    pub disable_progress: Option<bool>,
    pub depth: Option<usize>,
    pub bars_on_right: Option<bool>,
    pub threads: Option<usize>,
    pub output_json: Option<bool>,
    pub print_errors: Option<bool>,
    pub files0_from: Option<String>,
    pub number_of_lines: Option<usize>,
    pub files_from: Option<String>,
    pub collapse: Option<Vec<String>>,
}

impl Config {
    pub fn get_files0_from(&self, options: &Cli) -> Option<String> {
        let from_file = &options.files0_from;
        match from_file {
            None => self.files0_from.clone(),
            Some(x) => Some(x.clone()),
        }
    }

    pub fn get_files_from(&self, options: &Cli) -> Option<String> {
        let from_file = &options.files_from;
        match from_file {
            None => self.files_from.clone(),
            Some(x) => Some(x.clone()),
        }
    }

    pub fn get_no_colors(&self, options: &Cli) -> bool {
        Some(true) == self.no_colors || options.no_colors
    }

    pub fn get_force_colors(&self, options: &Cli) -> bool {
        Some(true) == self.force_colors || options.force_colors
    }

    pub fn get_disable_progress(&self, options: &Cli) -> bool {
        Some(true) == self.disable_progress || options.no_progress
    }

    pub fn get_apparent_size(&self, options: &Cli) -> bool {
        Some(true) == self.display_apparent_size || options.apparent_size
    }

    pub fn get_ignore_hidden(&self, options: &Cli) -> bool {
        Some(true) == self.ignore_hidden || options.ignore_hidden
    }

    pub fn get_limit_filesystem(&self, options: &Cli) -> bool {
        Some(true) == self.limit_filesystem || options.limit_filesystem
    }

    pub fn get_full_paths(&self, options: &Cli) -> bool {
        Some(true) == self.display_full_paths || options.full_paths
    }

    pub fn get_reverse(&self, options: &Cli) -> bool {
        Some(true) == self.reverse || options.reverse
    }

    pub fn get_no_bars(&self, options: &Cli) -> bool {
        Some(true) == self.no_bars || options.no_percent_bars
    }

    pub fn get_output_format(&self, options: &Cli) -> String {
        let out_fmt = options.output_format;
        (match out_fmt {
            None => match &self.output_format {
                None => String::new(),
                Some(x) => x.clone(),
            },
            Some(x) => x.to_string(),
        })
        .to_lowercase()
    }

    pub fn get_filetime(options: &Cli) -> Option<FileTime> {
        options.filetime.map(FileTime::from)
    }

    pub fn get_skip_total(&self, options: &Cli) -> bool {
        Some(true) == self.skip_total || options.skip_total
    }

    pub fn get_screen_reader(&self, options: &Cli) -> bool {
        Some(true) == self.screen_reader || options.screen_reader
    }

    pub fn get_depth(&self, options: &Cli) -> usize {
        if let Some(v) = options.depth {
            return v;
        }

        self.depth.unwrap_or(usize::MAX)
    }

    pub fn get_min_size(&self, options: &Cli) -> Option<usize> {
        self.get_min_size_from(options.min_size.as_ref())
    }

    fn get_min_size_from(&self, min_size: Option<&String>) -> Option<usize> {
        match min_size {
            // CLI wins: an explicitly invalid value is a user error, not a
            // reason to silently fall back to the config value
            Some(cli_val) => Some(parse_min_size(cli_val).unwrap_or_else(|| {
                eprintln!("Invalid --min-size value: {cli_val:?}");
                process::exit(1)
            })),
            None => self.min_size.as_ref().and_then(|c| convert_min_size(c)),
        }
    }

    pub fn get_only_dir(&self, options: &Cli) -> bool {
        Some(true) == self.only_dir || options.only_dir
    }

    pub fn get_print_errors(&self, options: &Cli) -> bool {
        Some(true) == self.print_errors || options.print_errors
    }

    pub fn get_only_file(&self, options: &Cli) -> bool {
        Some(true) == self.only_file || options.only_file
    }

    pub fn get_bars_on_right(&self, options: &Cli) -> bool {
        Some(true) == self.bars_on_right || options.bars_on_right
    }

    pub fn get_dim(&self, options: &Cli) -> bool {
        Some(true) == self.dim || options.dim
    }

    pub fn get_threads(&self, options: &Cli) -> Option<usize> {
        let from_cmd_line = options.threads;
        if from_cmd_line.is_none() {
            self.threads
        } else {
            from_cmd_line
        }
    }

    pub fn get_output_json(&self, options: &Cli) -> bool {
        Some(true) == self.output_json || options.output_json
    }

    pub fn get_number_of_lines(&self, options: &Cli) -> Option<usize> {
        let from_cmd_line = options.number_of_lines;
        if from_cmd_line.is_none() {
            self.number_of_lines
        } else {
            from_cmd_line
        }
    }

    pub fn get_modified_time_operator(options: &Cli) -> Option<(Operator, i64)> {
        // Lazily evaluated: only computes midnight when a time filter is given
        get_filter_time_operator(options.mtime.as_ref(), get_current_date_epoch_seconds)
    }

    pub fn get_accessed_time_operator(options: &Cli) -> Option<(Operator, i64)> {
        get_filter_time_operator(options.atime.as_ref(), get_current_date_epoch_seconds)
    }

    pub fn get_changed_time_operator(options: &Cli) -> Option<(Operator, i64)> {
        get_filter_time_operator(options.ctime.as_ref(), get_current_date_epoch_seconds)
    }

    pub fn get_collapse(&self, options: &Cli) -> Option<Vec<String>> {
        // command line wins, as in get_threads and get_number_of_lines
        if options.collapse.is_none() {
            self.collapse.clone()
        } else {
            options.collapse.clone()
        }
    }
}

fn get_current_date_epoch_seconds() -> i64 {
    // calculate current date epoch seconds
    let now = Local::now();
    let current_date = now.date_naive();

    let midnight = current_date.and_hms_opt(0, 0, 0).unwrap();
    match Local.from_local_datetime(&midnight) {
        // DST gap (LocalResult::None) or fold (Ambiguous): fall back to the
        // day's UTC midnight instead of panicking, which used to make every
        // zdu invocation crash for the whole day in DST-transition timezones
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        chrono::LocalResult::None => midnight.and_utc().timestamp(),
    }
}

fn get_filter_time_operator(
    option_value: Option<&String>,
    // Called only when a time filter is present, so the (slightly costly)
    // midnight computation is skipped for plain runs
    current_date_epoch_seconds: impl FnOnce() -> i64,
) -> Option<(Operator, i64)> {
    match option_value {
        Some(val) => {
            let days = val.parse::<i64>().unwrap_or_else(|_| {
                eprintln!("Invalid value for time filter: {val:?}");
                process::exit(1)
            });
            let time = current_date_epoch_seconds() - days.abs() * DAY_SECONDS;
            // the parse above rejects an empty string, so there is a first char
            match val.chars().next().unwrap_or_else(|| {
                eprintln!("Invalid value for time filter: {val:?}");
                process::exit(1)
            }) {
                '+' => Some((Operator::LessThan, time - DAY_SECONDS)),
                '-' => Some((Operator::GreaterThan, time)),
                _ => Some((Operator::Equal, time - DAY_SECONDS)),
            }
        },
        None => None,
    }
}

fn convert_min_size(input: &str) -> Option<usize> {
    parse_min_size(input).or_else(|| {
        eprintln!("Ignoring invalid min-size: {input}");
        None
    })
}

fn parse_min_size(input: &str) -> Option<usize> {
    // Anchored with optional decimals so "1.5k" no longer parses as 1 and
    // garbage input is rejected wholesale
    let re = Regex::new(r"^([0-9]+(?:\.[0-9]+)?)([a-zA-Z]*)$").unwrap();

    let (_, [number, letters]) = re.captures(input).map(|c| c.extract())?;

    let multiple: u128 = if letters.is_empty() {
        1
    } else {
        let (multiple, _) = get_number_format(&letters.to_lowercase())?;
        u128::from(multiple)
    };

    // Integer arithmetic with checked ops so oversized input errors out
    // instead of overflowing (u128 has ample headroom for digit counts a
    // usize string can produce)
    let (int_part, frac_part) = match number.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (number, None),
    };
    let int_val: u128 = int_part.parse().ok()?;
    let mut total = int_val.checked_mul(multiple)?;

    if let Some(frac) = frac_part {
        let frac_val: u128 = frac.parse().ok()?;
        let denom = 10u128.checked_pow(u32::try_from(frac.len()).ok()?)?;
        let numerator = frac_val
            .checked_mul(multiple)
            .and_then(|n| n.checked_add(denom / 2))?; // round half up
        total = total.checked_add(numerator / denom)?;
    }

    usize::try_from(total).ok()
}

fn get_config_locations(base: &Path, config_home: Option<&Path>) -> Vec<PathBuf> {
    let config_dir = match config_home {
        Some(path) => path.join("zdu"),
        None => base.join(".config").join("zdu"),
    };
    vec![base.join(".zdu.toml"), config_dir.join("config.toml")]
}

pub fn get_config(conf_path: Option<&String>) -> Config {
    match conf_path {
        Some(path_str) => {
            let path = Path::new(path_str);
            if path.exists() {
                match Config::from_config_file(path) {
                    Ok(config) => return config,
                    Err(e) => {
                        eprintln!("Ignoring invalid config file '{}': {}", path.display(), e);
                    },
                }
            } else {
                eprintln!("Config file {:?} doesn't exists", path.display());
            }
        },
        None => {
            if let Some(home) = std::env::home_dir() {
                let config_home = std::env::var_os("XDG_CONFIG_HOME")
                    .filter(|path| !path.is_empty() && Path::new(path).is_absolute())
                    .map(PathBuf::from);

                for path in get_config_locations(&home, config_home.as_deref()) {
                    if path.exists()
                        && let Ok(config) = Config::from_config_file(&path)
                    {
                        return config;
                    }
                }
            }
        },
    }
    Config {
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, Timelike};
    use clap::Parser;

    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn config_locations_use_xdg_config_home() {
        let home = PathBuf::from("/home/test");
        let config_home = PathBuf::from("/tmp/config");

        assert_eq!(get_config_locations(&home, Some(&config_home)), vec![
            home.join(".zdu.toml"),
            PathBuf::from("/tmp/config/zdu/config.toml"),
        ]);
        assert_eq!(get_config_locations(&home, None), vec![
            home.join(".zdu.toml"),
            home.join(".config/zdu/config.toml"),
        ]);
    }

    #[test]
    fn test_get_current_date_epoch_seconds() {
        let epoch_seconds = get_current_date_epoch_seconds();
        let dt = Local.timestamp_opt(epoch_seconds, 0).unwrap();

        assert_eq!(dt.hour(), 0);
        assert_eq!(dt.minute(), 0);
        assert_eq!(dt.second(), 0);
        assert_eq!(dt.date_naive().day(), Local::now().date_naive().day());
        assert_eq!(dt.date_naive().month(), Local::now().date_naive().month());
        assert_eq!(dt.date_naive().year(), Local::now().date_naive().year());
    }

    #[test]
    fn test_conversion() {
        assert_eq!(convert_min_size("55"), Some(55));
        assert_eq!(convert_min_size("12344321"), Some(12_344_321));
        assert_eq!(convert_min_size("95RUBBISH"), None);
        assert_eq!(convert_min_size("10Ki"), Some(10 * 1024));
        assert_eq!(convert_min_size("10MiB"), Some(10 * 1024usize.pow(2)));
        assert_eq!(convert_min_size("10M"), Some(10 * 1024usize.pow(2)));
        assert_eq!(convert_min_size("10Mb"), Some(10 * 1000usize.pow(2)));
        assert_eq!(convert_min_size("2Gi"), Some(2 * 1024usize.pow(3)));
        // Decimals are supported; anchored so garbage is rejected wholesale
        assert_eq!(convert_min_size("1.5k"), Some(1536));
        assert_eq!(convert_min_size("1.5Ki"), Some(1536));
        assert_eq!(convert_min_size("0.5"), Some(1));
        assert_eq!(convert_min_size("1k "), None);
        assert_eq!(convert_min_size("k"), None);
        // Overflows checked, not wrapped (regression: used to panic in debug)
        assert_eq!(convert_min_size("9999999999999999999k"), None);
    }

    #[test]
    fn test_min_size_from_config_applied_or_overridden() {
        let c = Config {
            min_size: Some("1KiB".to_owned()),
            ..Default::default()
        };
        assert_eq!(c.get_min_size_from(None), Some(1024));
        assert_eq!(c.get_min_size_from(Some(&"2KiB".into())), Some(2048));

        assert_eq!(c.get_min_size_from(Some(&"1kb".into())), Some(1000));
        assert_eq!(c.get_min_size_from(Some(&"2KB".into())), Some(2000));
    }

    #[test]
    fn test_get_depth() {
        // No config and no flag.
        let c = Config::default();
        let args = get_args(vec![]);
        assert_eq!(c.get_depth(&args), usize::MAX);

        // Config is not defined and flag is defined.
        let c = Config::default();
        let args = get_args(vec!["zdu", "--depth", "5"]);
        assert_eq!(c.get_depth(&args), 5);

        // Config is defined and flag is not defined.
        let c = Config {
            depth: Some(3),
            ..Default::default()
        };
        let args = get_args(vec![]);
        assert_eq!(c.get_depth(&args), 3);

        // Both config and flag are defined.
        let c = Config {
            depth: Some(3),
            ..Default::default()
        };
        let args = get_args(vec!["zdu", "--depth", "5"]);
        assert_eq!(c.get_depth(&args), 5);
    }

    fn get_args(args: Vec<&str>) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn test_get_filetime() {
        // No config and no flag.
        let args = get_filetime_args(vec!["zdu"]);
        assert_eq!(Config::get_filetime(&args), None);

        // Config is not defined and flag is defined as access time
        let args = get_filetime_args(vec!["zdu", "--filetime", "a"]);
        assert_eq!(Config::get_filetime(&args), Some(FileTime::Accessed));

        let args = get_filetime_args(vec!["zdu", "--filetime", "accessed"]);
        assert_eq!(Config::get_filetime(&args), Some(FileTime::Accessed));

        // Config is not defined and flag is defined as modified time
        let args = get_filetime_args(vec!["zdu", "--filetime", "m"]);
        assert_eq!(Config::get_filetime(&args), Some(FileTime::Modified));

        let args = get_filetime_args(vec!["zdu", "--filetime", "modified"]);
        assert_eq!(Config::get_filetime(&args), Some(FileTime::Modified));

        // Config is not defined and flag is defined as changed time
        let args = get_filetime_args(vec!["zdu", "--filetime", "c"]);
        assert_eq!(Config::get_filetime(&args), Some(FileTime::Changed));

        let args = get_filetime_args(vec!["zdu", "--filetime", "changed"]);
        assert_eq!(Config::get_filetime(&args), Some(FileTime::Changed));
    }

    fn get_filetime_args(args: Vec<&str>) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn test_get_number_of_lines() {
        // No config and no flag.
        let c = Config::default();
        let args = get_args(vec![]);
        assert_eq!(c.get_number_of_lines(&args), None);

        // Config is not defined and flag is defined.
        let c = Config::default();
        let args = get_args(vec!["zdu", "--number-of-lines", "5"]);
        assert_eq!(c.get_number_of_lines(&args), Some(5));

        // Config is defined and flag is not defined.
        let c = Config {
            number_of_lines: Some(3),
            ..Default::default()
        };
        let args = get_args(vec![]);
        assert_eq!(c.get_number_of_lines(&args), Some(3));

        // Both config and flag are defined.
        let c = Config {
            number_of_lines: Some(3),
            ..Default::default()
        };
        let args = get_args(vec!["zdu", "--number-of-lines", "5"]);
        assert_eq!(c.get_number_of_lines(&args), Some(5));
    }

    #[test]
    fn test_get_number_of_lines_with_output_json() {
        // Json output and no number-of-lines: main defaults this to usize::MAX.
        let c = Config::default();
        let args = get_args(vec!["zdu", "--output-json"]);
        assert!(c.get_output_json(&args));
        assert_eq!(c.get_number_of_lines(&args), None);

        // Json output from config and no number-of-lines.
        let c = Config {
            output_json: Some(true),
            ..Default::default()
        };
        let args = get_args(vec![]);
        assert!(c.get_output_json(&args));
        assert_eq!(c.get_number_of_lines(&args), None);

        // An explicit number-of-lines still wins over the json default.
        let c = Config::default();
        let args = get_args(vec!["zdu", "--output-json", "--number-of-lines", "5"]);
        assert!(c.get_output_json(&args));
        assert_eq!(c.get_number_of_lines(&args), Some(5));

        // A number-of-lines from the config file also wins.
        let c = Config {
            number_of_lines: Some(3),
            ..Default::default()
        };
        let args = get_args(vec!["zdu", "--output-json"]);
        assert!(c.get_output_json(&args));
        assert_eq!(c.get_number_of_lines(&args), Some(3));
    }
}
